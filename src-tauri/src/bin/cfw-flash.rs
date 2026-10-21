use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

use cfw_zero_touch_lib::backup::{self, disk_query_script};
use cfw_zero_touch_lib::flash::{
    authorize_flash, sha256_prefix, sha256_reader, write_gzip, FlashDisk, FlashPlan, SectorWriter,
};
use cfw_zero_touch_lib::restore::{
    self, cancel_file_requests_stop, hashes_match, image_fits, same_model, BoundedWrite,
    WatchedWrite, DISK_CLEARED, DISK_CLEAR_STARTED,
};

fn arg(name: &str) -> String {
    let prefix = format!("--{name}=");
    std::env::args()
        .find_map(|item| item.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("missing --{name}"))
}

/// Absent when this is an OS flash. Restore passes the flag by name.
fn optional_arg(name: &str) -> Option<String> {
    let prefix = format!("--{name}=");
    std::env::args().find_map(|item| item.strip_prefix(&prefix).map(str::to_string))
}

/// Restore passes `--restore` or one of its flags. `--log-file` is not one of them.
fn args_request_restore(args: &[String]) -> bool {
    flag_present_in(args, "restore")
        || flag_present_in(args, "sidecar-sha256")
        || flag_present_in(args, "uncompressed-bytes")
        || flag_present_in(args, "cancel-file")
        || flag_present_in(args, "progress-file")
        || flag_present_in(args, "expected-bytes")
        || flag_present_in(args, "expected-model")
}

fn flag_present_in(args: &[String], name: &str) -> bool {
    let exact = format!("--{name}");
    let prefix = format!("--{name}=");
    args.iter()
        .any(|item| item == &exact || item.starts_with(&prefix))
}

fn restore_requested() -> bool {
    let args: Vec<String> = std::env::args().collect();
    args_request_restore(&args)
}

/// In restore mode both flags are mandatory. `Ok(None)` is an OS flash.
fn require_restore_pair(
    restore: bool,
    uncompressed: Option<&str>,
    sidecar: Option<&str>,
) -> Result<(), &'static str> {
    if !restore {
        return Ok(());
    }
    if uncompressed.is_none() {
        return Err("--uncompressed-bytes");
    }
    if sidecar.is_none() {
        return Err("--sidecar-sha256");
    }
    Ok(())
}

fn parse_u64_flag(name: &str, value: Option<String>) -> Result<Option<u64>, String> {
    match value {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("bad --{name}")),
    }
}

/// `number|bus|size|IsSystem|IsBoot|FriendlyName` via [`backup::parse_disk_query`].
fn flash_disk_from_query_line(
    line: &str,
    expected_number: u32,
) -> Result<(FlashDisk, String), String> {
    let line = line.trim();
    let (head, model) = split_model(line);
    let queried = backup::parse_disk_query(head, expected_number).map_err(|error| {
        if error.contains("before the backup") {
            "the selected disk changed before the write".to_string()
        } else {
            error
        }
    })?;
    Ok((
        FlashDisk {
            number: queried.number,
            bus: queried.bus,
            size_bytes: queried.size_bytes,
            system_disk: queried.system_disk,
        },
        restore::display_model(model),
    ))
}

fn split_model(line: &str) -> (&str, &str) {
    let mut seen = 0;
    for (index, ch) in line.char_indices() {
        if ch == '|' {
            seen += 1;
            if seen == 5 {
                return (&line[..index], line[index + 1..].trim());
            }
        }
    }
    (line, "")
}

/// Recheck the size and model the operator confirmed. Absent flags are OS flash.
fn shown_disk_matches(
    live_bytes: u64,
    live_model: &str,
    expected_bytes: Option<u64>,
    expected_model: Option<&str>,
) -> Result<(), String> {
    if let Some(expected) = expected_bytes {
        if expected != live_bytes {
            return Err(format!(
                "shown disk size {expected} does not match the disk ({live_bytes} bytes)"
            ));
        }
    }
    if let Some(expected) = expected_model {
        if !same_model(expected, live_model) {
            return Err(format!(
                "shown disk model {expected} does not match the disk ({live_model})"
            ));
        }
    }
    Ok(())
}

fn hash_and_rewind(file: &mut File) -> Result<String, String> {
    let hash = sha256_reader(&mut *file)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("could not rewind the image: {error}"))?;
    Ok(hash)
}

struct RunLog {
    file: Option<File>,
}

impl RunLog {
    fn open(path: Option<&str>) -> Self {
        let file = match path {
            None => None,
            Some(path) => match restore::open_nofollow_write(Path::new(path)) {
                Ok(file) => Some(file),
                Err(error) => {
                    eprintln!("could not open log {path}: {error}");
                    std::process::exit(2);
                }
            },
        };
        Self { file }
    }

