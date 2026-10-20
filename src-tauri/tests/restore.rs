use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cfw_zero_touch_lib::backup::{backup_image, directory_free_bytes, sidecar_path};
use cfw_zero_touch_lib::block_device::{FakeBlockDevice, FileBlockDevice};
use cfw_zero_touch_lib::flash::sha256_bytes;
use cfw_zero_touch_lib::restore::{
    cancel_detail, cancel_file_requests_stop, fail_detail, gate_restore, image_fits,
    inspect_backup, judge_helper_log, parse_helper_log, parse_progress_line, parse_restore_disk,
    parse_sidecar, pass_detail, restore_to_device, HelperVerdict, InspectedBackup, RestoreBlock,
    RestoreDisk, RestoreError, RestoreOutcome, RestoreTarget,
};

#[allow(dead_code, unused_imports)]
#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::{build_card, CardOptions, Scratch};
use flate2::write::GzEncoder;
use flate2::Compression;

const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

struct Record<D> {
    inner: D,
    writes: Vec<usize>,
}

impl<D: FakeBlockDevice> FakeBlockDevice for Record<D> {
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
        self.writes.push(data.len());
        self.inner.write_at(offset, data)
    }
}

fn disk(number: u32, bus: &str, size: u64, system: bool, model: &str) -> RestoreDisk {
    RestoreDisk {
        number,
        bus: bus.into(),
        size_bytes: size,
        system_disk: system,
        model: model.into(),
    }
}

fn target<'a>(
    disk: &'a RestoreDisk,
    confirm: &'a str,
    bytes: u64,
    model: &'a str,
    number: u32,
) -> RestoreTarget<'a> {
    RestoreTarget {
        disk,
        confirmed_number: number,
        confirmation: confirm,
        displayed_bytes: bytes,
        displayed_model: model,
    }
}

fn inspected(bytes: u64) -> InspectedBackup {
    InspectedBackup {
        image: PathBuf::from("stock.img.gz"),
        sidecar: PathBuf::from("stock.img.gz.sha256"),
        sha256: DIGEST.into(),
        compressed_sha256: DIGEST.into(),
        image_bytes: bytes,
    }
}

fn gzip_bytes(payload: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(payload).unwrap();
    encoder.finish().unwrap()
}

fn write_backup(dir: &Path, name: &str, payload: &[u8]) -> PathBuf {
    let image = dir.join(name);
    std::fs::write(&image, gzip_bytes(payload)).unwrap();
    std::fs::write(sidecar_path(&image), format!("{}\n", sha256_bytes(payload))).unwrap();
    image
}

fn poisoned(dir: &Path, name: &str, stored: u64, reported: u64) -> Record<FileBlockDevice> {
    let mut device = FileBlockDevice::create(&dir.join(name), stored, reported).unwrap();
    device.write_at(0, &[0xA5; 32]).unwrap();
    Record {
        inner: device,
        writes: Vec::new(),
    }
}

fn still_poison(device: &mut FileBlockDevice) {
    let mut buf = [0u8; 32];
    device.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, [0xA5; 32]);
}

fn blocked(error: RestoreError) -> RestoreBlock {
    match error {
        RestoreError::Blocked(block) => block,
        other => panic!("expected a refusal, got {other}"),
    }
}

#[test]
fn restore_writes_through_the_flash_sector_writer() {
    let scratch = Scratch::new("restore-sector");
    let payload = b"rgb10x-unaligned-image";
    assert!(!payload.len().is_multiple_of(512));
    let image = write_backup(scratch.path(), "stock.img.gz", payload);
    let record = poisoned(scratch.path(), "dest.img", 4096, 4096);
    let live = disk(2, "USB", 4096, false, "Test SD");
    let (mut record, result) = restore_to_device(
        &image,
        record,
        &target(&live, "FLASH", 4096, "Test SD", 2),
        &AtomicBool::new(false),
        |_| {},
    );
    let outcome = result.unwrap();
    assert_eq!(outcome.status(), "pass");
    assert!(outcome.detail().contains("Read-back verify passed"));
    assert!(record.writes.iter().all(|len| len.is_multiple_of(512)));
    assert!(!record.writes.is_empty());
    let mut got = vec![0u8; 512];
    record.inner.read_at(0, &mut got).unwrap();
    assert_eq!(&got[..payload.len()], payload);
    assert!(got[payload.len()..].iter().all(|byte| *byte == 0));
    assert_eq!(
        match outcome {
            RestoreOutcome::Passed { sha256, .. } => sha256,
            other => panic!("expected pass, got {other:?}"),
        },
        sha256_bytes(payload)
    );
}

