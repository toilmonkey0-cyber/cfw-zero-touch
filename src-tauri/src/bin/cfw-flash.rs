use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use cfw_zero_touch_lib::backup::{self, disk_query_script};
use cfw_zero_touch_lib::block_device::{is_physical_disk_path, FakeBlockDevice};
use cfw_zero_touch_lib::flash::{
    authorize_flash, sha256_prefix, sha256_reader, write_gzip, FlashDisk, FlashPlan, SectorWriter,
};
use cfw_zero_touch_lib::fullcheck::{self, FullRun, FullStatus};
use cfw_zero_touch_lib::probe::{
    self, ProbeDisk, ProbePlan, ProbeRequest, ProbeStatus, SessionBackup,
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

fn args_request_probe(args: &[String]) -> bool {
    flag_present_in(args, "probe")
}

fn args_request_full_check(args: &[String]) -> bool {
    flag_present_in(args, "full-check")
}

#[derive(Debug)]
enum CardTestMode {
    Probe,
    FullCheck,
}

/// Probe and full check are their own modes. `--expected-bytes` also marks a
/// restore, so this has to win before `arg("image")`.
fn card_test_mode(args: &[String]) -> Result<Option<CardTestMode>, &'static str> {
    match (args_request_probe(args), args_request_full_check(args)) {
        (true, true) => Err("probe and full check cannot run together"),
        (true, false) => Ok(Some(CardTestMode::Probe)),
        (false, true) => Ok(Some(CardTestMode::FullCheck)),
        (false, false) => Ok(None),
    }
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
    let args: Vec<String> = std::env::args().collect();
    match card_test_mode(&args) {
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
        Ok(Some(CardTestMode::Probe)) => {
            if flag_present_in(&args, "restore") {
                eprintln!("probe and restore cannot run together");
                std::process::exit(2);
            }
            run_probe_mode();
        }
        Ok(Some(CardTestMode::FullCheck)) => {
            if flag_present_in(&args, "restore") {
                eprintln!("full check and restore cannot run together");
                std::process::exit(2);
            }
            run_full_check_mode();
        }
        Ok(None) => {}
    }
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

/// Read-only open used to hash the card before any write handle is taken.
fn open_drive_read(number: u32) -> Result<File, String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(format!(r"\\.\PhysicalDrive{number}"))
            .map_err(|error| format!("could not read physical drive {number}: {error}"))
    }
    #[cfg(not(windows))]
    {
        let _ = number;
        Err("physical disk flashing is implemented for Windows".into())
    }
}

/// Serial, prefix, and sample blocks must match the backup. No write handle yet.
///
/// The app may skip its own read when the card cannot be opened without admin.
/// This check still runs before any lock or write.
fn live_card_matches(number: u32, disk: &ProbeDisk, backup: &SessionBackup) -> Result<(), String> {
    if probe::normalize_serial(&backup.identity.serial) != probe::normalize_serial(&disk.serial) {
        return Err(probe::ProbeBlock::Backup.message().into());
    }
    let file = open_drive_read(number)?;
    let mut device = DriveDevice {
        file,
        len: disk.size_bytes,
    };
    probe::live_identity_matches(&mut device, &backup.identity).map_err(|error| error.to_string())
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

/// Seek the logical offset. A counterfeit card aliases in its own controller;
/// this wrapper does not wrap the address in software.
struct DriveDevice {
    file: File,
    len: u64,
}

impl FakeBlockDevice for DriveDevice {
    fn reported_len(&self) -> u64 {
        self.len
    }

    fn stored_len(&self) -> u64 {
        self.len
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buf)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)?;
        self.file.flush()
    }
}

fn probe_fail(log: &mut RunLog, code: i32, overwritten: bool, message: &str) -> ! {
    for line in probe::format_failed_log(overwritten, message).lines() {
        log.line(line);
    }
    log.exit(code);
}

struct ArmedCard {
    disk: ProbeDisk,
    plan: ProbePlan,
    backup: Option<SessionBackup>,
    locks: Option<Vec<File>>,
    device: DriveDevice,
    cancel_path: Option<PathBuf>,
}

struct PrepareStop {
    code: i32,
    overwritten: bool,
    message: String,
}

enum PreparedCard {
    Ready(Box<ArmedCard>),
    Cancelled { backed_up: bool },
    Failed(PrepareStop),
}

fn stop(code: i32, overwritten: bool, message: impl Into<String>) -> PreparedCard {
    PreparedCard::Failed(PrepareStop {
        code,
        overwritten,
        message: message.into(),
    })
}