    fn line(&mut self, line: &str) {
        println!("{line}");
        let _ = self.record(line);
    }

    fn err(&mut self, line: &str) {
        eprintln!("{line}");
        let _ = self.record(line);
    }

    fn record(&mut self, line: &str) -> bool {
        let Some(file) = self.file.as_mut() else {
            return true;
        };
        writeln!(file, "{line}").is_ok() && file.flush().is_ok()
    }

    fn fail(&mut self, code: i32, line: &str) -> ! {
        self.err(line);
        self.exit(code);
    }

    fn exit(&mut self, code: i32) -> ! {
        self.line(&format!("EXIT={code}"));
        std::process::exit(code);
    }
}

fn main() {
    let image = arg("image");
    let expected = arg("sha256");
    let disk_number: u32 = arg("disk").parse().expect("disk number");
    let confirmation = arg("confirm");
    let mut log = RunLog::open(optional_arg("log-file").as_deref());

    let restore = restore_requested();
    let uncompressed_raw = optional_arg("uncompressed-bytes");
    let sidecar = optional_arg("sidecar-sha256");
    if let Err(flag) =
        require_restore_pair(restore, uncompressed_raw.as_deref(), sidecar.as_deref())
    {
        log.fail(2, &format!("restore requires {flag}"));
    }
    let uncompressed = match parse_u64_flag("uncompressed-bytes", uncompressed_raw) {
        Ok(value) => value,
        Err(error) => log.fail(2, &error),
    };
    let expected_bytes = match parse_u64_flag("expected-bytes", optional_arg("expected-bytes")) {
        Ok(value) => value,
        Err(error) => log.fail(2, &error),
    };
    let expected_model = optional_arg("expected-model");

    let mut image_file = match File::open(&image) {
        Ok(file) => file,
        Err(error) => log.fail(2, &format!("could not open {image}: {error}")),
    };
    let actual = match hash_and_rewind(&mut image_file) {
        Ok(hash) => hash,
        Err(error) => log.fail(2, &error),
    };

    let cancel_file = optional_arg("cancel-file");
    let progress_file = optional_arg("progress-file");
    // Label lookup is informational and slow. Do it before the identity
    // recheck so a card swap during that call is still visible to Get-Disk.
    if disk_has_label(disk_number, "EASYROMS") {
        log.line("card is labeled EASYROMS; FLASH confirmation is erasing it");
    }

    // After the image hash, so this Get-Disk is the last look before Clear-Disk.
    // The process is already elevated: the approval prompt happened before launch.
    let (disk, model) = match read_disk(disk_number) {
        Ok(disk) => disk,
        Err(error) => log.fail(2, &error),
    };
    log.line(&format!(
        "target disk {} {} {} bytes ({:.2} GB)",
        disk.number,
        disk.bus,
        disk.size_bytes,
        disk.size_bytes as f64 / 1_000_000_000.0
    ));
    if let Err(error) = shown_disk_matches(
        disk.size_bytes,
        &model,
        expected_bytes,
        expected_model.as_deref(),
    ) {
        log.fail(3, &error);
    }

    let displayed_bytes = expected_bytes.unwrap_or(disk.size_bytes);
    let plan = FlashPlan {
        confirmation,
        disk_number,
        displayed_bytes,
        expected_sha256: expected,
        actual_sha256: actual,
    };
    if let Err(block) = authorize_flash(&disk, &plan) {
        log.fail(3, &format!("flash blocked: {block:?}"));
    }

    if let Some(image_bytes) = uncompressed {
        if !image_fits(image_bytes, disk.size_bytes) {
            log.fail(4, "image is larger than the target disk");
        }
    }
    if let Some(path) = &cancel_file {
        match cancel_file_requests_stop(Path::new(path)) {
            Ok(true) => log.fail(9, "restore cancelled before writing"),
            Ok(false) => {}
            Err(error) => log.fail(2, &error),
        }
    }

    println!("{DISK_CLEAR_STARTED}");
    if !log.record(DISK_CLEAR_STARTED) {
        eprintln!("could not record that Clear-Disk is starting");
        std::process::exit(2);
    }
    if let Err(error) = clear_disk(disk_number) {
        log.fail(5, &error);
    }
    log.line(DISK_CLEARED);

    let written = match if cancel_file.is_some() || progress_file.is_some() {
        write_physical_watched(
            disk_number,
            &image,
            &mut image_file,
            disk.size_bytes,
            cancel_file.as_deref(),
            progress_file.as_deref(),
            uncompressed.unwrap_or(0),
        )
    } else {
        write_physical(disk_number, &image, &mut image_file, disk.size_bytes)
    } {
        Ok(written) => written,
        Err(error) => {
            let code = if cancel_file.is_some() && error.contains("restore cancelled") {
                9
            } else if error.contains("image is larger than the target disk") {
                4
            } else {
                6
            };
            log.fail(code, &error);
        }
    };
    if written > disk.size_bytes {
        log.fail(4, "image is larger than the target disk");
    }
    if let Some(path) = &progress_file {
        let line = format!("readback 0 {written}\n");
        if let Err(error) = restore::write_nofollow(Path::new(path), line.as_bytes()) {
            log.err(&format!("could not update progress: {error}"));
        }
    }
    let card_hash = match hash_physical(disk_number, written) {
        Ok(hash) => hash,
        Err(error) => log.fail(7, &error),
    };
    if let Err(error) = image_file.seek(SeekFrom::Start(0)) {
        log.fail(7, &format!("could not rewind the image: {error}"));
    }
    let image_hash = if image.ends_with(".gz") {
        sha256_prefix(flate2::read::GzDecoder::new(&mut image_file), written)
    } else {
        sha256_prefix(&mut image_file, written)
    };
    let image_hash = match image_hash {
        Ok(hash) => hash,
        Err(error) => log.fail(7, &error),
    };
    log.line(&format!("wrote {written} bytes"));
    log.line(&format!("image {image_hash}"));
    log.line(&format!("card  {card_hash}"));
    if image_hash != card_hash {
        log.fail(8, "card does not match the image");
    }
    if restore {
        let sidecar = sidecar.expect("restore requires the sidecar");
        if !hashes_match(&card_hash, &sidecar) {
            log.err("verify=fail");
            log.fail(8, "card does not match the backup checksum");
        }
        log.line("verify=pass");
    }
    log.line("RESULT=OK");
    log.exit(0);
}

