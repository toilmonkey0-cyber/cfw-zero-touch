//! Quick fake-card probe. Devices are fixture files only.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cfw_zero_touch_lib::backup::{backup_image, BackupOutcome};
use cfw_zero_touch_lib::block_device::{is_physical_disk_path, FakeBlockDevice, FileBlockDevice};
use cfw_zero_touch_lib::probe::{
    authorize_probe, cancel_summary, card_is_blank, helper_script_line, parse_probe_log,
    plain_summary, run_probe, sample_offsets, BackupSession, CardKind, ProbeBlock, ProbeDisk,
    ProbeError, ProbePlan, ProbeRequest, ProbeStatus, SessionBackup, PROBE_BLOCK,
};
use cfw_zero_touch_lib::restore::signal_cancel_file;

#[allow(dead_code, unused_imports)]
#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::{
    fake_card, genuine_card, Scratch, CAPACITY_SCALE, FAKE_CLAIM_BYTES, FAKE_REAL_BYTES,
};

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

/// Reports a stored length equal to the claim so a probe cannot cheat by
/// reading `stored_len` instead of the patterns it wrote.
struct HideRealSize<D> {
    inner: D,
}

impl<D: FakeBlockDevice> FakeBlockDevice for HideRealSize<D> {
    fn reported_len(&self) -> u64 {
        self.inner.reported_len()
    }

    fn stored_len(&self) -> u64 {
        self.inner.reported_len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_at(offset, buf)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_at(offset, data)
    }
}

/// Reads past `limit` come back as zeros. Writes still land.
struct DropReads<D> {
    inner: D,
    limit: u64,
}

impl<D: FakeBlockDevice> FakeBlockDevice for DropReads<D> {
    fn reported_len(&self) -> u64 {
        self.inner.reported_len()
    }

    fn stored_len(&self) -> u64 {
        self.inner.stored_len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        if offset >= self.limit {
            buf.fill(0);
            return Ok(());
        }
        self.inner.read_at(offset, buf)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_at(offset, data)
    }
}

struct CancelOnWrite<D> {
    inner: D,
    cancel: std::sync::Arc<AtomicBool>,
    writes: u32,
}