/// Shared post-approval gate for `--probe` and `--full-check`.
///
/// Rechecks size and model, refuses system, boot, disk 0, and non USB/SD
/// disks, re-reads serial plus the prefix and sample hashes on the backup
/// route, and locks volumes before the drive is opened for write. Does not
/// call `Clear-Disk`.
fn prepare_destructive(log: &mut RunLog, word: &str) -> PreparedCard {
    let label = if word == "PROBE" {
        "probe"
    } else {
        "full check"
    };
    let disk_number: u32 = match arg("disk").parse() {
        Ok(number) => number,
        Err(_) => return stop(2, false, "bad disk number"),
    };
    let confirmation = arg("confirm");
    let expected_bytes = match parse_u64_flag("expected-bytes", optional_arg("expected-bytes")) {
        Ok(Some(value)) => value,
        Ok(None) => return stop(2, false, format!("{label} requires --expected-bytes")),
        Err(error) => return stop(2, false, error),
    };
    let Some(expected_model) = optional_arg("expected-model") else {
        return stop(2, false, format!("{label} requires --expected-model"));
    };
    let backup_image = optional_arg("backup-image");
    let backup_sha = optional_arg("backup-sha256");
    let backup_prefix = optional_arg("backup-prefix-sha256");
    let backup_serial = optional_arg("backup-serial");
    let cancel_file = optional_arg("cancel-file");
    let progress_file = optional_arg("progress-file");
    match (&backup_image, &backup_sha, &backup_prefix, &backup_serial) {
        (Some(_), Some(_), Some(_), Some(_)) | (None, None, None, None) => {}
        _ => {
            return stop(
                2,
                false,
                format!(
                    "a {label} backup needs the image, checksum, and card identity from this session"
                ),
            );
        }
    }
    if let Some(path) = &backup_image {
        if is_physical_disk_path(Path::new(path)) {
            return stop(
                3,
                false,
                "refusing to read a backup from a physical disk path",
            );
        }
    }
    if let Some(path) = &cancel_file {
        if is_physical_disk_path(Path::new(path)) {
            return stop(
                3,
                false,
                "refusing to read a cancel flag from a physical disk path",
            );
        }
    }
    if let Some(path) = &progress_file {
        if is_physical_disk_path(Path::new(path)) {
            return stop(
                3,
                false,
                "refusing to write progress to a physical disk path",
            );
        }
    }

    let (disk, model) = match read_disk(disk_number) {
        Ok(disk) => disk,
        Err(error) => return stop(2, false, error),
    };
    log.line(&format!(
        "target disk {} {} {} bytes model {model}",
        disk.number, disk.bus, disk.size_bytes
    ));
    if let Err(error) = shown_disk_matches(
        disk.size_bytes,
        &model,
        Some(expected_bytes),
        Some(expected_model.as_str()),
    ) {
        return stop(3, false, error);
    }
    let live_serial = if backup_image.is_some() {
        match read_serial(disk_number) {
            Ok(serial) => serial,
            Err(error) => return stop(2, false, error),
        }
    } else {
        String::new()
    };
    let probe_disk = ProbeDisk {
        number: disk.number,
        bus: disk.bus,
        size_bytes: disk.size_bytes,
        system_disk: disk.system_disk,
        model: model.clone(),
        serial: live_serial,
    };
    let plan = ProbePlan {
        confirmation,
        disk_number,
        displayed_bytes: expected_bytes,
        displayed_model: expected_model,
    };
    if let Err(block) = probe::authorize_card_write(&probe_disk, &plan, word) {
        return stop(3, false, block.message());
    }
    if let Some(path) = &cancel_file {
        match cancel_file_requests_stop(Path::new(path)) {
            Ok(true) => {
                return PreparedCard::Cancelled {
                    backed_up: backup_image.is_some(),
                };
            }
            Ok(false) => {}
            Err(error) => return stop(2, false, error),
        }
    }

    let backup = match (backup_image, backup_sha, backup_prefix, backup_serial) {
        (Some(path), Some(sha), Some(prefix), Some(serial)) => {
            match recorded_backup(&path, &sha, &prefix, &serial) {
                Ok(backup) => Some(backup),
                Err(error) => return stop(3, false, error),
            }
        }
        _ => None,
    };
    // Re-read this card and compare it to the backup before locking or writing.
    if let Some(backup) = &backup {
        if let Err(error) = live_card_matches(disk_number, &probe_disk, backup) {
            return stop(3, false, error);
        }
    }
    // Backup route only: lock and dismount volumes before any raw write.
    // Handles stay open until the caller drops them so Windows does not remount.
    let locks = if backup.is_some() {
        match lock_disk_volumes(disk_number) {
            Ok(locks) => Some(locks),
            Err(error) => {
                let message = probe::backup_write_allowed(Err(error)).unwrap_err();
                return stop(3, false, message);
            }
        }
    } else {
        None
    };
    let file = match open_drive(disk_number) {
        Ok(file) => file,
        Err(error) => return stop(2, false, error),
    };
    let device = DriveDevice {
        file,
        len: probe_disk.size_bytes,
    };
    let cancel_path = cancel_file.map(PathBuf::from);
    PreparedCard::Ready(Box::new(ArmedCard {
        disk: probe_disk,
        plan,
        backup,
        locks,
        device,
        cancel_path,
    }))
}