fn read_disk(number: u32) -> Result<(FlashDisk, String), String> {
    let script = disk_query_script(number);
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|error| format!("could not inspect the disk: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.contains('|'))
        .ok_or("no disk info")?;
    flash_disk_from_query_line(line, number)
}

fn disk_has_label(number: u32, label: &str) -> bool {
    let script = format!(
        "Get-Partition -DiskNumber {number} | Get-Volume | Where-Object {{ $_.FileSystemLabel -eq '{label}' }} | Select-Object -ExpandProperty DriveLetter"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map(|output| !String::from_utf8_lossy(&output.stdout).trim().is_empty())
        .unwrap_or(false)
}

fn clear_disk(number: u32) -> Result<(), String> {
    let script = format!("Clear-Disk -Number {number} -RemoveData -Confirm:$false");
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|error| format!("could not clear the disk: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

fn write_physical(number: u32, image: &str, input: &mut File, limit: u64) -> Result<u64, String> {
    let mut drive = open_drive(number)?;
    let mut capped = BoundedWrite::new(&mut drive, limit);
    let mut output = SectorWriter::new(&mut capped);
    write_opened_image(image, input, &mut output)
}

fn write_opened_image(
    image: &str,
    input: &mut File,
    output: &mut impl Write,
) -> Result<u64, String> {
    if image.ends_with(".gz") {
        // `write_gzip` already flushes. A second flush would re-check cancel
        // after a finished write and skip the read-back.
        return write_gzip(&mut *input, &mut *output);
    }
    let written = write_raw(input, &mut *output)?;
    output
        .flush()
        .map_err(|error| format!("could not flush the image: {error}"))?;
    Ok(written)
}

fn write_physical_watched(
    number: u32,
    image: &str,
    input: &mut File,
    limit: u64,
    cancel_file: Option<&str>,
    progress_file: Option<&str>,
    total: u64,
) -> Result<u64, String> {
    let mut drive = open_drive(number)?;
    let mut capped = BoundedWrite::new(&mut drive, limit);
    let cancel_path = cancel_file.map(Path::new);
    let progress_path = progress_file.map(Path::new);
    let mut output = SectorWriter::new(&mut capped);
    let mut watched = WatchedWrite::new(&mut output, total, None, cancel_path, progress_path, None);
    write_opened_image(image, input, &mut watched)
}

fn write_raw(input: &mut File, output: &mut impl Write) -> Result<u64, String> {
    let mut buffer = vec![0u8; 4 * 1024 * 1024];
    let mut written = 0u64;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("could not read the image: {error}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("could not write the image: {error}"))?;
        written += read as u64;
    }
    Ok(written)
}

fn hash_physical(number: u32, length: u64) -> Result<String, String> {
    sha256_prefix(open_drive(number)?, length)
}

fn open_drive(number: u32) -> Result<File, String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(3)
            .custom_flags(FILE_FLAG_WRITE_THROUGH)
            .open(format!(r"\\.\PhysicalDrive{number}"))
            .map_err(|error| format!("could not open physical drive {number}: {error}"))
    }
    #[cfg(not(windows))]
    {
        let _ = number;
        Err("physical disk flashing is implemented for Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cfw_zero_touch_lib::flash::{sha256_bytes, FlashBlock};

    fn plan_for(disk: &FlashDisk) -> FlashPlan {
        let digest = "ab".repeat(32);
        FlashPlan {
            confirmation: "FLASH".into(),
            disk_number: disk.number,
            displayed_bytes: disk.size_bytes,
            expected_sha256: digest.clone(),
            actual_sha256: digest,
        }
    }

    #[test]
    fn system_and_boot_disks_are_refused_by_the_helper_parser() {
        for line in [
            "2|USB|4096|True|False|Sandisk Ultra",
            "2|USB|4096|False|True|Sandisk Ultra",
            "0|USB|4096|False|False|System",
        ] {
            let number: u32 = line.split('|').next().unwrap().parse().unwrap();
            let (disk, _) = flash_disk_from_query_line(line, number).unwrap();
            assert!(disk.system_disk, "{line}");
            let error = authorize_flash(&disk, &plan_for(&disk)).unwrap_err();
            assert_eq!(error, FlashBlock::NotRemovable, "{line}");
        }
        let (nvme, _) = flash_disk_from_query_line("1|NVMe|4096|False|False|Internal", 1).unwrap();
        assert!(!nvme.system_disk);
        assert_eq!(
            authorize_flash(&nvme, &plan_for(&nvme)).unwrap_err(),
            FlashBlock::NotRemovable
        );
    }

    #[test]
    fn a_plain_usb_disk_parses_and_is_allowed() {
        let (disk, model) =
            flash_disk_from_query_line("2|USB|4096|False|False|Samsung EVO", 2).unwrap();
        assert!(!disk.system_disk);
        assert_eq!(disk.size_bytes, 4096);
        assert_eq!(model, "Samsung EVO");
        authorize_flash(&disk, &plan_for(&disk)).unwrap();
    }

    #[test]
    fn disk_query_script_asks_for_system_boot_and_model() {
        let script = disk_query_script(3);
        assert!(script.contains("-Number 3"));
        assert!(script.contains("IsSystem"));
        assert!(script.contains("IsBoot"));
        assert!(script.contains("FriendlyName"));
    }

    #[test]
    fn shown_size_and_model_are_rechecked_only_when_passed() {
        shown_disk_matches(4096, "Samsung EVO", None, None).unwrap();
        shown_disk_matches(4096, "Samsung EVO", Some(4096), Some("Samsung EVO")).unwrap();
        shown_disk_matches(4096, "A/B", Some(4096), Some("A|B")).unwrap();
        let size = shown_disk_matches(4096, "Samsung EVO", Some(1024), Some("Samsung EVO"));
        assert!(size.unwrap_err().contains("size"));
        let model = shown_disk_matches(4096, "Samsung EVO", Some(4096), Some("Other"));
        assert!(model.unwrap_err().contains("model"));
    }

    #[test]
    fn log_file_alone_stays_an_os_flash() {
        let os_flash = [
            "cfw-flash".to_string(),
            "--image=os.img.gz".to_string(),
            "--log-file=C:\\Temp\\flash.txt".to_string(),
        ];
        assert!(!args_request_restore(&os_flash));
        let mut restore = os_flash.to_vec();
        restore.push("--restore".to_string());
        assert!(args_request_restore(&restore));
        assert!(args_request_restore(&[
            "cfw-flash".to_string(),
            "--expected-bytes=4096".to_string()
        ]));
    }

    #[test]
    fn restore_mode_requires_uncompressed_bytes_and_the_sidecar() {
        assert!(require_restore_pair(false, None, None).is_ok());
        assert!(require_restore_pair(true, Some("100"), Some("ab")).is_ok());
        assert_eq!(
            require_restore_pair(true, None, Some("ab")).unwrap_err(),
            "--uncompressed-bytes"
        );
        assert_eq!(
            require_restore_pair(true, Some("100"), None).unwrap_err(),
            "--sidecar-sha256"
        );
    }

    #[test]
    fn hash_and_rewind_leaves_the_same_handle_at_the_start() {
        let path = std::env::temp_dir().join(format!(
            "cfw-flash-hash-{}-{}.bin",
            std::process::id(),
            line!()
        ));
        let payload = b"abc";
        std::fs::write(&path, payload).unwrap();
        let mut file = File::open(&path).unwrap();
        let hash = hash_and_rewind(&mut file).unwrap();
        assert_eq!(hash, sha256_bytes(payload));
        let mut got = Vec::new();
        file.read_to_end(&mut got).unwrap();
        assert_eq!(got, payload);
        let _ = std::fs::remove_file(&path);
    }
}