impl<D: FakeBlockDevice> FakeBlockDevice for CancelOnWrite<D> {
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
        self.inner.write_at(offset, data)?;
        self.writes += 1;
        if self.writes == 1 {
            self.cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
}

fn disk(number: u32, bus: &str, size: u64, system: bool, model: &str) -> ProbeDisk {
    ProbeDisk {
        number,
        bus: bus.into(),
        size_bytes: size,
        system_disk: system,
        model: model.into(),
    }
}

fn plan_for(disk: &ProbeDisk) -> ProbePlan {
    ProbePlan {
        confirmation: "PROBE".into(),
        disk_number: disk.number,
        displayed_bytes: disk.size_bytes,
        displayed_model: disk.model.clone(),
    }
}

fn open_fixture(scratch: &Scratch, bytes: &[u8], reported: u64) -> FileBlockDevice {
    let path = scratch.path().join("card.img");
    assert!(
        !is_physical_disk_path(&path),
        "fixture path must be a regular file"
    );
    let mut device = FileBlockDevice::create(&path, bytes.len() as u64, reported).unwrap();
    device.write_at(0, bytes).unwrap();
    device
}

fn run_on(
    device: &mut dyn FakeBlockDevice,
    disk: &ProbeDisk,
    backup: Option<&SessionBackup>,
    cancel: &AtomicBool,
) -> Result<ProbeStatus, ProbeError> {
    let plan = plan_for(disk);
    let request = ProbeRequest {
        disk,
        plan: &plan,
        backup,
    };
    run_probe(device, &request, cancel, None)
}

fn finished_backup(dir: &Path, bytes: &[u8], model: &str) -> SessionBackup {
    let image_path = dir.join("source.img");
    let mut device =
        FileBlockDevice::create(&image_path, bytes.len() as u64, bytes.len() as u64).unwrap();
    device.write_at(0, bytes).unwrap();
    drop(device);
    let mut source = FileBlockDevice::open_read_only(&image_path).unwrap();
    let dest = dir.join("stock.img.gz");
    let outcome = backup_image(
        &mut source,
        &dest,
        u64::MAX,
        &AtomicBool::new(false),
        |_| {},
    )
    .expect("backup fixture");
    match outcome {
        BackupOutcome::Complete {
            image,
            sha256,
            bytes: size,
            ..
        } => SessionBackup {
            image,
            size_bytes: size,
            model: model.into(),
            sha256,
        },
        BackupOutcome::Cancelled { .. } => panic!("backup fixture was cancelled"),
    }
}

#[test]
fn fake_and_genuine_fixtures_keep_the_64_to_8_story_without_a_huge_file() {
    let fake = fake_card();
    assert_eq!(FAKE_CLAIM_BYTES / FAKE_REAL_BYTES, 8);
    assert_eq!(fake.reported_len, FAKE_CLAIM_BYTES / CAPACITY_SCALE);
    assert_eq!(fake.stored_len(), FAKE_REAL_BYTES / CAPACITY_SCALE);
    assert_eq!(fake.stored_len() * 8, fake.reported_len);
    assert!(fake.stored_len() < 64 * 1024);
    assert!(fake.bytes.iter().all(|byte| *byte == 0));

    let genuine = genuine_card();
    assert_eq!(genuine.stored_len(), genuine.reported_len);
    assert!(genuine.stored_len() > 512);
    assert_eq!(genuine.bytes[510], 0x55);
    assert_eq!(genuine.bytes[511], 0xAA);
}

#[test]
fn sample_offsets_cover_the_claim_and_include_the_last_block() {
    let fake = fake_card();
    let offsets = sample_offsets(fake.reported_len);
    assert_eq!(offsets.first().copied(), Some(0));
    assert!(
        offsets
            .iter()
            .any(|offset| offset + PROBE_BLOCK == fake.reported_len),
        "near the end of {offsets:?}"
    );
    assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(offsets
        .iter()
        .all(|offset| offset % PROBE_BLOCK == 0 && offset + PROBE_BLOCK <= fake.reported_len));
}

#[test]
fn wraparound_fake_card_reports_the_real_size_and_aliasing() {
    let fake = fake_card();
    let scratch = Scratch::new("probe-fake");
    let device = open_fixture(&scratch, &fake.bytes, fake.reported_len);
    let mut device = CountWrites {
        inner: HideRealSize { inner: device },
        writes: 0,
    };
    let card = disk(2, "USB", fake.reported_len, false, "Generic SD");
    let status = run_on(&mut device, &card, None, &AtomicBool::new(false)).unwrap();
    let ProbeStatus::Finished(report) = status else {
        panic!("expected a finished probe");
    };
    assert_eq!(report.kind, CardKind::Fake);
    assert!(report.aliased);
    assert_eq!(report.real_bytes, fake.stored_len());
    assert_eq!(report.claimed_bytes, fake.reported_len);
    assert!(report.write_mbps.is_finite() && report.write_mbps > 0.0);
    assert!(report.read_mbps.is_finite() && report.read_mbps > 0.0);
    assert!(device.writes > 0);
    let summary = plain_summary(CardKind::Fake, 64_000_000_000, 8_000_000_000, 12.0, 40.0);
    assert_eq!(
        summary,
        "This card is fake: it says 64 GB but only holds about 8 GB. Sequential write is 12 MB/s and sequential read is 40 MB/s."
    );
}

#[test]
fn genuine_blank_card_reports_the_size_it_claims() {
    let scratch = Scratch::new("probe-genuine-blank");
    let reported = 64 * 1024;
    let device = open_fixture(&scratch, &vec![0u8; reported as usize], reported);
    let mut device = CountWrites {
        inner: device,
        writes: 0,
    };
    let card = disk(3, "SD", reported, false, "Genuine SD");
    let status = run_on(&mut device, &card, None, &AtomicBool::new(false)).unwrap();
    let ProbeStatus::Finished(report) = status else {
        panic!("expected a finished probe");
    };
    assert_eq!(report.kind, CardKind::Genuine);
    assert!(!report.aliased);
    assert_eq!(report.real_bytes, reported);
    assert_eq!(report.claimed_bytes, reported);
    assert!(report.summary.contains("genuine"));
    assert!(report.summary.contains("65.5 KB"));
}

#[test]
fn a_card_that_drops_data_is_failing_rather_than_a_clean_wrap() {
    let scratch = Scratch::new("probe-fail");
    let reported = 64 * 1024;
    let device = open_fixture(&scratch, &vec![0u8; reported as usize], reported);
    let mut device = DropReads {
        inner: device,
        limit: 8 * 1024,
    };
    let card = disk(3, "usb", reported, false, "Tired SD");
    let status = run_on(&mut device, &card, None, &AtomicBool::new(false)).unwrap();
    let ProbeStatus::Finished(report) = status else {
        panic!("expected a finished probe");
    };
    assert_eq!(report.kind, CardKind::Failing);
    assert_eq!(report.real_bytes, 8 * 1024);
    assert!(report.summary.contains("failing"));
}

#[test]
fn partitioned_card_without_a_session_backup_is_not_written() {
    let genuine = genuine_card();
    let scratch = Scratch::new("probe-refuse");
    let device = open_fixture(&scratch, &genuine.bytes, genuine.reported_len);
    let before = std::fs::read(device.path()).unwrap();
    let mut device = CountWrites {
        inner: device,
        writes: 0,
    };
    assert!(!card_is_blank(&mut device).unwrap());
    let card = disk(2, "USB", genuine.reported_len, false, "Stock R36S");
    let error = run_on(&mut device, &card, None, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(error, ProbeError::Blocked(ProbeBlock::NotBlank)));
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(device.inner.path()).unwrap(), before);