#[test]
fn flash_identity_guard_refuses_before_any_write() {
    let scratch = Scratch::new("restore-identity");
    let image = write_backup(scratch.path(), "stock.img.gz", b"card-bytes");
    let record = poisoned(scratch.path(), "dest.img", 4096, 4096);
    let live = disk(2, "USB", 4096, false, "Test SD");
    let (mut record, result) = restore_to_device(
        &image,
        record,
        &target(&live, "FLASH", 4096, "Test SD", 9),
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(blocked(result.unwrap_err()), RestoreBlock::Identity);
    assert!(record.writes.is_empty());
    still_poison(&mut record.inner);
}

#[test]
fn partial_missing_and_mismatched_sidecars_refuse_before_write() {
    let scratch = Scratch::new("restore-sidecar");
    let live = disk(2, "USB", 4096, false, "Test SD");
    let plan = target(&live, "FLASH", 4096, "Test SD", 2);

    let partial = scratch.path().join("stock.img.gz.partial");
    let record = poisoned(scratch.path(), "partial-dest.img", 4096, 4096);
    let (mut record, result) =
        restore_to_device(&partial, record, &plan, &AtomicBool::new(false), |_| {});
    let error = result.unwrap_err().to_string();
    assert!(error.to_lowercase().contains("partial"), "{error}");
    assert!(record.writes.is_empty());
    still_poison(&mut record.inner);

    let image = scratch.path().join("noside.img.gz");
    std::fs::write(&image, b"not-a-real-gzip").unwrap();
    let record = poisoned(scratch.path(), "missing-dest.img", 4096, 4096);
    let (mut record, result) =
        restore_to_device(&image, record, &plan, &AtomicBool::new(false), |_| {});
    let error = result.unwrap_err().to_string();
    assert!(error.to_lowercase().contains("missing"), "{error}");
    assert!(record.writes.is_empty());
    still_poison(&mut record.inner);

    let image = write_backup(scratch.path(), "good.img.gz", b"real-bytes");
    std::fs::write(sidecar_path(&image), format!("{DIGEST}\n")).unwrap();
    let record = poisoned(scratch.path(), "bad-hash-dest.img", 4096, 4096);
    let (mut record, result) =
        restore_to_device(&image, record, &plan, &AtomicBool::new(false), |_| {});
    let error = result.unwrap_err().to_string();
    assert!(error.to_lowercase().contains("sidecar"), "{error}");
    assert!(record.writes.is_empty());
    still_poison(&mut record.inner);
}

#[test]
fn image_larger_than_the_card_is_refused() {
    let scratch = Scratch::new("restore-large");
    let payload = vec![0x22; 2048];
    let image = write_backup(scratch.path(), "stock.img.gz", &payload);
    let live = disk(2, "USB", 1024, false, "Test SD");
    let record = poisoned(scratch.path(), "dest.img", 4096, 4096);
    let (mut record, result) = restore_to_device(
        &image,
        record,
        &target(&live, "FLASH", 1024, "Test SD", 2),
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(blocked(result.unwrap_err()), RestoreBlock::TooLarge);
    assert!(record.writes.is_empty());
    still_poison(&mut record.inner);
    assert!(!image_fits(2048, 1024));
    assert!(image_fits(1024, 1024));
    assert!(!image_fits(0, 1024));
}

#[test]
fn system_boot_and_non_removable_disks_are_refused() {
    let scratch = Scratch::new("restore-guard");
    let cases = [
        disk(0, "USB", 4096, true, "System"),
        disk(0, "USB", 4096, false, "Disk Zero"),
        disk(1, "NVMe", 4096, false, "Internal"),
        disk(3, "USB", 4096, true, "Boot USB"),
    ];
    for live in cases {
        let record = poisoned(
            scratch.path(),
            &format!("dest-{}.img", live.number),
            4096,
            4096,
        );
        let (mut record, result) = restore_to_device(
            &scratch.path().join("missing.img.gz"),
            record,
            &target(&live, "FLASH", live.size_bytes, &live.model, live.number),
            &AtomicBool::new(false),
            |_| {},
        );
        assert_eq!(
            blocked(result.unwrap_err()),
            RestoreBlock::NotRemovable,
            "{live:?}"
        );
        assert!(record.writes.is_empty());
        still_poison(&mut record.inner);
    }

    let system = parse_restore_disk("3|USB|4096|True|False|Sandisk Ultra", 3).unwrap();
    assert!(system.system_disk);
    assert_eq!(system.model, "Sandisk Ultra");
    assert_eq!(
        blocked(
            gate_restore(
                &inspected(100),
                &target(&system, "FLASH", 4096, "Sandisk Ultra", 3)
            )
            .unwrap_err()
        ),
        RestoreBlock::NotRemovable
    );
    let boot = parse_restore_disk("4|USB|4096|False|True|Generic USB", 4).unwrap();
    assert!(boot.system_disk);
    assert_eq!(
        blocked(
            gate_restore(
                &inspected(100),
                &target(&boot, "FLASH", 4096, "Generic USB", 4)
            )
            .unwrap_err()
        ),
        RestoreBlock::NotRemovable
    );

    let mut zero = disk(2, "USB", 0, false, "Empty");
    zero.model = "Empty".into();
    assert_eq!(
        blocked(gate_restore(&inspected(1), &target(&zero, "FLASH", 0, "Empty", 2)).unwrap_err()),
        RestoreBlock::Size
    );
}

#[test]
fn readback_failure_is_reported_and_is_not_a_pass() {
    let scratch = Scratch::new("restore-fail");
    // A uniform payload survives wrap-around and would hash as a match.
    let mut payload = vec![0x11; 1024];
    payload.extend(std::iter::repeat_n(0x22, 1024));
    let image = write_backup(scratch.path(), "stock.img.gz", &payload);
    let mut device = FileBlockDevice::create(&scratch.path().join("wrap.img"), 1024, 4096).unwrap();
    device.write_at(0, &[0xA5; 32]).unwrap();
    let live = disk(2, "USB", 4096, false, "Wrap Card");
    let (mut device, result) = restore_to_device(
        &image,
        device,
        &target(&live, "FLASH", 4096, "Wrap Card", 2),
        &AtomicBool::new(false),
        |_| {},
    );
    let outcome = result.unwrap();
    assert_eq!(outcome.status(), "fail");
    assert!(outcome.detail().contains("Read-back verify failed"));
    assert!(outcome.detail().contains("does not match"));
    let RestoreOutcome::Failed {
        expected, actual, ..
    } = outcome
    else {
        panic!("expected a failed read-back");
    };
    assert_eq!(expected, sha256_bytes(&payload));
    assert_ne!(actual, expected);
    assert!(!actual.is_empty());
    let mut got = vec![0u8; payload.len()];
    device.read_at(0, &mut got).unwrap();
    assert_ne!(got, payload);
}

#[test]
fn fixture_backup_restores_byte_identical_on_a_second_card() {
    let card = build_card(CardOptions::armed_original());
    let scratch = Scratch::new("restore-round");
    let source_path = scratch.path().join("source.img");
    let mut source =
        FileBlockDevice::create(&source_path, card.stored_len(), card.reported_len).unwrap();
    source.write_at(0, &card.bytes).unwrap();
    let destination = scratch.path().join("r36s-stock.img.gz");
    let outcome = backup_image(
        &mut source,
        &destination,
        directory_free_bytes(scratch.path()).unwrap(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let cfw_zero_touch_lib::backup::BackupOutcome::Complete { sha256, bytes, .. } = outcome else {
        panic!("U3 backup did not finish");
    };
    assert_eq!(bytes, card.bytes.len() as u64);
    assert_eq!(sha256, sha256_bytes(&card.bytes));

    let inspected = inspect_backup(&destination, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(inspected.sha256, sha256);
    assert_eq!(
        inspected.compressed_sha256,
        sha256_bytes(&std::fs::read(&destination).unwrap())
    );
    assert_eq!(
        std::fs::read_to_string(sidecar_path(&destination))
            .unwrap()
            .trim(),
        sha256
    );

    let mut blank = FileBlockDevice::create(
        &scratch.path().join("blank.img"),
        card.stored_len(),
        card.reported_len,
    )
    .unwrap();
    blank.write_at(0, &[0xA5; 32]).unwrap();
    let record = Record {
        inner: blank,
        writes: Vec::new(),
    };
    let live = disk(2, "USB", card.reported_len, false, "Fixture SD");
    let mut saw_verify = false;
    let mut saw_write = false;
    let mut saw_readback = false;
    let (mut record, result) = restore_to_device(
        &destination,
        record,
        &target(&live, "FLASH", card.reported_len, "Fixture SD", 2),
        &AtomicBool::new(false),
        |progress| {
            assert!(progress.bytes_done <= progress.bytes_total || progress.bytes_total == 0);
            match progress.phase.as_str() {
                "verify" => saw_verify = true,
                "write" => saw_write = true,
                "readback" => saw_readback = true,
                other => panic!("unexpected phase {other}"),
            }
        },
    );
    let outcome = result.unwrap();
    assert_eq!(outcome.status(), "pass");
    assert_eq!(outcome.detail(), pass_detail(&sha256));
    assert!(saw_verify && saw_write && saw_readback);
    assert!(record.writes.iter().all(|len| len.is_multiple_of(512)));
    let mut got = vec![0u8; card.bytes.len()];
    record.inner.read_at(0, &mut got).unwrap();
    assert_eq!(got, card.bytes);
    assert_eq!(std::fs::read(&source_path).unwrap(), card.bytes);
}

#[test]
fn confirmation_must_match_the_shown_disk_size_and_model() {
    let scratch = Scratch::new("restore-confirm");
    let image = write_backup(scratch.path(), "stock.img.gz", b"shown-disk");
    let live = disk(2, "USB", 4096, false, "Samsung EVO");

    for (confirm, bytes, model, number, block) in [
        (
            "flash",
            4096u64,
            "Samsung EVO",
            2u32,
            RestoreBlock::Confirmation,
        ),
        ("FORMAT", 4096, "Samsung EVO", 2, RestoreBlock::Confirmation),
        ("FLASH", 4095, "Samsung EVO", 2, RestoreBlock::Size),
        ("FLASH", 4096, "Other card", 2, RestoreBlock::Model),
        ("FLASH", 4096, "", 2, RestoreBlock::Model),
        ("FLASH", 4096, "Samsung EVO", 8, RestoreBlock::Identity),
    ] {
        let record = poisoned(
            scratch.path(),
            &format!("dest-{confirm}-{bytes}-{number}.img"),
            4096,
            4096,
        );
        let (mut record, result) = restore_to_device(
            &image,
            record,
            &target(&live, confirm, bytes, model, number),
            &AtomicBool::new(false),
            |_| {},
        );
        assert_eq!(blocked(result.unwrap_err()), block, "{confirm} {model}");
        assert!(record.writes.is_empty(), "{block:?}");
        still_poison(&mut record.inner);
    }

    let slashed = disk(2, "USB", 4096, false, "A/B");
    assert!(
        gate_restore(&inspected(100), &target(&slashed, "FLASH", 4096, "A|B", 2)).is_ok(),
        "a pipe in the shown model is the slash Get-Disk stores"
    );
}

#[test]
fn cancel_before_write_leaves_the_card_untouched() {
    let scratch = Scratch::new("restore-cancel-early");
    let image = write_backup(scratch.path(), "stock.img.gz", b"do-not-write");
    let record = poisoned(scratch.path(), "dest.img", 4096, 4096);
    let live = disk(2, "USB", 4096, false, "Test SD");
    let cancel = AtomicBool::new(false);
    let (mut record, result) = restore_to_device(
        &image,
        record,
        &target(&live, "FLASH", 4096, "Test SD", 2),
        &cancel,
        |_| cancel.store(true, Ordering::Release),
    );
    let outcome = result.unwrap();
    assert_eq!(outcome.status(), "cancelled");
    assert_eq!(outcome.detail(), cancel_detail(0));
    assert!(record.writes.is_empty());
    still_poison(&mut record.inner);
}

#[test]
fn cancel_during_write_does_not_report_a_pass() {
    let scratch = Scratch::new("restore-cancel-write");
    let payload = vec![0x5A; (1024 * 1024) + 100];
    let image = write_backup(scratch.path(), "stock.img.gz", &payload);
    let stored = payload.len() as u64;
    let record = poisoned(scratch.path(), "dest.img", stored, stored);
    let live = disk(2, "USB", stored, false, "Test SD");
    let cancel = AtomicBool::new(false);
    let (record, result) = restore_to_device(
        &image,
        record,
        &target(&live, "FLASH", stored, "Test SD", 2),
        &cancel,
        |progress| {
            if progress.phase == "write" && progress.bytes_done > 0 {
                cancel.store(true, Ordering::Release);
            }
        },
    );
    let outcome = result.unwrap();
    assert_eq!(outcome.status(), "cancelled");
    assert!(outcome.detail().contains("Cancelled"));
    assert_ne!(outcome.status(), "pass");
    let RestoreOutcome::Cancelled { bytes_written, .. } = outcome else {
        panic!("expected cancel");
    };
    assert!(bytes_written > 0);
    assert!(bytes_written < stored);
    assert!(!record.writes.is_empty());
}

#[test]
fn helper_log_distinguishes_pass_fail_and_an_os_flash_ok() {
    let sidecar = DIGEST;
    let pass_log =
        format!("wrote 100 bytes\nimage {sidecar}\ncard  {sidecar}\nverify=pass\nRESULT=OK\n");
    let verdict = judge_helper_log(&pass_log, sidecar);
    match verdict {
        HelperVerdict::Passed {
            sha256,
            bytes,
            detail,
        } => {
            assert_eq!(sha256, sidecar);
            assert_eq!(bytes, 100);
            assert_eq!(detail, pass_detail(sidecar));
        }
        other => panic!("expected pass, got {other:?}"),
    }

    let other = "a".repeat(64);
    let fail_log = format!(
        "wrote 100 bytes\ncard  {other}\nverify=fail\ncard does not match the backup checksum\n"
    );
    match judge_helper_log(&fail_log, sidecar) {
        HelperVerdict::Failed {
            expected,
            actual,
            detail,
            ..
        } => {
            assert_eq!(expected, sidecar);
            assert_eq!(actual, other);
            assert_eq!(detail, fail_detail(sidecar, &other));
            assert!(detail.contains("Read-back verify failed"));
        }
        other => panic!("expected fail, got {other:?}"),
    }

    let flash_ok = format!("wrote 100 bytes\nimage {sidecar}\ncard  {sidecar}\nRESULT=OK\n");
    assert!(matches!(
        judge_helper_log(&flash_ok, sidecar),
        HelperVerdict::Incomplete { .. }
    ));
    assert!(parse_helper_log("restore cancelled before writing").cancelled_before_write);
    assert!(
        parse_helper_log("could not write the image: restore cancelled").cancelled_during_write
    );
    assert!(!parse_helper_log("restore cancelled before writing").cancelled_during_write);

    let progress = parse_progress_line("write 512 4096\n").unwrap();
    assert_eq!(progress.phase, "write");
    assert_eq!(progress.bytes_done, 512);
    assert_eq!(progress.bytes_total, 4096);
    assert!(parse_sidecar(DIGEST).is_ok());
    assert!(parse_sidecar("nope").is_err());
}

#[test]
fn physical_backup_paths_are_refused_before_open() {
    let scratch = Scratch::new("restore-physical");
    let live = disk(2, "USB", 4096, false, "Test SD");
    for path in ["/dev/sda", r"\\.\PhysicalDrive1", "/dev/mmcblk0"] {
        let record = poisoned(scratch.path(), "dest.img", 4096, 4096);
        let (mut record, result) = restore_to_device(
            Path::new(path),
            record,
            &target(&live, "FLASH", 4096, "Test SD", 2),
            &AtomicBool::new(false),
            |_| {},
        );
        let error = result.unwrap_err().to_string();
        assert!(
            error.to_lowercase().contains("physical"),
            "{path} -> {error}"
        );
        assert!(record.writes.is_empty());
        still_poison(&mut record.inner);
        let _ = std::fs::remove_file(scratch.path().join("dest.img"));
    }
}

#[cfg(unix)]
#[test]
fn restore_refuses_a_symlinked_backup() {
    let scratch = Scratch::new("restore-link");
    let image = write_backup(scratch.path(), "stock.img.gz", b"linked");
    let link = scratch.path().join("link.img.gz");
    std::os::unix::fs::symlink(&image, &link).unwrap();
    let live = disk(2, "USB", 4096, false, "Test SD");
    let record = poisoned(scratch.path(), "dest.img", 4096, 4096);
    let (mut record, result) = restore_to_device(
        &link,
        record,
        &target(&live, "FLASH", 4096, "Test SD", 2),
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .to_lowercase()
        .contains("symlink"));
    assert!(record.writes.is_empty());
    still_poison(&mut record.inner);
    assert_eq!(std::fs::read(&image).unwrap(), gzip_bytes(b"linked"));
}

#[test]
fn cancel_file_is_exact() {
    let scratch = Scratch::new("restore-flag");
    let path = scratch.path().join("cancel.txt");
    assert!(!cancel_file_requests_stop(&path).unwrap());
    std::fs::write(&path, b"\n").unwrap();
    assert!(!cancel_file_requests_stop(&path).unwrap());
    std::fs::write(&path, b"1\n").unwrap();
    assert!(cancel_file_requests_stop(&path).unwrap());
}