fn finish_prepare(log: &mut RunLog, prepared: PreparedCard, cancelled_log: String) -> ArmedCard {
    match prepared {
        PreparedCard::Ready(armed) => *armed,
        PreparedCard::Cancelled { .. } => {
            for line in cancelled_log.lines() {
                log.line(line);
            }
            log.exit(9);
        }
        PreparedCard::Failed(stop) => probe_fail(log, stop.code, stop.overwritten, &stop.message),
    }
}

/// Quick probe. Rechecks size, model, and card identity after the approval
/// prompt, then writes patterns. Does not call Clear-Disk. Every exit writes
/// `OVERWRITTEN=yes` or `OVERWRITTEN=no`.
fn run_probe_mode() -> ! {
    let mut log = RunLog::open(optional_arg("log-file").as_deref());
    let prepared = prepare_destructive(&mut log, "PROBE");
    let cancelled = probe::format_probe_log(&ProbeStatus::Cancelled {
        overwritten: false,
        summary: probe::cancel_summary(
            false,
            matches!(prepared, PreparedCard::Cancelled { backed_up: true }),
        ),
    });
    let armed = finish_prepare(&mut log, prepared, cancelled);
    let ArmedCard {
        disk,
        plan,
        backup,
        locks,
        mut device,
        cancel_path,
    } = armed;
    let _locks = locks;
    let request = ProbeRequest {
        disk: &disk,
        plan: &plan,
        backup: backup.as_ref(),
    };
    match probe::run_probe(
        &mut device,
        &request,
        &AtomicBool::new(false),
        cancel_path.as_deref(),
    ) {
        Ok(status) => {
            let code = match &status {
                ProbeStatus::Cancelled { .. } => 9,
                ProbeStatus::Finished(_) => 0,
            };
            for line in probe::format_probe_log(&status).lines() {
                log.line(line);
            }
            log.exit(code);
        }
        Err(probe::ProbeError::Blocked(block)) => probe_fail(&mut log, 3, false, block.message()),
        Err(probe::ProbeError::Failed {
            message,
            overwritten,
        }) => probe_fail(&mut log, 6, overwritten, &message),
    }
}

/// Full-card pattern write and read-back. Same gate as the probe. Every exit
/// writes `OVERWRITTEN=yes` or `OVERWRITTEN=no`.
fn run_full_check_mode() -> ! {
    let mut log = RunLog::open(optional_arg("log-file").as_deref());
    let progress_file = optional_arg("progress-file");
    let prepared = prepare_destructive(&mut log, fullcheck::FULL_WORD);
    let backed_up = matches!(prepared, PreparedCard::Cancelled { backed_up: true });
    let cancelled = fullcheck::format_full_log(&FullStatus::Cancelled {
        overwritten: false,
        summary: fullcheck::cancel_summary(false, backed_up),
    });
    let armed = finish_prepare(&mut log, prepared, cancelled);
    let ArmedCard {
        disk,
        plan,
        backup,
        locks,
        mut device,
        cancel_path,
    } = armed;
    let _locks = locks;
    let request = ProbeRequest {
        disk: &disk,
        plan: &plan,
        backup: backup.as_ref(),
    };
    let job = FullRun {
        request: &request,
        cancel: &AtomicBool::new(false),
        cancel_file: cancel_path.as_deref(),
        chunk_bytes: fullcheck::DEFAULT_CHUNK,
    };
    let mut last_percent = None;
    match fullcheck::run_full_check(&mut device, &job, &mut |progress| {
        let Some(path) = &progress_file else {
            return;
        };
        let done = progress.bytes_done == progress.bytes_total;
        if last_percent == Some(progress.percent) && !done {
            return;
        }
        last_percent = Some(progress.percent);
        let line = fullcheck::format_progress(&progress);
        let _ = restore::write_nofollow(Path::new(path), line.as_bytes());
    }) {
        Ok(status) => {
            let code = match &status {
                FullStatus::Cancelled { .. } => 9,
                FullStatus::Finished(_) => 0,
            };
            for line in fullcheck::format_full_log(&status).lines() {
                log.line(line);
            }
            log.exit(code);
        }
        Err(probe::ProbeError::Blocked(block)) => probe_fail(&mut log, 3, false, block.message()),
        Err(probe::ProbeError::Failed {
            message,
            overwritten,
        }) => probe_fail(&mut log, 6, overwritten, &message),
    }
}