    let backup = finished_backup(scratch.path(), &genuine.bytes, "Stock R36S");
    let mut session = BackupSession::default();
    assert!(session.find(card.size_bytes, &card.model).is_none());
    session.remember(backup);
    assert!(session.find(card.size_bytes, &card.model).is_some());
    assert!(session.find(card.size_bytes, "Other reader").is_none());
}

#[test]
fn a_card_with_no_partition_table_is_blank_even_if_some_bytes_are_not() {
    let scratch = Scratch::new("probe-nopart");
    let reported = 8192u64;
    let mut bytes = vec![0u8; reported as usize];
    bytes[600] = 0xAB;
    let mut device = open_fixture(&scratch, &bytes, reported);
    assert!(card_is_blank(&mut device).unwrap());
}

#[test]
fn verified_session_backup_allows_a_partitioned_card_and_a_bad_sidecar_does_not() {
    let genuine = genuine_card();
    let scratch = Scratch::new("probe-backup");
    let backup = finished_backup(scratch.path(), &genuine.bytes, "Stock R36S");
    let device = open_fixture(&scratch, &genuine.bytes, genuine.reported_len);
    let mut device = CountWrites {
        inner: device,
        writes: 0,
    };
    let card = disk(2, "USB", genuine.reported_len, false, "Stock R36S");
    let status = run_on(&mut device, &card, Some(&backup), &AtomicBool::new(false)).unwrap();
    assert!(matches!(status, ProbeStatus::Finished(_)));
    assert!(device.writes > 0);

    let again = open_fixture(&scratch, &genuine.bytes, genuine.reported_len);
    let mut again = CountWrites {
        inner: again,
        writes: 0,
    };
    let mut wrong = backup.clone();
    wrong.sha256 = "ab".repeat(32);
    let error = run_on(&mut again, &card, Some(&wrong), &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(error, ProbeError::Blocked(ProbeBlock::Backup)));
    assert_eq!(again.writes, 0);

    let mut other_model = backup.clone();
    other_model.model = "Other reader".into();
    let error = run_on(
        &mut again,
        &card,
        Some(&other_model),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(error, ProbeError::Blocked(ProbeBlock::Backup)));
    assert_eq!(again.writes, 0);
}

#[test]
fn probe_refuses_system_boot_fixed_and_mismatched_disks_without_writing() {
    let size = 64 * 1024;
    let ok = disk(2, "USB", size, false, "Generic SD");
    authorize_probe(&ok, &plan_for(&ok)).unwrap();
    authorize_probe(&disk(4, "sd", size, false, "Built-in"), &{
        let mut plan = plan_for(&disk(4, "sd", size, false, "Built-in"));
        plan.displayed_model = "Built-in".into();
        plan
    })
    .unwrap();

    for refused in [
        disk(0, "USB", size, true, "System"),
        disk(0, "USB", size, false, "System"),
        disk(2, "USB", size, true, "System"),
        disk(3, "NVMe", size, false, "Internal"),
        disk(3, "SATA", size, false, "Internal"),
    ] {
        let error = authorize_probe(&refused, &plan_for(&refused)).unwrap_err();
        assert_eq!(error, ProbeBlock::NotRemovable, "{refused:?}");
    }

    let mut changed_size = plan_for(&ok);
    changed_size.displayed_bytes = size - 512;
    assert_eq!(
        authorize_probe(&ok, &changed_size).unwrap_err(),
        ProbeBlock::Size
    );

    let mut changed_model = plan_for(&ok);
    changed_model.displayed_model = "Other".into();
    assert_eq!(
        authorize_probe(&ok, &changed_model).unwrap_err(),
        ProbeBlock::Model
    );

    let mut other_disk = plan_for(&ok);
    other_disk.disk_number = 9;
    assert_eq!(
        authorize_probe(&ok, &other_disk).unwrap_err(),
        ProbeBlock::Identity
    );

    let mut word = plan_for(&ok);
    word.confirmation = "FLASH".into();
    assert_eq!(
        authorize_probe(&ok, &word).unwrap_err(),
        ProbeBlock::Confirmation
    );

    let scratch = Scratch::new("probe-gate");
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let before = std::fs::read(device.path()).unwrap();
    let mut device = CountWrites {
        inner: device,
        writes: 0,
    };
    let error = run_on(&mut device, &ok, None, &AtomicBool::new(false));
    // The matching plan is allowed; this call proves the blank card can run.
    assert!(error.is_ok());
    let _ = before;
}

