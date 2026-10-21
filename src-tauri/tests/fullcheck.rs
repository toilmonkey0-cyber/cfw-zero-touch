//! Full-card write and read-back. Devices are fixture files only.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cfw_zero_touch_lib::backup::{backup_image, BackupOutcome};
use cfw_zero_touch_lib::block_device::{is_physical_disk_path, FakeBlockDevice, FileBlockDevice};
use cfw_zero_touch_lib::fullcheck::{
    self, cancel_summary, format_full_log, interpret_full_log, pattern_block, plain_summary,
    progress_at, run_full_check, BadRange, FullReport, FullRun, FullStatus, FULL_BLOCK, FULL_SEED,
    FULL_WORD,
};
use cfw_zero_touch_lib::probe::{
    self, prefix_sha256, sample_block_hashes, write_identity, CardIdentity, ProbeBlock, ProbeDisk,
    ProbeError, ProbePlan, ProbeRequest, SessionBackup,
};
use cfw_zero_touch_lib::restore::signal_cancel_file;

#[allow(dead_code, unused_imports)]
#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::{fake_card, Scratch};

const FIXTURE_SERIAL: &str = "fixture-card";

/// Read-back of these block offsets is flipped after a pattern has been written.
struct CorruptBlocks<D> {
    inner: D,
    offsets: Vec<u64>,
}

impl<D: FakeBlockDevice> FakeBlockDevice for CorruptBlocks<D> {
    fn reported_len(&self) -> u64 {
        self.inner.reported_len()
    }

    fn stored_len(&self) -> u64 {
        self.inner.stored_len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_at(offset, buf)?;
        let end = offset + buf.len() as u64;
        for bad in &self.offsets {
            if *bad >= offset && *bad + FULL_BLOCK <= end {
                let index = (*bad - offset) as usize;
                if buf.get(index..index + 8) == Some(b"CFWFULL1".as_slice()) {
                    buf[index] ^= 0xff;
                }
            }
        }
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_at(offset, data)
    }
}

struct Record<D> {
    inner: D,
    writes: Vec<(u64, Vec<u8>)>,
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
        self.writes.push((offset, data.to_vec()));
        self.inner.write_at(offset, data)
    }
}

struct CancelOnWrite<D> {
    inner: D,
    cancel: std::sync::Arc<AtomicBool>,
    writes: u32,
}

/// Accepts writes and reports success, but leaves the previous bytes in place.
struct IgnoreWrites<D> {
    inner: D,
}

impl<D: FakeBlockDevice> FakeBlockDevice for IgnoreWrites<D> {
    fn reported_len(&self) -> u64 {
        self.inner.reported_len()
    }

    fn stored_len(&self) -> u64 {
        self.inner.stored_len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_at(offset, buf)
    }

    fn write_at(&mut self, _offset: u64, _data: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
}

/// `read_at` fails when the request covers one of the chosen block offsets.
///
/// The blank-card check reads before any write. Failures start after the first
/// write so they belong to the read-back, not the gate.
struct FailReads<D> {
    inner: D,
    offsets: Vec<u64>,
    armed: bool,
}

impl<D: FakeBlockDevice> FakeBlockDevice for FailReads<D> {
    fn reported_len(&self) -> u64 {
        self.inner.reported_len()
    }

    fn stored_len(&self) -> u64 {
        self.inner.stored_len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let end = offset + buf.len() as u64;
        if self.armed && self.offsets.iter().any(|bad| *bad >= offset && *bad < end) {
            return Err(std::io::Error::other("injected read failure"));
        }
        self.inner.read_at(offset, buf)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_at(offset, data)?;
        self.armed = true;
        Ok(())
    }
}

/// `write_at` fails when the request covers one of the chosen block offsets.
struct FailWrites<D> {
    inner: D,
    offsets: Vec<u64>,
}

impl<D: FakeBlockDevice> FakeBlockDevice for FailWrites<D> {
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
        let end = offset + data.len() as u64;
        if self.offsets.iter().any(|bad| *bad >= offset && *bad < end) {
            return Err(std::io::Error::other("injected write failure"));
        }
        self.inner.write_at(offset, data)
    }
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
        serial: FIXTURE_SERIAL.into(),
    }
}