/// Identity comes from the backup's `.identity` file, not from the live disk.
fn recorded_backup(
    image: &str,
    sha: &str,
    prefix: &str,
    serial: &str,
) -> Result<SessionBackup, String> {
    let image = PathBuf::from(image);
    let stored = probe::load_identity(&image)?;
    let claimed = probe::CardIdentity {
        serial: serial.to_string(),
        prefix_sha256: prefix.to_string(),
        bytes: stored.bytes,
        model: stored.model.clone(),
        samples: stored.samples.clone(),
    };
    if !probe::same_identity(&stored, &claimed) {
        return Err(
            "the backup identity passed to the helper does not match the identity file. The card was not written."
                .into(),
        );
    }
    Ok(SessionBackup {
        image,
        size_bytes: stored.bytes,
        model: stored.model.clone(),
        sha256: sha.to_string(),
        identity: stored,
    })
}

fn read_serial(number: u32) -> Result<String, String> {
    let script = probe::disk_serial_script(number);
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|error| format!("could not read the disk serial: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not read the disk serial: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    Ok(probe::normalize_serial(line))
}

/// Turn a `Get-Volume` path into a `CreateFile` path, or refuse it.
fn volume_open_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        return Err("empty volume path".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("physicaldrive") || lower.contains("globalroot") {
        return Err(format!("refusing to lock {trimmed}"));
    }
    if let Some(index) = lower.find("volume{") {
        let rest = &trimmed[index..];
        if !rest.contains('}') {
            return Err(format!("refusing to lock {trimmed}"));
        }
        return Ok(format!(r"\\.\{rest}"));
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() == 6
        && (trimmed.starts_with(r"\\.\") || trimmed.starts_with(r"\\?/"))
        && bytes[4].is_ascii_alphabetic()
        && bytes[5] == b':'
    {
        return Ok(trimmed.to_string());
    }
    Err(format!(
        "refusing to lock an unexpected volume path {trimmed}"
    ))
}

fn lock_disk_volumes(number: u32) -> Result<Vec<File>, String> {
    #[cfg(windows)]
    {
        let paths = volume_paths(number)?;
        let mut files = Vec::new();
        for path in paths {
            let file = open_volume(&path)?;
            lock_and_dismount(&file)?;
            files.push(file);
        }
        Ok(files)
    }
    #[cfg(not(windows))]
    {
        let _ = number;
        // The Windows path is the only production caller. Keep the parser in
        // this build so the fixture tests can refuse physical-disk paths.
        let _ = volume_open_path;
        Err("volume lock is implemented for Windows".into())
    }
}

#[cfg(windows)]
fn volume_paths(number: u32) -> Result<Vec<String>, String> {
    let script = format!(
        "$ErrorActionPreference = 'Stop'; Get-Partition -DiskNumber {number} -ErrorAction SilentlyContinue | ForEach-Object {{ $v = $_ | Get-Volume -ErrorAction SilentlyContinue; if ($null -ne $v -and -not [string]::IsNullOrWhiteSpace([string]$v.Path)) {{ Write-Output ([string]$v.Path) }} }}"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|error| format!("could not list volumes: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not list volumes: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut paths = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        paths.push(volume_open_path(line)?);
    }
    Ok(paths)
}

#[cfg(windows)]
fn open_volume(path: &str) -> Result<File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
        .map_err(|error| format!("could not open {path} to lock it: {error}"))
}

#[cfg(windows)]
fn lock_and_dismount(file: &File) -> Result<(), String> {
    const FSCTL_LOCK_VOLUME: u32 = 0x0009_0018;
    const FSCTL_DISMOUNT_VOLUME: u32 = 0x0009_0020;
    ioctl(file, FSCTL_LOCK_VOLUME)?;
    ioctl(file, FSCTL_DISMOUNT_VOLUME)?;
    Ok(())
}

#[cfg(windows)]
fn ioctl(file: &File, code: u32) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn DeviceIoControl(
            handle: *mut core::ffi::c_void,
            code: u32,
            in_buf: *mut core::ffi::c_void,
            in_len: u32,
            out_buf: *mut core::ffi::c_void,
            out_len: u32,
            bytes: *mut u32,
            overlapped: *mut core::ffi::c_void,
        ) -> i32;
    }
    let mut bytes = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            0,
            &mut bytes,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(format!(
            "could not lock the volume ({code:#x}): {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
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
    fn probe_flag_is_its_own_mode() {
        let args = [
            "cfw-flash".to_string(),
            "--probe".to_string(),
            "--expected-bytes=4096".to_string(),
            "--expected-model=Samsung EVO".to_string(),
        ];
        assert!(args_request_probe(&args));
        assert!(!flag_present_in(&args, "restore"));
        assert!(!flag_present_in(&args, "image"));
        assert!(matches!(
            card_test_mode(&args),
            Ok(Some(CardTestMode::Probe))
        ));
    }

    #[test]
    fn full_check_flag_is_its_own_mode() {
        let args = vec![
            "cfw-flash".to_string(),
            "--full-check".to_string(),
            "--expected-bytes=4096".to_string(),
            "--expected-model=Samsung EVO".to_string(),
        ];
        assert!(args_request_full_check(&args));
        assert!(!args_request_probe(&args));
        assert!(matches!(
            card_test_mode(&args),
            Ok(Some(CardTestMode::FullCheck))
        ));
        let mut both = args.clone();
        both.push("--probe".to_string());
        assert!(card_test_mode(&both)
            .unwrap_err()
            .contains("cannot run together"));
        assert!(card_test_mode(&["cfw-flash".into(), "--image=os.img.gz".into()]).is_ok());
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

    /// Regular files only. Dropped at the end of the test.
    struct Tmp(std::path::PathBuf);

    impl Tmp {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cfw-flash-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            assert!(
                !is_physical_disk_path(&path),
                "helper tests must use a regular directory"
            );
            Self(path)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_card(
        dir: &std::path::Path,
        name: &str,
        bytes: &[u8],
    ) -> cfw_zero_touch_lib::block_device::FileBlockDevice {
        let path = dir.join(name);
        let len = bytes.len() as u64;
        let mut device =
            cfw_zero_touch_lib::block_device::FileBlockDevice::create(&path, len, len).unwrap();
        device.write_at(0, bytes).unwrap();
        device
    }

    #[test]
    fn unpartitioned_data_is_not_blank_and_an_all_zero_card_is() {
        let tmp = Tmp::new("blank");
        let mut data = vec![0u8; 8192];
        data[600] = 0xAB;
        let mut device = write_card(&tmp.0, "data.img", &data);
        assert!(
            !probe::card_is_blank(&mut device).unwrap(),
            "no partition table does not make leftover bytes blank"
        );
        let mut zeros = write_card(&tmp.0, "zero.img", &vec![0u8; 8192]);
        assert!(probe::card_is_blank(&mut zeros).unwrap());
    }

    #[test]
    fn a_backup_from_another_card_of_the_same_model_and_size_is_refused() {
        let tmp = Tmp::new("identity");
        let size = 8192usize;
        let model = "Generic SD";
        let serial = "reader-1";
        let mut card_a = vec![0u8; size];
        card_a[0] = 0x11;
        card_a[510] = 0x55;
        card_a[511] = 0xAA;
        let mut card_b = card_a.clone();
        card_b[0] = 0x22;

        let mut source = write_card(&tmp.0, "a.img", &card_a);
        let prefix = probe::prefix_sha256(&mut source).unwrap();
        let samples = probe::sample_block_hashes(&mut source).unwrap();
        let dest = tmp.0.join("a.img.gz");
        let outcome = backup::backup_image(
            &mut source,
            &dest,
            u64::MAX,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        let cfw_zero_touch_lib::backup::BackupOutcome::Complete {
            image,
            sha256,
            bytes,
            ..
        } = outcome
        else {
            panic!("backup fixture was cancelled");
        };
        let identity = probe::CardIdentity {
            serial: serial.into(),
            prefix_sha256: prefix,
            bytes,
            model: model.into(),
            samples,
        };
        probe::write_identity(&image, &identity).unwrap();
        let recorded = recorded_backup(
            image.to_str().unwrap(),
            &sha256,
            &identity.prefix_sha256,
            &identity.serial,
        )
        .unwrap();
        assert_eq!(recorded.size_bytes, size as u64);
        assert_eq!(recorded.model, model);

        let mut other_prefix_device = write_card(&tmp.0, "b-hash.img", &card_b);
        let other_prefix = probe::prefix_sha256(&mut other_prefix_device).unwrap();
        let mismatch = recorded_backup(
            image.to_str().unwrap(),
            &sha256,
            &other_prefix,
            &identity.serial,
        )
        .unwrap_err();
        assert!(mismatch.contains("identity"));

        let disk = ProbeDisk {
            number: 2,
            bus: "USB".into(),
            size_bytes: size as u64,
            system_disk: false,
            model: model.into(),
            serial: serial.into(),
        };
        let plan = ProbePlan {
            confirmation: "PROBE".into(),
            disk_number: 2,
            displayed_bytes: size as u64,
            displayed_model: model.into(),
        };
        let request = ProbeRequest {
            disk: &disk,
            plan: &plan,
            backup: Some(&recorded),
        };
        let b_path = tmp.0.join("b.img");
        {
            let mut live = write_card(&tmp.0, "b.img", &card_b);
            let error =
                probe::run_probe(&mut live, &request, &AtomicBool::new(false), None).unwrap_err();
            assert!(matches!(
                error,
                probe::ProbeError::Blocked(probe::ProbeBlock::Backup)
            ));
        }
        assert_eq!(
            std::fs::read(&b_path).unwrap(),
            card_b,
            "a same-model, same-size card with different contents must not be written"
        );
    }

    #[test]
    fn every_probe_exit_records_whether_the_card_was_overwritten() {
        let finished = probe::format_probe_log(&ProbeStatus::Finished(probe::ProbeReport {
            kind: probe::CardKind::Genuine,
            claimed_bytes: 8192,
            real_bytes: 8192,
            read_mbps: 1.0,
            write_mbps: 1.0,
            aliased: false,
            summary: "done".into(),
        }));
        assert!(finished.contains("OVERWRITTEN=yes"));
        assert!(finished.contains("RESULT=OK"));
        assert!(probe::overwritten_flag(&finished).unwrap());
        assert!(probe::parse_probe_log(&finished).unwrap().overwritten);

        let cancelled_yes = probe::format_probe_log(&ProbeStatus::Cancelled {
            overwritten: true,
            summary: probe::cancel_summary(true, true),
        });
        assert!(cancelled_yes.contains("OVERWRITTEN=yes"));
        assert!(cancelled_yes.contains("RESULT=CANCELLED"));
        let cancelled_no = probe::format_probe_log(&ProbeStatus::Cancelled {
            overwritten: false,
            summary: probe::cancel_summary(false, false),
        });
        assert!(cancelled_no.contains("OVERWRITTEN=no"));

        assert!(probe::format_failed_log(true, "write failed").contains("OVERWRITTEN=yes"));
        let refused = probe::format_failed_log(false, "refused before a write");
        assert!(refused.contains("OVERWRITTEN=no"));
        assert!(refused.contains("RESULT=FAIL"));

        let missing = "RESULT=CANCELLED\nSUMMARY=Cancelled.\n";
        assert!(probe::overwritten_flag(missing)
            .unwrap_err()
            .contains("missing"));
        assert!(probe::parse_probe_log(missing).is_err());
        let error = probe::interpret_helper_log(Some(9), missing, true, true).unwrap_err();
        assert!(error.contains("did not say whether"));
        assert!(!error.contains("Nothing on the card was overwritten"));
    }

    #[test]
    fn volume_lock_paths_are_volume_paths_and_a_lock_failure_writes_nothing() {
        assert_eq!(
            volume_open_path(r"\\?\Volume{12345678-abcd-ef}\").unwrap(),
            r"\\.\Volume{12345678-abcd-ef}"
        );
        assert_eq!(volume_open_path(r"\\.\E:").unwrap(), r"\\.\E:");
        assert!(volume_open_path(r"\\.\PhysicalDrive2").is_err());
        assert!(volume_open_path(r"\\.\GLOBALROOT\device\harddisk2").is_err());
        let message = probe::backup_write_allowed(Err("busy".into())).unwrap_err();
        assert!(message.contains("nothing was written"));
        assert!(probe::format_failed_log(false, &message).contains("OVERWRITTEN=no"));
    }
}