#[test]
fn a_refused_probe_does_not_write_when_the_recheck_fails() {
    let size = 32 * 1024;
    let scratch = Scratch::new("probe-recheck");
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let before = std::fs::read(device.path()).unwrap();
    let mut device = CountWrites {
        inner: device,
        writes: 0,
    };
    let card = disk(2, "SD", size, false, "Generic SD");
    let mut plan = plan_for(&card);
    plan.displayed_model = "Swapped card".into();
    let request = ProbeRequest {
        disk: &card,
        plan: &plan,
        backup: None,
    };
    let error = run_probe(&mut device, &request, &AtomicBool::new(false), None).unwrap_err();
    assert!(matches!(error, ProbeError::Blocked(ProbeBlock::Model)));
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(device.inner.path()).unwrap(), before);

    let boot = disk(2, "USB", size, true, "Generic SD");
    let error = run_on(&mut device, &boot, None, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(
        error,
        ProbeError::Blocked(ProbeBlock::NotRemovable)
    ));
    assert_eq!(device.writes, 0);
}

#[test]
fn physical_backup_paths_are_refused_before_the_card_is_written() {
    let genuine = genuine_card();
    let scratch = Scratch::new("probe-physical");
    let device = open_fixture(&scratch, &genuine.bytes, genuine.reported_len);
    let mut device = CountWrites {
        inner: device,
        writes: 0,
    };
    let card = disk(2, "USB", genuine.reported_len, false, "Stock R36S");
    let backup = SessionBackup {
        image: PathBuf::from(r"\\.\PhysicalDrive2"),
        size_bytes: genuine.reported_len,
        model: "Stock R36S".into(),
        sha256: "ab".repeat(32),
    };
    let error = run_on(&mut device, &card, Some(&backup), &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(error, ProbeError::Blocked(ProbeBlock::Backup)));
    assert_eq!(device.writes, 0);
    assert!(is_physical_disk_path(Path::new("/dev/mmcblk0")));
}

#[test]
fn cancel_before_a_write_says_the_card_was_not_overwritten() {
    let scratch = Scratch::new("probe-cancel-early");
    let reported = 16 * 1024;
    let device = open_fixture(&scratch, &vec![0u8; reported as usize], reported);
    let before = std::fs::read(device.path()).unwrap();
    let mut device = CountWrites {
        inner: device,
        writes: 0,
    };
    let card = disk(2, "USB", reported, false, "Generic SD");
    let cancel = AtomicBool::new(true);
    let status = run_on(&mut device, &card, None, &cancel).unwrap();
    match status {
        ProbeStatus::Cancelled {
            overwritten,
            summary,
        } => {
            assert!(!overwritten);
            assert_eq!(summary, cancel_summary(false));
            assert!(summary.contains("Nothing"));
        }
        ProbeStatus::Finished(_) => panic!("cancel before writing must not finish"),
    }
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(device.inner.path()).unwrap(), before);
    assert_eq!(
        cancel_summary(false),
        "Cancelled. Nothing on the card was overwritten."
    );
}

#[test]
fn cancel_after_a_write_says_data_was_overwritten() {
    let scratch = Scratch::new("probe-cancel-late");
    let reported = 16 * 1024;
    let device = open_fixture(&scratch, &vec![0u8; reported as usize], reported);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let mut device = CancelOnWrite {
        inner: device,
        cancel: std::sync::Arc::clone(&cancel),
        writes: 0,
    };
    let card = disk(2, "SD", reported, false, "Generic SD");
    let status = run_on(&mut device, &card, None, &cancel).unwrap();
    match status {
        ProbeStatus::Cancelled {
            overwritten,
            summary,
        } => {
            assert!(overwritten);
            assert_eq!(summary, cancel_summary(true));
            assert!(summary.contains("overwritten"));
        }
        ProbeStatus::Finished(_) => panic!("cancel after a write must not look finished"),
    }
    assert!(device.writes >= 1);
    assert_eq!(
        cancel_summary(true),
        "Cancelled. Data on the card was overwritten. Restore your backup before you use this card."
    );
}