fn plan_for(disk: &ProbeDisk) -> ProbePlan {
    ProbePlan {
        confirmation: FULL_WORD.into(),
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

fn run(
    device: &mut dyn FakeBlockDevice,
    disk: &ProbeDisk,
    backup: Option<&SessionBackup>,
    cancel: &AtomicBool,
) -> Result<FullStatus, ProbeError> {
    let mut progress = Vec::new();
    run_collect(device, disk, backup, cancel, None, &mut progress)
}

fn run_collect(
    device: &mut dyn FakeBlockDevice,
    disk: &ProbeDisk,
    backup: Option<&SessionBackup>,
    cancel: &AtomicBool,
    cancel_file: Option<&Path>,
    progress: &mut Vec<cfw_zero_touch_lib::fullcheck::FullProgress>,
) -> Result<FullStatus, ProbeError> {
    let plan = plan_for(disk);
    let request = ProbeRequest {
        disk,
        plan: &plan,
        backup,
    };
    let job = FullRun {
        request: &request,
        cancel,
        cancel_file,
        chunk_bytes: FULL_BLOCK,
    };
    run_full_check(device, &job, &mut |event| progress.push(event.clone()))
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

fn assert_blocked(error: ProbeError, block: ProbeBlock) {
    match error {
        ProbeError::Blocked(got) => assert_eq!(got, block),
        ProbeError::Failed { message, .. } => panic!("expected a refusal, got {message}"),
    }
}

#[test]
fn pattern_is_seeded_and_changes_with_position() {
    let first = pattern_block(FULL_SEED, 0);
    let next = pattern_block(FULL_SEED, FULL_BLOCK);
    let later = pattern_block(FULL_SEED, FULL_BLOCK * 8);
    assert_eq!(&first[..8], b"CFWFULL1");
    assert_eq!(
        u64::from_le_bytes(first[8..16].try_into().unwrap()),
        FULL_SEED
    );
    assert_ne!(first, next);
    assert_ne!(first, later);
    assert_ne!(first, pattern_block(FULL_SEED ^ 1, 0));
    assert_eq!(
        fullcheck::decode_pattern_offset(FULL_SEED, &next),
        Some(FULL_BLOCK)
    );
    assert_eq!(fullcheck::decode_pattern_offset(FULL_SEED, &first), Some(0));
    let mut flipped = next;
    flipped[0] ^= 0xff;
    assert_eq!(fullcheck::decode_pattern_offset(FULL_SEED, &flipped), None);
    assert_ne!(fullcheck::fresh_seed(), fullcheck::fresh_seed());
}

#[test]
fn three_spots_near_twelve_gigabytes_are_named_in_plain_language() {
    let ranges = [
        BadRange {
            offset: 12_000_000_000,
            length: FULL_BLOCK,
        },
        BadRange {
            offset: 12_000_001_024,
            length: FULL_BLOCK,
        },
        BadRange {
            offset: 12_000_002_048,
            length: FULL_BLOCK,
        },
    ];
    let summary = plain_summary(&ranges, ranges.len() as u64, 0, false, false);
    assert_eq!(
        summary,
        "Found 3 bad spots near 12 GB. Don't use this card. This card was fully erased. Format the card again before you use it."
    );
    let backed_up = plain_summary(&ranges, ranges.len() as u64, 0, true, false);
    assert!(backed_up.contains("Found 3 bad spots near 12 GB"));
    assert!(backed_up.contains("Don't use this card"));
    assert!(backed_up.contains("fully erased"));
    assert!(backed_up.contains("Restore your backup"));
    assert!(!backed_up.contains("Format the card again"));
}

#[test]
fn distant_spots_are_listed_and_a_clean_card_says_none() {
    let ranges = [
        BadRange {
            offset: 1_000_000,
            length: FULL_BLOCK,
        },
        BadRange {
            offset: 12_000_000_000,
            length: FULL_BLOCK,
        },
        BadRange {
            offset: 40_000_000_000,
            length: FULL_BLOCK * 2,
        },
    ];
    let summary = plain_summary(&ranges, ranges.len() as u64, 0, false, true);
    assert!(summary.contains("Found 3 bad spots near 1 MB, 12 GB, and 40 GB"));
    assert!(summary.contains("repeating"));
    assert!(summary.contains("fully erased"));
    let clean = plain_summary(&[], 0, 0, true, false);
    assert!(clean.starts_with("No bad spots."));
    assert!(clean.contains("fully erased"));
    assert!(clean.contains("Restore your backup"));
    assert!(!clean.contains("Don't use this card"));
}

#[test]
fn progress_reports_percent_speed_and_eta() {
    let midway = progress_at("write", 2_000_000, 8_000_000, Duration::from_secs(2));
    assert_eq!(midway.phase, "write");
    assert_eq!(midway.percent, 25);
    assert!((midway.mbps - 1.0).abs() < 0.05);
    assert_eq!(midway.eta_seconds, 6);
    let done = progress_at("read", 8_000_000, 8_000_000, Duration::from_secs(4));
    assert_eq!(done.percent, 100);
    assert_eq!(done.eta_seconds, 0);
    assert!((done.mbps - 2.0).abs() < 0.05);
    let line = fullcheck::format_progress(&midway);
    let parsed = fullcheck::parse_progress_line(&line).expect("progress line");
    assert_eq!(parsed.phase, "write");
    assert_eq!(parsed.percent, 25);
    assert_eq!(parsed.bytes_done, 2_000_000);
    assert_eq!(parsed.bytes_total, 8_000_000);
    assert_eq!(parsed.eta_seconds, 6);
    assert!((parsed.mbps - 1.0).abs() < 0.05);
    assert!(fullcheck::parse_progress_line("verify 1 2").is_none());
}

#[test]
fn a_clean_fixture_reports_zero_bad_spots() {
    let scratch = Scratch::new("full-clean");
    let size = 8 * 1024;
    let device = open_fixture(&scratch, &vec![0u8; size], size as u64);
    let mut device = Record {
        inner: device,
        writes: Vec::new(),
    };
    let card = disk(2, "USB", size as u64, false, "Generic SD");
    let mut progress = Vec::new();
    let status = run_collect(
        &mut device,
        &card,
        None,
        &AtomicBool::new(false),
        None,
        &mut progress,
    )
    .unwrap();
    match status {
        FullStatus::Finished(report) => {
            assert!(report.bad_ranges.is_empty(), "{:?}", report.bad_ranges);
            assert!(!report.aliased);
            assert!(report.summary.contains("No bad spots"));
            assert!(report.summary.contains("fully erased"));
            assert!(report.summary.contains("Format the card again"));
            assert!(report.write_mbps.is_finite() && report.write_mbps >= 0.0);
            assert!(report.read_mbps.is_finite() && report.read_mbps >= 0.0);
        }
        FullStatus::Cancelled { .. } => panic!("a clean card should finish"),
    }
    assert_aligned_cover(&device.writes, size as u64);
    assert!(progress.iter().any(|event| event.percent < 100));
    assert_eq!(progress.last().map(|event| event.percent), Some(100));
    assert!(progress.iter().any(|event| event.phase == "write"));
    assert!(progress.iter().any(|event| event.phase == "read"));
    assert!(progress
        .windows(2)
        .all(|pair| pair[0].bytes_done <= pair[1].bytes_done));
    assert!(progress
        .iter()
        .all(|event| event.mbps.is_finite() && event.eta_seconds < 100_000));
    assert_eq!(progress.last().map(|event| event.eta_seconds), Some(0));
}

#[test]
fn injected_corrupt_blocks_are_reported_as_exact_ranges() {
    let scratch = Scratch::new("full-corrupt");
    let size = 16 * 1024u64;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let bad = vec![512u64, 2048, 4096];
    let mut device = CorruptBlocks {
        inner: device,
        offsets: bad.clone(),
    };
    let card = disk(2, "SD", size, false, "Generic SD");
    let status = run(&mut device, &card, None, &AtomicBool::new(false)).unwrap();
    match status {
        FullStatus::Finished(report) => {
            let expected: Vec<BadRange> = bad
                .iter()
                .map(|offset| BadRange {
                    offset: *offset,
                    length: FULL_BLOCK,
                })
                .collect();
            assert_eq!(report.bad_ranges, expected);
            assert!(!report.aliased);
            assert!(report.summary.contains("Found 3 bad spots"));
            assert!(report.summary.contains("Don't use this card"));
            assert!(report.summary.contains("fully erased"));
        }
        FullStatus::Cancelled { .. } => panic!("corrupt read-back should finish"),
    }
}

#[test]
fn a_wrapping_card_is_caught_by_the_position_pattern() {
    let fake = fake_card();
    let scratch = Scratch::new("full-alias");
    let device = open_fixture(&scratch, &fake.bytes, fake.reported_len);
    let mut device = Record {
        inner: device,
        writes: Vec::new(),
    };
    let card = disk(2, "USB", fake.reported_len, false, "Fake SD");
    let status = run(&mut device, &card, None, &AtomicBool::new(false)).unwrap();
    match status {
        FullStatus::Finished(report) => {
            let bad_len = fake.reported_len - fake.stored_len();
            assert_eq!(
                report.bad_ranges,
                vec![BadRange {
                    offset: 0,
                    length: bad_len,
                }]
            );
            assert!(report.aliased);
            assert!(report.summary.contains("Don't use this card"));
            assert!(report.summary.contains("repeating"));
            assert!(report.summary.contains("fully erased"));
        }
        FullStatus::Cancelled { .. } => panic!("alias scan should finish"),
    }
    assert_aligned_cover(&device.writes, fake.reported_len);
}

#[test]
fn writes_never_pass_the_claimed_size() {
    let scratch = Scratch::new("full-bounds");
    let size = 4096u64;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let mut device = Record {
        inner: device,
        writes: Vec::new(),
    };
    let card = disk(2, "USB", size, false, "Generic SD");
    let status = run(&mut device, &card, None, &AtomicBool::new(false)).unwrap();
    let seed = match &status {
        FullStatus::Finished(report) => report.seed,
        FullStatus::Cancelled { summary, .. } => panic!("bounds run should finish: {summary}"),
    };
    assert_aligned_cover(&device.writes, size);
    let wide = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let mut wide = Record {
        inner: wide,
        writes: Vec::new(),
    };
    let plan = plan_for(&card);
    let request = ProbeRequest {
        disk: &card,
        plan: &plan,
        backup: None,
    };
    run_full_check(
        &mut wide,
        &FullRun {
            request: &request,
            cancel: &AtomicBool::new(false),
            cancel_file: None,
            chunk_bytes: 1536,
        },
        &mut |_| {},
    )
    .unwrap();
    assert_aligned_cover(&wide.writes, size);
    assert!(wide.writes.iter().any(|(_, data)| data.len() == 1536));
    assert!(wide.writes.last().unwrap().1.len() < 1536);
    for (offset, data) in &device.writes {
        let mut expect = vec![0u8; data.len()];
        let mut at = 0;
        while at < data.len() {
            let block = pattern_block(seed, *offset + at as u64);
            expect[at..at + FULL_BLOCK as usize].copy_from_slice(&block);
            at += FULL_BLOCK as usize;
        }
        assert_eq!(data, &expect);
    }
}

#[test]
fn the_full_check_uses_the_probe_gates_and_does_not_write_when_refused() {
    let size = 8192u64;
    let scratch = Scratch::new("full-gates");
    let mut leftover = vec![0u8; size as usize];
    leftover[600] = 0xab;
    let device = open_fixture(&scratch, &leftover, size);
    let before = std::fs::read(device.path()).unwrap();
    let mut device = Record {
        inner: device,
        writes: Vec::new(),
    };
    let card = disk(2, "USB", size, false, "Generic SD");
    let error = run(&mut device, &card, None, &AtomicBool::new(false)).unwrap_err();
    assert_blocked(error, ProbeBlock::NotBlank);
    assert!(device.writes.is_empty());
    assert_eq!(std::fs::read(device.inner.path()).unwrap(), before);

    let zeros = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let mut zeros = Record {
        inner: zeros,
        writes: Vec::new(),
    };
    for refused in [
        disk(0, "USB", size, false, "Generic SD"),
        disk(2, "USB", size, true, "Generic SD"),
        disk(3, "NVMe", size, false, "Internal"),
        disk(3, "SATA", size, false, "Internal"),
    ] {
        let error = run(&mut zeros, &refused, None, &AtomicBool::new(false)).unwrap_err();
        assert_blocked(error, ProbeBlock::NotRemovable);
    }
    let mut changed_size = plan_for(&card);
    changed_size.displayed_bytes = size - FULL_BLOCK;
    let request = ProbeRequest {
        disk: &card,
        plan: &changed_size,
        backup: None,
    };
    let error = run_full_check(
        &mut zeros,
        &FullRun {
            request: &request,
            cancel: &AtomicBool::new(false),
            cancel_file: None,
            chunk_bytes: FULL_BLOCK,
        },
        &mut |_| {},
    )
    .unwrap_err();
    assert_blocked(error, ProbeBlock::Size);

    let mut changed_model = plan_for(&card);
    changed_model.displayed_model = "Other".into();
    let request = ProbeRequest {
        disk: &card,
        plan: &changed_model,
        backup: None,
    };
    let error = run_full_check(
        &mut zeros,
        &FullRun {
            request: &request,
            cancel: &AtomicBool::new(false),
            cancel_file: None,
            chunk_bytes: FULL_BLOCK,
        },
        &mut |_| {},
    )
    .unwrap_err();
    assert_blocked(error, ProbeBlock::Model);

    let mut word = plan_for(&card);
    word.confirmation = "PROBE".into();
    let request = ProbeRequest {
        disk: &card,
        plan: &word,
        backup: None,
    };
    let error = run_full_check(
        &mut zeros,
        &FullRun {
            request: &request,
            cancel: &AtomicBool::new(false),
            cancel_file: None,
            chunk_bytes: FULL_BLOCK,
        },
        &mut |_| {},
    )
    .unwrap_err();
    assert_blocked(error, ProbeBlock::Confirmation);
    assert!(zeros.writes.is_empty());
    assert!(std::fs::read(zeros.inner.path())
        .unwrap()
        .iter()
        .all(|byte| *byte == 0));
}

#[test]
fn a_verified_backup_matches_serial_prefix_and_samples_or_the_card_is_not_written() {
    let scratch = Scratch::new("full-backup");
    let size = 8192usize;
    let model = "Generic SD";
    let mut bytes = vec![0u8; size];
    bytes[0] = 0x11;
    bytes[510] = 0x55;
    bytes[511] = 0xaa;
    let backup = finished_backup(scratch.path(), &bytes, model);
    let device = open_fixture(&scratch, &bytes, size as u64);
    let mut device = Record {
        inner: device,
        writes: Vec::new(),
    };
    let card = disk(2, "USB", size as u64, false, model);
    let status = run(&mut device, &card, Some(&backup), &AtomicBool::new(false)).unwrap();
    match status {
        FullStatus::Finished(report) => {
            assert!(report.bad_ranges.is_empty());
            assert!(report.summary.contains("Restore your backup"));
            assert!(report.summary.contains("fully erased"));
        }
        FullStatus::Cancelled { .. } => panic!("matching backup should finish"),
    }
    assert!(!device.writes.is_empty());

    let mut other = bytes.clone();
    other[size - 1] = 0x5a;
    let live = open_fixture(&scratch, &other, size as u64);
    let before = std::fs::read(live.path()).unwrap();
    let mut live = Record {
        inner: live,
        writes: Vec::new(),
    };
    let error = run(&mut live, &card, Some(&backup), &AtomicBool::new(false)).unwrap_err();
    assert_blocked(error, ProbeBlock::Backup);
    assert!(live.writes.is_empty());
    assert_eq!(std::fs::read(live.inner.path()).unwrap(), before);

    let mut wrong_serial = card.clone();
    wrong_serial.serial = "other-card".into();
    let again = open_fixture(&scratch, &bytes, size as u64);
    let before = std::fs::read(again.path()).unwrap();
    let mut again = Record {
        inner: again,
        writes: Vec::new(),
    };
    let error = run(
        &mut again,
        &wrong_serial,
        Some(&backup),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_blocked(error, ProbeBlock::Backup);
    assert!(again.writes.is_empty());
    assert_eq!(std::fs::read(again.inner.path()).unwrap(), before);
}

#[test]
fn cancel_before_a_write_says_nothing_was_overwritten() {
    let scratch = Scratch::new("full-cancel-early");
    let size = 8192u64;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let before = std::fs::read(device.path()).unwrap();
    let mut device = Record {
        inner: device,
        writes: Vec::new(),
    };
    let card = disk(2, "USB", size, false, "Generic SD");
    let status = run(&mut device, &card, None, &AtomicBool::new(true)).unwrap();
    match status {
        FullStatus::Cancelled {
            overwritten,
            summary,
        } => {
            assert!(!overwritten);
            assert_eq!(summary, cancel_summary(false, false));
            assert!(summary.contains("Nothing"));
            assert!(!summary.contains("fully erased"));
        }
        FullStatus::Finished(_) => panic!("cancel before writing must not finish"),
    }
    assert!(device.writes.is_empty());
    assert_eq!(std::fs::read(device.inner.path()).unwrap(), before);
}

#[test]
fn cancel_after_a_write_is_honest_that_data_was_overwritten() {
    let scratch = Scratch::new("full-cancel-late");
    let size = 8192u64;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let mut device = CancelOnWrite {
        inner: device,
        cancel: std::sync::Arc::clone(&cancel),
        writes: 0,
    };
    let card = disk(2, "SD", size, false, "Generic SD");
    let status = run(&mut device, &card, None, &cancel).unwrap();
    match status {
        FullStatus::Cancelled {
            overwritten,
            summary,
        } => {
            assert!(overwritten);
            assert_eq!(summary, cancel_summary(true, false));
            assert!(summary.contains("overwritten") || summary.contains("erased"));
            assert!(!summary.contains("Nothing"));
            assert!(summary.contains("Format the card again"));
        }
        FullStatus::Finished(_) => panic!("cancel after a write must not look finished"),
    }
    assert!(device.writes >= 1);
    let backed_up = cancel_summary(true, true);
    assert!(backed_up.contains("Restore your backup"));
    assert!(!backed_up.contains("Nothing"));
}

#[test]
fn a_cancel_file_before_writing_leaves_the_card_unchanged() {
    let scratch = Scratch::new("full-cancel-file");
    let size = 8192u64;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let before = std::fs::read(device.path()).unwrap();
    let flag = scratch.path().join("cancel.txt");
    signal_cancel_file(&flag).unwrap();
    let mut device = Record {
        inner: device,
        writes: Vec::new(),
    };
    let card = disk(2, "USB", size, false, "Generic SD");
    let mut progress = Vec::new();
    let status = run_collect(
        &mut device,
        &card,
        None,
        &AtomicBool::new(false),
        Some(flag.as_path()),
        &mut progress,
    )
    .unwrap();
    match status {
        FullStatus::Cancelled { overwritten, .. } => assert!(!overwritten),
        FullStatus::Finished(_) => panic!("cancel file should stop the test"),
    }
    assert!(device.writes.is_empty());
    assert_eq!(std::fs::read(device.inner.path()).unwrap(), before);
}

#[test]
fn every_exit_records_whether_the_card_was_overwritten() {
    let finished = format_full_log(&FullStatus::Finished(FullReport {
        claimed_bytes: 8192,
        seed: 7,
        bad_ranges: vec![BadRange {
            offset: 512,
            length: 512,
        }],
        range_count: 1,
        bad_bytes: 512,
        aliased: false,
        write_mbps: 1.5,
        read_mbps: 2.5,
        summary: plain_summary(
            &[BadRange {
                offset: 512,
                length: 512,
            }],
            1,
            512,
            true,
            false,
        ),
    }));
    assert!(finished.contains("OVERWRITTEN=yes"));
    assert!(finished.contains("RESULT=OK"));
    assert!(finished.contains("SEED=7"));
    assert!(finished.contains("BAD_BYTES=512"));
    assert!(finished.contains("BAD_COUNT=1"));
    assert!(finished.contains("BAD=512:512"));
    assert!(probe::overwritten_flag(&finished).unwrap());
    let parsed = interpret_full_log(Some(0), &finished, false, true).unwrap();
    assert_eq!(parsed.status, "finished");
    assert!(parsed.overwritten);
    assert_eq!(parsed.bad_ranges.len(), 1);
    assert_eq!(parsed.bad_ranges[0].offset, 512);
    assert_eq!(parsed.seed, 7);
    assert_eq!(parsed.bad_bytes, 512);
    assert_eq!(parsed.range_count, 1);

    let cancelled_yes = format_full_log(&FullStatus::Cancelled {
        overwritten: true,
        summary: cancel_summary(true, true),
    });
    assert!(cancelled_yes.contains("OVERWRITTEN=yes"));
    assert!(cancelled_yes.contains("RESULT=CANCELLED"));
    let cancelled_no = format_full_log(&FullStatus::Cancelled {
        overwritten: false,
        summary: cancel_summary(false, false),
    });
    assert!(cancelled_no.contains("OVERWRITTEN=no"));

    let refused = probe::format_failed_log(false, "refused before a write");
    assert!(refused.contains("OVERWRITTEN=no"));
    let failed = probe::format_failed_log(true, "write failed");
    assert!(failed.contains("OVERWRITTEN=yes"));

    let missing = "RESULT=CANCELLED\nSUMMARY=Cancelled.\n";
    let error = interpret_full_log(Some(9), missing, true, true).unwrap_err();
    assert!(error.contains("did not say whether"));
    assert!(!error.contains("Nothing on the card was overwritten"));
    let follow = probe::interpret_helper_log(Some(9), missing, true, true).unwrap_err();
    assert_eq!(error, follow);
}

#[test]
fn helper_launch_uses_full_check_and_refuses_a_physical_path() {
    let tool = Path::new("cfw-flash.exe");
    let cancel = Path::new("C:\\Temp\\cfw-full-cancel.txt");
    let log = Path::new("C:\\Temp\\cfw-full-log.txt");
    let progress = Path::new("C:\\Temp\\cfw-full-progress.txt");
    let backup = Path::new("C:\\Backups\\stock.img.gz");
    let script = probe::launch_command(
        tool,
        "--full-check",
        &probe::HelperRequest {
            disk_number: 2,
            confirmation: FULL_WORD.into(),
            displayed_bytes: 64_000_000_000,
            displayed_model: "Generic SD".into(),
            backup_image: Some(backup.into()),
            backup_sha256: Some("ab".repeat(32)),
            backup_prefix_sha256: Some("cd".repeat(32)),
            backup_serial: Some(FIXTURE_SERIAL.into()),
            cancel_file: cancel.into(),
            log_file: log.into(),
        },
        Some(progress),
    )
    .unwrap();
    assert!(script.contains("'--full-check'"));
    assert!(script.contains("'--confirm=FULL'"));
    assert!(script.contains("'--progress-file="));
    assert!(script.contains("Start-Process"));
    assert!(!script.contains("Clear-Disk"));
    assert!(!script.contains("'--probe'"));
    assert!(!script.to_ascii_lowercase().contains("physicaldrive"));

    let error = probe::launch_command(
        tool,
        "--full-check",
        &probe::HelperRequest {
            disk_number: 2,
            confirmation: FULL_WORD.into(),
            displayed_bytes: 1024,
            displayed_model: "Generic SD".into(),
            backup_image: Some(PathBuf::from(r"\\.\PhysicalDrive1")),
            backup_sha256: Some("ab".repeat(32)),
            backup_prefix_sha256: Some("cd".repeat(32)),
            backup_serial: Some(FIXTURE_SERIAL.into()),
            cancel_file: cancel.into(),
            log_file: log.into(),
        },
        Some(progress),
    )
    .unwrap_err();
    assert!(error.to_lowercase().contains("physical"));
}

#[test]
fn a_card_that_ignores_writes_and_keeps_a_previous_pattern_is_bad() {
    let scratch = Scratch::new("full-ignore-writes");
    let size = 8192u64;
    let mut bytes = vec![0u8; size as usize];
    let mut at = 0;
    while at < bytes.len() {
        let block = pattern_block(FULL_SEED, at as u64);
        bytes[at..at + FULL_BLOCK as usize].copy_from_slice(&block);
        at += FULL_BLOCK as usize;
    }
    let model = "Generic SD";
    let backup = finished_backup(scratch.path(), &bytes, model);
    let device = open_fixture(&scratch, &bytes, size);
    let mut device = IgnoreWrites { inner: device };
    let card = disk(2, "USB", size, false, model);
    let status = run(&mut device, &card, Some(&backup), &AtomicBool::new(false))
        .expect("a stuck previous pattern is a finished bad report");
    match status {
        FullStatus::Finished(report) => {
            assert_eq!(
                report.bad_ranges,
                vec![BadRange {
                    offset: 0,
                    length: size,
                }]
            );
            assert!(report.summary.contains("Don't use this card"));
            assert!(!report.summary.contains("No bad spots"));
            let log = format_full_log(&FullStatus::Finished(report));
            let seed_line = log
                .lines()
                .find(|line| line.starts_with("SEED="))
                .expect("each run logs its seed");
            let seed: u64 = seed_line
                .trim_start_matches("SEED=")
                .parse()
                .expect("seed is a number");
            assert_ne!(seed, FULL_SEED);
            assert!(log.contains(&format!("SEED={seed}")));
        }
        FullStatus::Cancelled { summary, .. } => {
            panic!("ignored writes must not pass or cancel: {summary}")
        }
    }
}

#[test]
fn a_read_error_at_chosen_offsets_is_a_bad_range_and_the_scan_continues() {
    let scratch = Scratch::new("full-read-errors");
    let size = 8192u64;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let bad = vec![512u64, 4096];
    let mut device = FailReads {
        inner: device,
        offsets: bad.clone(),
        armed: false,
    };
    let card = disk(2, "SD", size, false, "Generic SD");
    let plan = plan_for(&card);
    let request = ProbeRequest {
        disk: &card,
        plan: &plan,
        backup: None,
    };
    let status = run_full_check(
        &mut device,
        &FullRun {
            request: &request,
            cancel: &AtomicBool::new(false),
            cancel_file: None,
            chunk_bytes: 2048,
        },
        &mut |_| {},
    )
    .expect("read errors under the cap are bad ranges, not an abort");
    match status {
        FullStatus::Finished(report) => {
            assert_eq!(
                report.bad_ranges,
                vec![
                    BadRange {
                        offset: 512,
                        length: FULL_BLOCK,
                    },
                    BadRange {
                        offset: 4096,
                        length: FULL_BLOCK,
                    },
                ]
            );
            assert!(report.summary.contains("Found 2 bad spots"));
            assert!(report.summary.contains("fully erased"));
            assert!(!report.summary.contains("Cancelled"));
        }
        FullStatus::Cancelled { summary, .. } => panic!("read errors must not cancel: {summary}"),
    }
}

#[test]
fn a_write_error_at_one_block_is_a_bad_range_and_the_scan_continues() {
    let scratch = Scratch::new("full-write-errors");
    let size = 8192u64;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let mut device = FailWrites {
        inner: device,
        offsets: vec![1024],
    };
    let card = disk(2, "USB", size, false, "Generic SD");
    let plan = plan_for(&card);
    let request = ProbeRequest {
        disk: &card,
        plan: &plan,
        backup: None,
    };
    let status = run_full_check(
        &mut device,
        &FullRun {
            request: &request,
            cancel: &AtomicBool::new(false),
            cancel_file: None,
            chunk_bytes: 2048,
        },
        &mut |_| {},
    )
    .expect("one write error is a bad range, not an abort");
    match status {
        FullStatus::Finished(report) => {
            assert_eq!(
                report.bad_ranges,
                vec![BadRange {
                    offset: 1024,
                    length: FULL_BLOCK,
                }]
            );
            assert!(!report.summary.contains("Cancelled"));
            assert!(report.summary.contains("fully erased"));
        }
        FullStatus::Cancelled { summary, .. } => panic!("write errors must not cancel: {summary}"),
    }
}

#[test]
fn too_many_read_errors_stop_without_saying_cancelled() {
    let scratch = Scratch::new("full-error-cap");
    let blocks = 1088u64;
    let size = blocks * FULL_BLOCK;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let offsets: Vec<u64> = (0..blocks).map(|index| index * FULL_BLOCK).collect();
    let mut device = FailReads {
        inner: device,
        offsets,
        armed: false,
    };
    let card = disk(2, "USB", size, false, "Generic SD");
    let error = run(&mut device, &card, None, &AtomicBool::new(false))
        .expect_err("a card that errors on every block stops");
    match error {
        ProbeError::Failed {
            overwritten,
            message,
        } => {
            assert!(overwritten);
            assert!(message.contains("stopped because of an error"));
            assert!(message.contains("Format the card again"));
            assert!(!message.contains("Cancelled"));
            assert!(!message.contains("Nothing"));
        }
        ProbeError::Blocked(block) => panic!("expected an error stop, got {}", block.message()),
    }
}

#[test]
fn only_the_first_ranges_are_reported_with_the_total_bad_bytes() {
    let scratch = Scratch::new("full-range-cap");
    let spots = 40u64;
    let size = spots * FULL_BLOCK * 2;
    let device = open_fixture(&scratch, &vec![0u8; size as usize], size);
    let offsets: Vec<u64> = (0..spots).map(|index| index * FULL_BLOCK * 2).collect();
    let mut device = CorruptBlocks {
        inner: device,
        offsets,
    };
    let card = disk(2, "SD", size, false, "Generic SD");
    let status = run(&mut device, &card, None, &AtomicBool::new(false)).unwrap();
    match status {
        FullStatus::Finished(report) => {
            assert_eq!(report.bad_ranges.len(), 32);
            assert_eq!(report.bad_ranges[0].offset, 0);
            assert_eq!(report.bad_ranges[0].length, FULL_BLOCK);
            assert!(report.summary.contains("Found 40 bad spots"));
            assert!(report.summary.contains("Showing the first 32"));
            assert!(report.summary.contains("20.5 KB"));
            let log = format_full_log(&FullStatus::Finished(report));
            assert_eq!(
                log.lines().filter(|line| line.starts_with("BAD=")).count(),
                32
            );
            assert!(log.contains(&format!("BAD_BYTES={}", spots * FULL_BLOCK)));
            assert!(log.contains("BAD_COUNT=40"));
        }
        FullStatus::Cancelled { summary, .. } => {
            panic!("a capped report still finishes: {summary}")
        }
    }
}

fn assert_aligned_cover(writes: &[(u64, Vec<u8>)], claimed: u64) {
    assert!(!writes.is_empty());
    let mut cursor = 0u64;
    for (offset, data) in writes {
        assert_eq!(*offset, cursor);
        assert_eq!(*offset % FULL_BLOCK, 0);
        assert_eq!(data.len() as u64 % FULL_BLOCK, 0);
        assert!(offset.saturating_add(data.len() as u64) <= claimed);
        cursor += data.len() as u64;
    }
    assert_eq!(cursor, claimed);
}
