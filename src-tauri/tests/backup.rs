use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cfw_zero_touch_lib::backup::{
    authorize_backup, backup_image, bytes_required, check_destination, directory_free_bytes,
    parse_disk_query, partial_path, sidecar_path, source_volume, BackupBlock, BackupDisk,
    BackupError, BackupOutcome,
};
use cfw_zero_touch_lib::block_device::{FakeBlockDevice, FileBlockDevice};
use cfw_zero_touch_lib::flash::sha256_bytes;
use cfw_zero_touch_lib::format_gate::{drive_is_usable, FormatBlock};
use cfw_zero_touch_lib::volume::{decide, VolumeDecision};

#[allow(dead_code, unused_imports)]
#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::{build_card, CardOptions, Scratch};

struct CountWrites<D> {
    inner: D,
    writes: u32,
}

impl<D: FakeBlockDevice> FakeBlockDevice for CountWrites<D> {
    fn reported_len(&self) -> u64 {
        self.inner.reported_len()
    }

    fn stored_len(&self) -> u64 {
        self.inner.stored_len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_at(offset, buf)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.writes += 1;
        self.inner.write_at(offset, data)
    }
}

fn disk(number: u32, bus: &str, size: u64, system: bool) -> BackupDisk {
    BackupDisk {
        number,
        bus: bus.into(),
        size_bytes: size,
        system_disk: system,
        displayed_bytes: size,
    }
}

fn stage_card(label: &str) -> (Scratch, PathBuf, Vec<u8>) {
    let card = build_card(CardOptions::armed_original());
    let scratch = Scratch::new(label);
    let path = scratch.path().join("card.img");
    let mut device = FileBlockDevice::create(&path, card.stored_len(), card.reported_len).unwrap();
    device.write_at(0, &card.bytes).unwrap();
    drop(device);
    (scratch, path, card.bytes)
}