#[test]
fn a_cancel_file_before_writing_leaves_the_card_unchanged() {
    let scratch = Scratch::new("probe-cancel-file");
    let reported = 8192u64;
    let device = open_fixture(&scratch, &vec![0u8; reported as usize], reported);
    let before = std::fs::read(device.path()).unwrap();
    let flag = scratch.path().join("cancel.txt");
    signal_cancel_file(&flag).unwrap();
    let mut device = CountWrites {
        inner: device,
        writes: 0,
    };
    let card = disk(2, "USB", reported, false, "Generic SD");
    let plan = plan_for(&card);
    let request = ProbeRequest {
        disk: &card,
        plan: &plan,
        backup: None,
    };
    let status = run_probe(
        &mut device,
        &request,
        &AtomicBool::new(false),
        Some(flag.as_path()),
    )
    .unwrap();
    match status {
        ProbeStatus::Cancelled { overwritten, .. } => assert!(!overwritten),
        ProbeStatus::Finished(_) => panic!("cancel file should stop the probe"),
    }
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(device.inner.path()).unwrap(), before);
}

#[test]
fn helper_launch_uses_the_elevated_tool_and_refuses_a_physical_backup_path() {
    let tool = Path::new("cfw-flash.exe");
    let cancel = Path::new("C:\\Temp\\cfw-probe-cancel.txt");
    let log = Path::new("C:\\Temp\\cfw-probe-log.txt");
    let backup = Path::new("C:\\Backups\\stock.img.gz");
    let script = helper_script_line(
        tool,
        &cfw_zero_touch_lib::probe::HelperRequest {
            disk_number: 2,
            confirmation: "PROBE".into(),
            displayed_bytes: 64_000_000_000,
            displayed_model: "Generic SD".into(),
            backup_image: Some(backup.into()),
            backup_sha256: Some("ab".repeat(32)),
            cancel_file: cancel.into(),
            log_file: log.into(),
        },
    )
    .unwrap();
    assert!(script.contains("--probe"));
    assert!(script.contains("--disk=2"));
    assert!(script.contains("--confirm='PROBE'"));
    assert!(script.contains("--expected-bytes=64000000000"));
    assert!(script.contains("--expected-model='Generic SD'"));
    assert!(script.contains("stock.img.gz"));
    assert!(!script.to_ascii_lowercase().contains("physicaldrive"));
    assert!(!script.contains("Clear-Disk"));

    let error = helper_script_line(
        tool,
        &cfw_zero_touch_lib::probe::HelperRequest {
            disk_number: 2,
            confirmation: "PROBE".into(),
            displayed_bytes: 1024,
            displayed_model: "Generic SD".into(),
            backup_image: Some(PathBuf::from(r"\\.\PhysicalDrive1")),
            backup_sha256: None,
            cancel_file: cancel.into(),
            log_file: log.into(),
        },
    )
    .unwrap_err();
    assert!(error.to_lowercase().contains("physical"));
}

#[test]
fn probe_log_parser_keeps_cancel_honest() {
    let finished = "\
KIND=fake
CLAIMED=64000000000
REAL=8000000000
WRITE_MBPS=12
READ_MBPS=40
ALIASED=yes
SUMMARY=This card is fake: it says 64 GB but only holds about 8 GB. Sequential write is 12 MB/s and sequential read is 40 MB/s.
RESULT=OK
EXIT=0
";
    let report = parse_probe_log(finished).unwrap();
    assert_eq!(report.status, "finished");
    assert_eq!(report.kind, "fake");
    assert_eq!(report.claimed_bytes, 64_000_000_000);
    assert_eq!(report.real_bytes, 8_000_000_000);
    assert!(!report.overwritten);
    assert!(report
        .summary
        .starts_with("This card is fake: it says 64 GB but only holds about 8 GB."));

    let cancelled = "\
RESULT=CANCELLED
OVERWRITTEN=yes
SUMMARY=Cancelled. Data on the card was overwritten. Restore your backup before you use this card.
EXIT=9
";
    let report = parse_probe_log(cancelled).unwrap();
    assert_eq!(report.status, "cancelled");
    assert!(report.overwritten);
    assert!(report.summary.contains("overwritten"));
}
