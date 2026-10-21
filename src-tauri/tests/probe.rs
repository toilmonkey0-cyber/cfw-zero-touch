//! Quick fake-card probe. Devices are fixture files only.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cfw_zero_touch_lib::backup::{backup_image, BackupOutcome};
use cfw_zero_touch_lib::block_device::{is_physical_disk_path, FakeBlockDevice, FileBlockDevice};
use cfw_zero_touch_lib::probe::{
    accept_read_only_identity, app_precheck, authorize_probe, cancel_summary, card_is_blank,
    format_failed_log, format_probe_log, helper_launch_command, identity_path,
    interpret_helper_log, live_identity_matches, load_identity, normalize_partition_table_id,
    overwritten_flag, parse_probe_log, plain_summary, prefix_sha256, run_probe,
    sample_block_hashes, sample_offsets, write_identity, AppPrecheck, BackupSession, CardIdentity,
    CardKind, HelperFollowup, ProbeBlock, ProbeDisk, ProbeError, ProbePlan, ProbeReport,
    ProbeRequest, ProbeStatus, SessionBackup, PREFIX_IDENTITY_BYTES, PROBE_BLOCK,
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

const FIXTURE_SERIAL: &str = "fixture-card";

fn disk(number: u32, bus: &str, size: u64, system: bool, model: &str) -> ProbeDisk {
    ProbeDisk {
        number,
        bus: bus.into(),
        size_bytes: size,
        system_disk: system,
        model: model.into(),
        serial: String::new(),
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
    let prefix = prefix_sha256(&mut source).expect("fixture prefix");
    let samples = sample_block_hashes(&mut source).expect("fixture samples");
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
        } => {
            let identity = CardIdentity {
                serial: FIXTURE_SERIAL.into(),
                prefix_sha256: prefix,
                bytes: size,
                model: model.into(),
                samples,
            };
            write_identity(&image, &identity).expect("fixture identity");
            SessionBackup {
                image,
                size_bytes: size,
                model: model.into(),
                sha256,
                identity,
            }
        }
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
    let summary = plain_summary(
        CardKind::Fake,
        64_000_000_000,
        8_000_000_000,
        12.0,
        40.0,
        false,
    );
    assert_eq!(
        summary,
        "This card is fake: it says 64 GB but only holds about 8 GB. Sequential write is 12 MB/s and sequential read is 40 MB/s. Part of this card was overwritten. Format the card again before you use this card."
    );
    assert!(report.summary.contains("Part of this card was overwritten"));
    assert!(report.summary.contains("Format the card again"));
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
    assert!(
        !card_is_blank(&mut device).unwrap(),
        "leftover bytes are data even when the card has no partition table"
    );

    let zeros = Scratch::new("probe-zeros");
    let mut blank = open_fixture(&zeros, &vec![0u8; reported as usize], reported);
    assert!(card_is_blank(&mut blank).unwrap());
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
    let mut card = disk(2, "USB", genuine.reported_len, false, "Stock R36S");
    card.serial = FIXTURE_SERIAL.into();
    let status = run_on(&mut device, &card, Some(&backup), &AtomicBool::new(false)).unwrap();
    let ProbeStatus::Finished(report) = status else {
        panic!("expected a finished probe");
    };
    assert!(report.summary.contains("Restore your backup"));
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
        identity: CardIdentity {
            serial: FIXTURE_SERIAL.into(),
            prefix_sha256: "ab".repeat(32),
            bytes: genuine.reported_len,
            model: "Stock R36S".into(),
            samples: Vec::new(),
        },
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
            assert_eq!(summary, cancel_summary(false, false));
            assert!(summary.contains("Nothing"));
        }
        ProbeStatus::Finished(_) => panic!("cancel before writing must not finish"),
    }
    assert_eq!(device.writes, 0);
    assert_eq!(std::fs::read(device.inner.path()).unwrap(), before);
    assert_eq!(
        cancel_summary(false, false),
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
            assert_eq!(summary, cancel_summary(true, false));
            assert!(summary.contains("Format the card again"));
        }
        ProbeStatus::Finished(_) => panic!("cancel after a write must not look finished"),
    }
    assert!(device.writes >= 1);
    assert_eq!(
        cancel_summary(true, false),
        "Cancelled. Data on the card was overwritten. Format the card again before you use this card."
    );
    assert_eq!(
        cancel_summary(true, true),
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
    let script = helper_launch_command(
        tool,
        &cfw_zero_touch_lib::probe::HelperRequest {
            disk_number: 2,
            confirmation: "PROBE".into(),
            displayed_bytes: 64_000_000_000,
            displayed_model: "Generic SD".into(),
            backup_image: Some(backup.into()),
            backup_sha256: Some("ab".repeat(32)),
            backup_prefix_sha256: Some("cd".repeat(32)),
            backup_serial: Some(FIXTURE_SERIAL.into()),
            cancel_file: cancel.into(),
            log_file: log.into(),
        },
    )
    .unwrap();
    assert!(script.contains("Start-Process -FilePath 'cfw-flash.exe'"));
    assert!(script.contains("-ArgumentList"));
    assert!(script.contains("'--probe'"));
    assert!(script.contains("'--disk=2'"));
    assert!(script.contains("'--confirm=PROBE'"));
    assert!(script.contains("'--expected-bytes=64000000000'"));
    assert!(script.contains("'--expected-model=Generic SD'"));
    assert!(script.contains("'--backup-prefix-sha256="));
    assert!(script.contains("'--backup-serial=fixture-card'"));
    assert!(script.contains("stock.img.gz"));
    assert!(!script.contains(".ps1"));
    assert!(!script.to_ascii_lowercase().contains("physicaldrive"));
    assert!(!script.contains("Clear-Disk"));

    let error = helper_launch_command(
        tool,
        &cfw_zero_touch_lib::probe::HelperRequest {
            disk_number: 2,
            confirmation: "PROBE".into(),
            displayed_bytes: 1024,
            displayed_model: "Generic SD".into(),
            backup_image: Some(PathBuf::from(r"\\.\PhysicalDrive1")),
            backup_sha256: None,
            backup_prefix_sha256: None,
            backup_serial: None,
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
OVERWRITTEN=yes
SUMMARY=This card is fake: it says 64 GB but only holds about 8 GB. Sequential write is 12 MB/s and sequential read is 40 MB/s. Part of this card was overwritten. Restore your backup before you use this card.
RESULT=OK
EXIT=0
";
    let report = parse_probe_log(finished).unwrap();
    assert_eq!(report.status, "finished");
    assert_eq!(report.kind, "fake");
    assert_eq!(report.claimed_bytes, 64_000_000_000);
    assert_eq!(report.real_bytes, 8_000_000_000);
    assert!(report.overwritten);
    assert!(report
        .summary
        .starts_with("This card is fake: it says 64 GB but only holds about 8 GB."));
    assert!(report.summary.contains("Restore your backup"));

    let missing = finished.replace("OVERWRITTEN=yes\n", "");
    assert!(parse_probe_log(&missing)
        .unwrap_err()
        .contains("OVERWRITTEN"));
    assert!(overwritten_flag(&missing).is_err());
    let finished_but_not = finished.replace("OVERWRITTEN=yes", "OVERWRITTEN=no");
    assert!(parse_probe_log(&finished_but_not)
        .unwrap_err()
        .contains("overwritten"));

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

    let finished_status = ProbeStatus::Finished(ProbeReport {
        kind: CardKind::Fake,
        claimed_bytes: 64_000_000_000,
        real_bytes: 8_000_000_000,
        read_mbps: 40.0,
        write_mbps: 12.0,
        aliased: true,
        summary: "done".into(),
    });
    let log = format_probe_log(&finished_status);
    assert!(log.contains("OVERWRITTEN=yes"));
    assert!(log.contains("RESULT=OK"));
    assert!(overwritten_flag(&log).unwrap());
    assert!(format_probe_log(&ProbeStatus::Cancelled {
        overwritten: false,
        summary: cancel_summary(false, true),
    })
    .contains("OVERWRITTEN=no"));
    assert!(format_failed_log(true, "write failed").contains("OVERWRITTEN=yes"));
    assert!(format_failed_log(false, "refused").contains("OVERWRITTEN=no"));
}

#[test]
fn a_backup_of_a_different_card_with_the_same_model_and_size_is_refused() {
    let scratch = Scratch::new("probe-other-card");
    let size = 16 * 1024;
    let model = "Generic SD";
    let mut card_a = vec![0u8; size];
    card_a[0] = 0x11;
    card_a[510] = 0x55;
    card_a[511] = 0xAA;
    let mut card_b = card_a.clone();
    card_b[0] = 0x22;
    let backup = finished_backup(scratch.path(), &card_a, model);
    let mut card = disk(2, "USB", size as u64, false, model);
    card.serial = FIXTURE_SERIAL.into();

    let matching = open_fixture(&scratch, &card_a, size as u64);
    let mut matching = CountWrites {
        inner: matching,
        writes: 0,
    };
    let status = run_on(&mut matching, &card, Some(&backup), &AtomicBool::new(false)).unwrap();
    assert!(matches!(status, ProbeStatus::Finished(_)));
    assert!(matching.writes > 0);

    let other = open_fixture(&scratch, &card_b, size as u64);
    let before = std::fs::read(other.path()).unwrap();
    let mut other = CountWrites {
        inner: other,
        writes: 0,
    };
    let error = run_on(&mut other, &card, Some(&backup), &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(error, ProbeError::Blocked(ProbeBlock::Backup)));
    assert_eq!(other.writes, 0);
    assert_eq!(std::fs::read(other.inner.path()).unwrap(), before);

    card.serial = "other-reader".into();
    let same_bytes = open_fixture(&scratch, &card_a, size as u64);
    let before = std::fs::read(same_bytes.path()).unwrap();
    let mut same_bytes = CountWrites {
        inner: same_bytes,
        writes: 0,
    };
    let error = run_on(
        &mut same_bytes,
        &card,
        Some(&backup),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(error, ProbeError::Blocked(ProbeBlock::Backup)));
    assert_eq!(same_bytes.writes, 0);
    assert_eq!(std::fs::read(same_bytes.inner.path()).unwrap(), before);

    let mut forged_device = open_fixture(&scratch, &card_b, size as u64);
    let forged_prefix = prefix_sha256(&mut forged_device).unwrap();
    let mut stored = cfw_zero_touch_lib::probe::load_identity(&backup.image).unwrap();
    stored.prefix_sha256 = forged_prefix;
    let identity_file = cfw_zero_touch_lib::probe::identity_path(&backup.image);
    std::fs::remove_file(&identity_file).unwrap();
    write_identity(&backup.image, &stored).unwrap();
    let mut forged = backup.clone();
    forged.identity = stored;
    card.serial = FIXTURE_SERIAL.into();
    let mut forged_device = CountWrites {
        inner: forged_device,
        writes: 0,
    };
    let error = run_on(
        &mut forged_device,
        &card,
        Some(&forged),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        matches!(error, ProbeError::Blocked(ProbeBlock::Backup)),
        "editing the identity file cannot retarget a backup to another card"
    );
    assert_eq!(forged_device.writes, 0);
}

#[test]
fn two_cards_with_the_same_first_4_mib_but_different_sample_blocks_are_refused() {
    let scratch = Scratch::new("probe-sample-identity");
    let size: u64 = 8 * 1024 * 1024;
    let model = "Generic SD";
    let sample_at: u64 = 4 * 1024 * 1024;
    assert!(sample_at >= PREFIX_IDENTITY_BYTES);
    assert!(sample_offsets(size).contains(&sample_at));

    let mut card_a = vec![0u8; size as usize];
    card_a[100] = 0x11;
    let mut card_b = card_a.clone();
    card_b[sample_at as usize] = 0xAB;

    let mut device_a =
        FileBlockDevice::create(&scratch.path().join("hash-a.img"), size, size).unwrap();
    device_a.write_at(0, &card_a).unwrap();
    let mut device_b =
        FileBlockDevice::create(&scratch.path().join("hash-b.img"), size, size).unwrap();
    device_b.write_at(0, &card_b).unwrap();
    let prefix_a = prefix_sha256(&mut device_a).unwrap();
    let prefix_b = prefix_sha256(&mut device_b).unwrap();
    assert_eq!(prefix_a, prefix_b, "the first 4 MiB match");
    let samples_a = sample_block_hashes(&mut device_a).unwrap();
    let samples_b = sample_block_hashes(&mut device_b).unwrap();
    assert_ne!(samples_a, samples_b);
    assert!(samples_a.iter().zip(&samples_b).any(|(left, right)| {
        left.offset == right.offset
            && left.offset >= PREFIX_IDENTITY_BYTES
            && left.sha256 != right.sha256
    }));

    let backup = finished_backup(scratch.path(), &card_a, model);
    let recorded = load_identity(&backup.image).unwrap();
    assert_eq!(recorded.samples, samples_a);
    assert!(std::fs::read_to_string(identity_path(&backup.image))
        .unwrap()
        .contains(&format!("sample-sha256={sample_at}:")));

    let mut card = disk(2, "USB", size, false, model);
    card.serial = FIXTURE_SERIAL.into();

    let same = open_fixture(&scratch, &card_a, size);
    let mut same = CountWrites {
        inner: same,
        writes: 0,
    };
    assert!(live_identity_matches(&mut same, &backup.identity).is_ok());
    let status = run_on(&mut same, &card, Some(&backup), &AtomicBool::new(false)).unwrap();
    assert!(matches!(status, ProbeStatus::Finished(_)));
    assert!(same.writes > 0);

    let other = open_fixture(&scratch, &card_b, size);
    let before = std::fs::read(other.path()).unwrap();
    let mut other = CountWrites {
        inner: other,
        writes: 0,
    };
    let mismatch = live_identity_matches(&mut other, &backup.identity).unwrap_err();
    assert!(matches!(mismatch, ProbeError::Blocked(ProbeBlock::Backup)));
    let error = run_on(&mut other, &card, Some(&backup), &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(error, ProbeError::Blocked(ProbeBlock::Backup)));
    assert_eq!(other.writes, 0);
    assert_eq!(std::fs::read(other.inner.path()).unwrap(), before);

    let mut forged_identity = load_identity(&backup.image).unwrap();
    forged_identity.samples = samples_b;
    let identity_file = identity_path(&backup.image);
    std::fs::remove_file(&identity_file).unwrap();
    write_identity(&backup.image, &forged_identity).unwrap();
    let mut forged = backup.clone();
    forged.identity = forged_identity;
    let other = open_fixture(&scratch, &card_b, size);
    let before = std::fs::read(other.path()).unwrap();
    let mut other = CountWrites {
        inner: other,
        writes: 0,
    };
    let error = run_on(&mut other, &card, Some(&forged), &AtomicBool::new(false)).unwrap_err();
    assert!(
        matches!(error, ProbeError::Blocked(ProbeBlock::Backup)),
        "editing the sample hashes cannot retarget a backup to another card"
    );
    assert_eq!(other.writes, 0);
    assert_eq!(std::fs::read(other.inner.path()).unwrap(), before);
}

#[test]
fn a_partition_table_id_is_the_gpt_guid_or_the_mbr_signature() {
    let gpt = normalize_partition_table_id("GPT|{AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE}").unwrap();
    assert_eq!(gpt, "gpt:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    let mbr = normalize_partition_table_id("MBR|00ab12cd").unwrap();
    assert_eq!(mbr, "mbr:00ab12cd");
    for raw in ["RAW|", "GPT|", "MBR|00000000", "MBR|zz", ""] {
        let error = normalize_partition_table_id(raw).unwrap_err();
        assert!(error.contains("partition table id"), "{raw}: {error}");
    }
}

#[test]
fn a_read_only_identity_accepts_only_a_removable_card_that_reports_a_serial() {
    let mut card = disk(2, "USB", 64 * 1024 * 1024, false, "Reader");
    card.serial = "  Reader-1 ".into();
    assert_eq!(
        accept_read_only_identity(&card).unwrap(),
        ("reader-1".into(), 64 * 1024 * 1024)
    );

    let mut sd = disk(3, "sd", 513, false, "Slot");
    sd.serial = "Slot-9".into();
    assert_eq!(
        accept_read_only_identity(&sd).unwrap(),
        ("slot-9".into(), 513)
    );

    let zero = disk(0, "USB", 64 * 1024 * 1024, false, "Reader");
    let error = accept_read_only_identity(&zero).unwrap_err();
    assert!(error.contains("disk 0"), "{error}");

    let mut system = disk(2, "USB", 64 * 1024 * 1024, true, "Reader");
    system.serial = "reader-1".into();
    let error = accept_read_only_identity(&system).unwrap_err();
    assert!(error.contains("system or boot"), "{error}");

    let mut fixed = disk(2, "SATA", 64 * 1024 * 1024, false, "Internal");
    fixed.serial = "internal".into();
    let error = accept_read_only_identity(&fixed).unwrap_err();
    assert!(error.contains("USB or SD"), "{error}");

    let mut empty = disk(2, "USB", 0, false, "Reader");
    empty.serial = "reader-1".into();
    let error = accept_read_only_identity(&empty).unwrap_err();
    assert!(error.contains("size is 0"), "{error}");

    let missing = disk(2, "USB", 64 * 1024 * 1024, false, "Reader");
    let error = accept_read_only_identity(&missing).unwrap_err();
    assert!(error.contains("serial"), "{error}");
}

#[test]
fn an_unreadable_card_defers_the_app_precheck_to_the_elevated_helper() {
    assert_eq!(app_precheck(None), AppPrecheck::Enforce);
    let error = "could not open disk 2 read-only (Access is denied. (os error 5)). If Windows refused access, run Card Studio as administrator and try again. The card was not written.";
    let AppPrecheck::Deferred { note } = app_precheck(Some(error)) else {
        panic!("an open failure must not block elevation");
    };
    assert!(note.contains("without admin"));
    assert!(note.contains("elevated helper"));
    assert!(note.contains("before it writes"));
    assert!(note.contains("Access is denied"));
    assert!(!note.contains("was not written"));
    assert!(!note.contains("Nothing on the card was overwritten"));
    assert!(!note.contains("The card was not written"));
}

#[test]
fn cancel_message_comes_from_the_exit_code_and_overwritten_line() {
    let clean = "\
RESULT=CANCELLED
OVERWRITTEN=no
SUMMARY=Cancelled. Nothing on the card was overwritten.
";
    let follow = interpret_helper_log(Some(9), clean, true, false).unwrap();
    assert!(matches!(
        follow,
        HelperFollowup::Parsed(ref log) if !log.overwritten && log.summary.contains("Nothing")
    ));

    let partial_yes = "OVERWRITTEN=yes\n";
    let follow = interpret_helper_log(Some(9), partial_yes, false, true).unwrap();
    match follow {
        HelperFollowup::Cancelled {
            overwritten,
            summary,
        } => {
            assert!(overwritten);
            assert!(summary.contains("Restore your backup"));
        }
        HelperFollowup::Parsed(_) => panic!("a partial log is not a finished report"),
    }

    let missing = "RESULT=CANCELLED\nSUMMARY=Cancelled.\n";
    let error = interpret_helper_log(Some(9), missing, true, true).unwrap_err();
    assert!(error.contains("did not say whether"));
    assert!(!error.contains("Nothing on the card was overwritten"));
    assert!(!error.contains("The card was not written"));

    let dismissed = interpret_helper_log(Some(1), "", false, false).unwrap_err();
    assert!(dismissed.contains("did not say whether"));
    assert!(!dismissed.contains("Nothing on the card was overwritten"));
    assert!(!dismissed.contains("The card was not written"));

    let failed = format_failed_log(false, "the disk model changed");
    let error = interpret_helper_log(Some(3), &failed, false, true).unwrap_err();
    assert!(error.contains("model changed"));
    assert!(failed.contains("OVERWRITTEN=no"));
}