#[test]
fn read_only_open_rejects_writes_and_backup_does_not_attempt_them() {
    let (_scratch, path, bytes) = stage_card("backup-ro");
    let before = std::fs::read(&path).unwrap();
    assert_eq!(before, bytes);

    let mut opened = FileBlockDevice::open_read_only(&path).unwrap();
    assert!(opened.write_at(0, &[0xA5; 32]).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let mut device = CountWrites {
        inner: FileBlockDevice::open_read_only(&path).unwrap(),
        writes: 0,
    };
    let destination = path.parent().unwrap().join("stock.img.gz");
    let free = bytes_required(device.stored_len());
    let outcome = backup_image(
        &mut device,
        &destination,
        free,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert!(matches!(outcome, BackupOutcome::Complete { .. }));
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let error = backup_image(
        &mut device,
        Path::new("/dev/mmcblk0"),
        u64::MAX,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    match error {
        BackupError::Destination(message) => {
            assert!(message.to_lowercase().contains("physical"));
        }
        other => panic!("expected a physical-path refusal, got {other:?}"),
    }
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn backup_refuses_system_and_non_removable_disks_through_the_shared_gates() {
    for refused in [
        disk(0, "USB", 64 * 1024, true),
        disk(0, "USB", 64 * 1024, false),
        disk(1, "NVMe", 64 * 1024, false),
        disk(1, "SATA", 64 * 1024, true),
    ] {
        let volume = source_volume(&refused);
        assert!(
            matches!(decide(&volume, ""), VolumeDecision::Rejected { .. }),
            "volume::decide should reject {refused:?}"
        );
        assert_eq!(drive_is_usable(&volume), Err(FormatBlock::NotRemovable));
        assert_eq!(authorize_backup(&refused), Err(BackupBlock::NotRemovable));
    }

    let mut zero = disk(2, "USB", 0, false);
    zero.displayed_bytes = 0;
    let volume = source_volume(&zero);
    assert!(
        !matches!(decide(&volume, ""), VolumeDecision::Rejected { .. }),
        "size 0 is a capacity refusal, not the removable-drive refusal"
    );
    assert_eq!(drive_is_usable(&volume), Err(FormatBlock::Size));
    assert_eq!(authorize_backup(&zero), Err(BackupBlock::Size));

    let mut changed = disk(2, "usb", 32 * 1024 * 1024, false);
    changed.displayed_bytes = 1;
    let volume = source_volume(&changed);
    assert!(drive_is_usable(&volume).is_ok());
    assert_eq!(authorize_backup(&changed), Err(BackupBlock::Size));

    assert!(authorize_backup(&disk(2, "USB", 32 * 1024 * 1024, false)).is_ok());
}

#[test]
fn backup_aborts_when_destination_free_space_is_short() {
    let (_scratch, path, bytes) = stage_card("backup-space");
    let mut device = CountWrites {
        inner: FileBlockDevice::open_read_only(&path).unwrap(),
        writes: 0,
    };
    let destination = path.parent().unwrap().join("stock.img.gz");
    let need = bytes_required(bytes.len() as u64);
    let error = backup_image(
        &mut device,
        &destination,
        need - 1,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    match error {
        BackupError::InsufficientSpace { need: got, free } => {
            assert_eq!(got, need);
            assert_eq!(free, need - 1);
        }
        other => panic!("expected a clean space abort, got {other:?}"),
    }
    assert!(!destination.exists());
    assert!(!partial_path(&destination).exists());
    assert!(!sidecar_path(&destination).exists());
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn cancel_leaves_partial_and_does_not_publish_a_finished_image() {
    let (_scratch, path, bytes) = stage_card("backup-cancel");
    let mut device = CountWrites {
        inner: FileBlockDevice::open_read_only(&path).unwrap(),
        writes: 0,
    };
    let destination = path.parent().unwrap().join("stock.img.gz");
    let cancel = AtomicBool::new(false);
    let free = bytes_required(device.stored_len());
    let outcome = backup_image(&mut device, &destination, free, &cancel, |_| {
        cancel.store(true, Ordering::Release)
    })
    .unwrap();
    let BackupOutcome::Cancelled { partial } = outcome else {
        panic!("cancel must not finish the image");
    };
    assert_eq!(partial, partial_path(&destination));
    assert!(partial.is_file());
    assert!(partial
        .file_name()
        .unwrap()
        .to_string_lossy()
        .ends_with(".partial"));
    assert!(!destination.exists());
    assert!(!sidecar_path(&destination).exists());
    assert!(std::fs::metadata(&partial).unwrap().len() > 0);
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    let partial_bytes = std::fs::read(&partial).unwrap();
    let mut restored = Vec::new();
    let decoded = flate2::read::GzDecoder::new(partial_bytes.as_slice()).read_to_end(&mut restored);
    assert!(
        decoded.is_err(),
        "cancelled partial decompressed to {} bytes",
        restored.len()
    );
}

#[test]
fn armed_original_fixture_round_trips_through_gzip_and_sha256() {
    let (_scratch, path, bytes) = stage_card("backup-round");
    let mut device = CountWrites {
        inner: FileBlockDevice::open_read_only(&path).unwrap(),
        writes: 0,
    };
    assert_eq!(device.stored_len(), bytes.len() as u64);
    assert_eq!(device.reported_len(), bytes.len() as u64);
    let destination = path.parent().unwrap().join("r36s-stock.img.gz");
    let mut last = 0u64;
    let outcome = backup_image(
        &mut device,
        &destination,
        directory_free_bytes(destination.parent().unwrap()).unwrap(),
        &AtomicBool::new(false),
        |progress| {
            assert!(progress.bytes_read >= last);
            assert!(progress.bytes_read <= progress.bytes_total);
            last = progress.bytes_read;
        },
    )
    .unwrap();
    let BackupOutcome::Complete {
        image,
        sidecar,
        sha256,
        bytes: copied,
    } = outcome
    else {
        panic!("expected a finished backup");
    };
    assert_eq!(copied, bytes.len() as u64);
    assert_eq!(image, destination);
    assert_eq!(sha256, sha256_bytes(&bytes));
    assert_eq!(
        std::fs::read_to_string(&sidecar).unwrap().trim(),
        sha256.as_str()
    );
    assert_eq!(sidecar, sidecar_path(&destination));
    assert!(!partial_path(&destination).exists());

    let compressed = std::fs::read(&image).unwrap();
    let mut restored = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut restored)
        .unwrap();
    assert_eq!(restored, bytes);
    assert_eq!(sha256_bytes(&restored), sha256);
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(last, bytes.len() as u64);
}

#[cfg(unix)]
#[test]
fn backup_refuses_to_follow_a_symlinked_partial_file() {
    let (_scratch, path, bytes) = stage_card("backup-link");
    let dir = path.parent().unwrap();
    let destination = dir.join("stock.img.gz");
    let partial = partial_path(&destination);
    let real = dir.join("real-partial.bin");
    std::fs::write(&real, b"keep").unwrap();
    std::os::unix::fs::symlink(&real, &partial).unwrap();
    let mut device = CountWrites {
        inner: FileBlockDevice::open_read_only(&path).unwrap(),
        writes: 0,
    };
    let free = bytes_required(bytes.len() as u64);
    let error = backup_image(
        &mut device,
        &destination,
        free,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(error, BackupError::Destination(_)));
    assert_eq!(std::fs::read(&real).unwrap(), b"keep");
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!destination.exists());
}

#[test]
fn verbatim_drive_letter_is_not_a_physical_backup_destination() {
    let path = Path::new(r"\\?\C:\Users\cfw\stock.img.gz");
    assert!(!cfw_zero_touch_lib::block_device::is_physical_disk_path(
        path
    ));
    if let Err(error) = check_destination(path) {
        let message = error.to_string().to_lowercase();
        assert!(
            !message.contains("physical"),
            "drive-letter verbatim path was treated as a disk: {message}"
        );
    }
}

#[test]
fn device_paths_are_refused_before_any_filesystem_call() {
    for path in [
        r"\\.\PhysicalDrive0",
        r"\\?\PhysicalDrive1",
        r"\\?\GLOBALROOT\Device\Harddisk0\DR0",
        r"\\.\GLOBALROOT\Device\Harddisk0\DR0",
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\card.img.gz",
        r"\\.\C:",
    ] {
        let error = check_destination(Path::new(path)).unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("physical"),
            "{path} -> {error}"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_canonical_drive_destination_is_accepted() {
    let dir = std::env::temp_dir().join(format!(
        "cfw-backup-dest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("stock.img.gz");
    let canonical = std::fs::canonicalize(&dir).unwrap();
    let display = canonical.display().to_string();
    let bare = display.strip_prefix(r"\\?\").unwrap_or(display.as_str());
    let verbatim = format!(r"\\?\{bare}");
    assert!(
        !cfw_zero_touch_lib::block_device::is_physical_disk_path(std::path::Path::new(&verbatim)),
        "{verbatim}"
    );
    assert!(
        !cfw_zero_touch_lib::block_device::is_physical_disk_path(&canonical),
        "canonical path treated as a disk: {display}"
    );
    match check_destination(&dest) {
        Ok(()) => {}
        Err(error) => panic!("C: destination refused: {error}"),
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn windows_device_paths_are_still_refused() {
    for path in [
        r"\\.\PhysicalDrive0",
        r"\\?\PhysicalDrive1",
        r"\\?\GLOBALROOT\Device\Harddisk0\Partition0\card.img.gz",
        r"\\.\C:",
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\card.img.gz",
    ] {
        let error = check_destination(Path::new(path)).unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("physical"),
            "{path} -> {error}"
        );
    }
}

#[test]
fn get_disk_system_or_boot_flags_refuse_the_backup() {
    let system = parse_disk_query("3|USB|33554432|True|False", 3).unwrap();
    assert!(system.system_disk);
    assert_eq!(
        authorize_backup(&BackupDisk {
            number: system.number,
            bus: system.bus.clone(),
            size_bytes: system.size_bytes,
            system_disk: system.system_disk,
            displayed_bytes: system.size_bytes,
        }),
        Err(BackupBlock::NotRemovable)
    );

    let boot = parse_disk_query("4|USB|33554432|False|True", 4).unwrap();
    assert!(boot.system_disk);
    assert_eq!(
        authorize_backup(&BackupDisk {
            number: boot.number,
            bus: boot.bus,
            size_bytes: boot.size_bytes,
            system_disk: boot.system_disk,
            displayed_bytes: boot.size_bytes,
        }),
        Err(BackupBlock::NotRemovable)
    );

    let normal = parse_disk_query("5|USB|33554432|False|False", 5).unwrap();
    assert!(!normal.system_disk);
    assert_eq!(normal.number, 5);
    assert_eq!(normal.size_bytes, 33_554_432);
    assert!(authorize_backup(&BackupDisk {
        number: normal.number,
        bus: normal.bus,
        size_bytes: normal.size_bytes,
        system_disk: normal.system_disk,
        displayed_bytes: normal.size_bytes,
    })
    .is_ok());

    assert!(parse_disk_query("9|USB|1|False|False", 3).is_err());
}

#[test]
fn temp_directory_free_space_is_queried_without_opening_a_disk() {
    let scratch = Scratch::new("backup-free");
    let free = directory_free_bytes(scratch.path()).unwrap();
    assert!(free > 0);
}
