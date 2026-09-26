use std::fs::File;
use std::io::{Read, Write};
use std::process::Command;

use cfw_zero_touch_lib::flash::{
    authorize_flash, sha256_prefix, sha256_reader, write_gzip, FlashDisk, FlashPlan, SectorWriter,
};

fn arg(name: &str) -> String {
    let prefix = format!("--{name}=");
    std::env::args()
        .find_map(|item| item.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("missing --{name}"))
}

fn main() {
    let image = arg("image");
    let expected = arg("sha256");
    let disk_number: u32 = arg("disk").parse().expect("disk number");
    let confirmation = arg("confirm");

    let disk = read_disk(disk_number).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    println!(
        "target disk {} {} {} bytes ({:.2} GB)",
        disk.number,
        disk.bus,
        disk.size_bytes,
        disk.size_bytes as f64 / 1_000_000_000.0
    );

    let actual = sha256_reader(File::open(&image).expect("open image")).expect("hash image");
    let plan = FlashPlan {
        confirmation,
        disk_number,
        displayed_bytes: disk.size_bytes,
        expected_sha256: expected,
        actual_sha256: actual,
    };
    if let Err(block) = authorize_flash(&disk, &plan) {
        eprintln!("flash blocked: {block:?}");
        std::process::exit(3);
    }

    if disk_has_label(disk_number, "EASYROMS") {
        println!("card is labeled EASYROMS; FLASH confirmation is erasing it");
    }

    clear_disk(disk_number).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(5);
    });

    let written = write_physical(disk_number, &image).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(6);
    });
    let card_hash = hash_physical(disk_number, written).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(7);
    });
    let image_hash = if image.ends_with(".gz") {
        sha256_prefix(
            flate2::read::GzDecoder::new(File::open(&image).expect("reopen image")),
            written,
        )
        .expect("hash decompressed image")
    } else {
        sha256_prefix(File::open(&image).expect("reopen image"), written).expect("hash image bytes")
    };
    println!("wrote {written} bytes");
    println!("image {image_hash}");
    println!("card  {card_hash}");
    if image_hash != card_hash {
        eprintln!("card does not match the image");
        std::process::exit(8);
    }
    println!("RESULT=OK");
}

fn read_disk(number: u32) -> Result<FlashDisk, String> {
    let script = format!(
        "$d = Get-Disk -Number {number}; Write-Output ($d.Number.ToString() + '|' + $d.BusType + '|' + $d.Size)"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|error| format!("could not inspect the disk: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| line.contains('|')).ok_or("no disk info")?;
    let mut parts = line.trim().split('|');
    let number: u32 = parts
        .next()
        .unwrap()
        .parse()
        .map_err(|_| format!("bad disk line {line}"))?;
    let bus = parts.next().unwrap_or("").to_string();
    let size_bytes: u64 = parts
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|_| format!("bad disk size {line}"))?;
    Ok(FlashDisk {
        number,
        bus,
        size_bytes,
        system_disk: number == 0,
    })
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

fn write_physical(number: u32, image: &str) -> Result<u64, String> {
    let mut output = SectorWriter::new(open_drive(number)?);
    let mut input = File::open(image).map_err(|error| error.to_string())?;
    let written = if image.ends_with(".gz") {
        write_gzip(input, &mut output)?
    } else {
        write_raw(&mut input, &mut output)?
    };
    output
        .flush()
        .map_err(|error| format!("could not flush the image: {error}"))?;
    Ok(written)
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
