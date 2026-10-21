//! Quick probe for a counterfeit or failing SD card.
//!
//! The probe writes a short, position-tagged pattern at offsets spread across
//! the size the card claims, including the last block, then reads those blocks
//! back. A fake card that aliases past its real NAND returns another offset's
//! pattern. The same run times a short sequential read and write.
//!
//! Nothing here opens `\\.\PhysicalDriveN`. The elevated helper does that,
//! after the same size and model recheck U4 uses, and only when this module
//! authorizes the disk. Tests pass a [`FakeBlockDevice`].

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::backup::{self, BackupBlock, BackupDisk};
use crate::block_device::{is_physical_disk_path, FakeBlockDevice};
use crate::restore::{self, RestoreError};

/// Pattern block. Samples and the sequential window stay on this boundary.
pub const PROBE_BLOCK: u64 = 512;

/// Leading bytes that must be zero, along with sector 0, the GPT header, and
/// every sample, before a card with no backup may be tested.
pub const BLANK_HEAD: u64 = 1024 * 1024;

/// How much of the card is hashed into the backup's card identity.
pub const PREFIX_IDENTITY_BYTES: u64 = 4 * 1024 * 1024;

const SEQUENTIAL_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeDisk {
    pub number: u32,
    pub bus: String,
    pub size_bytes: u64,
    pub system_disk: bool,
    pub model: String,
    /// Fresh disk serial or UniqueId. Empty when the disk does not report one.
    pub serial: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbePlan {
    pub confirmation: String,
    pub disk_number: u32,
    pub displayed_bytes: u64,
    pub displayed_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeBlock {
    Confirmation,
    NotRemovable,
    Size,
    Identity,
    Model,
    Backup,
    NotBlank,
}

impl ProbeBlock {
    pub fn message(self) -> &'static str {
        match self {
            Self::Confirmation => {
                "type PROBE to test the card shown, including its size and model"
            }
            Self::NotRemovable => {
                "only a removable USB or SD card can be tested, and system or boot disks are refused"
            }
            Self::Size => {
                "refusing a disk whose size is 0 or no longer matches the size on screen"
            }
            Self::Identity => "the selected disk changed before the test",
            Self::Model => {
                "the disk model on screen does not match this disk; refresh and confirm the model shown"
            }
            Self::Backup => {
                "the backup does not match this card. The size, model, serial, or contents differ. The card was not written."
            }
            Self::NotBlank => {
                "this card has data on it. Back up this exact card in this session before testing it. The card was not written."
            }
        }
    }
}

#[derive(Debug)]
pub enum ProbeError {
    Blocked(ProbeBlock),
    Failed { message: String, overwritten: bool },
}

fn fail(overwritten: bool, message: impl Into<String>) -> ProbeError {
    ProbeError::Failed {
        message: message.into(),
        overwritten,
    }
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(block) => formatter.write_str(block.message()),
            Self::Failed { message, .. } => formatter.write_str(message),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Genuine,
    Fake,
    Failing,
}

impl CardKind {
    fn label(self) -> &'static str {
        match self {
            Self::Genuine => "genuine",
            Self::Fake => "fake",
            Self::Failing => "failing",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReport {
    pub kind: CardKind,
    pub claimed_bytes: u64,
    pub real_bytes: u64,
    pub read_mbps: f64,
    pub write_mbps: f64,
    pub aliased: bool,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProbeStatus {
    Finished(ProbeReport),
    Cancelled { overwritten: bool, summary: String },
}

/// sha256 of one 512-byte block the probe will overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleHash {
    pub offset: u64,
    pub sha256: String,
}

/// Identity captured when this process backed the card up.
///
/// `bytes` and `model` are the disk at backup time. `prefix_sha256` is the
/// first [`PREFIX_IDENTITY_BYTES`] of that card. `samples` are the blocks at
/// [`sample_offsets`] for that size, which are the blocks the probe overwrites.
/// `serial` is Windows `UniqueId`, or `SerialNumber` when UniqueId is empty.
/// An empty serial means the disk did not report one. USB readers often report
/// their own serial, so the sample hashes distinguish two cards that share a
/// reader and the same first 4 MiB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardIdentity {
    pub serial: String,
    pub prefix_sha256: String,
    pub bytes: u64,
    pub model: String,
    pub samples: Vec<SampleHash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBackup {
    pub image: PathBuf,
    pub size_bytes: u64,
    pub model: String,
    pub sha256: String,
    pub identity: CardIdentity,
}

#[derive(Debug, Default)]
pub struct BackupSession {
    records: Vec<SessionBackup>,
}

impl BackupSession {
    pub fn remember(&mut self, backup: SessionBackup) {
        self.records.push(backup);
    }

    /// Latest backup from this process whose size and model match the card.
    pub fn find(&self, size_bytes: u64, model: &str) -> Option<&SessionBackup> {
        self.records.iter().rev().find(|record| {
            record.size_bytes == size_bytes && restore::same_model(&record.model, model)
        })
    }
}

pub struct ProbeRequest<'a> {
    pub disk: &'a ProbeDisk,
    pub plan: &'a ProbePlan,
    pub backup: Option<&'a SessionBackup>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeLog {
    pub status: String,
    pub kind: String,
    pub claimed_bytes: u64,
    pub real_bytes: u64,
    pub read_mbps: f64,
    pub write_mbps: f64,
    pub overwritten: bool,
    pub summary: String,
}

/// Offsets spread across `claimed`, always including the first and last block.
pub fn sample_offsets(claimed: u64) -> Vec<u64> {
    if claimed < PROBE_BLOCK {
        return Vec::new();
    }
    let mut offsets = vec![0];
    let step = (claimed / 8) / PROBE_BLOCK * PROBE_BLOCK;
    if step >= PROBE_BLOCK {
        let mut at = step;
        while at + PROBE_BLOCK <= claimed && offsets.len() < 16 {
            offsets.push(at);
            at += step;
        }
    }
    let end = claimed - PROBE_BLOCK;
    if !offsets.contains(&end) {
        offsets.push(end);
    }
    offsets
}

fn is_probe_bus(bus: &str) -> bool {
    let bus = bus.trim();
    bus.eq_ignore_ascii_case("USB") || bus.eq_ignore_ascii_case("SD")
}

/// Typed confirmation, USB or SD bus, and the U3 removable/system/boot/size gate.
///
/// [`backup::authorize_backup`] only names a USB bus. An SD slot is the same
/// removable class, so it is accepted here and then handed to that gate as USB
/// after the real bus has been checked. System, boot, disk 0, and a size that
/// changed since the screen was drawn still come from `authorize_backup`.
pub fn authorize_probe(disk: &ProbeDisk, plan: &ProbePlan) -> Result<(), ProbeBlock> {
    if plan.confirmation != "PROBE" {
        return Err(ProbeBlock::Confirmation);
    }
    if disk.number != plan.disk_number {
        return Err(ProbeBlock::Identity);
    }
    if !restore::same_model(&plan.displayed_model, &disk.model) {
        return Err(ProbeBlock::Model);
    }
    if !is_probe_bus(&disk.bus) {
        return Err(ProbeBlock::NotRemovable);
    }
    if disk.size_bytes == 0 || !disk.size_bytes.is_multiple_of(PROBE_BLOCK) {
        return Err(ProbeBlock::Size);
    }
    let backup_disk = BackupDisk {
        number: disk.number,
        bus: "USB".into(),
        size_bytes: disk.size_bytes,
        system_disk: disk.system_disk || disk.number == 0,
        displayed_bytes: plan.displayed_bytes,
    };
    backup::authorize_backup(&backup_disk).map_err(|block| match block {
        BackupBlock::NotRemovable => ProbeBlock::NotRemovable,
        BackupBlock::Size => ProbeBlock::Size,
    })
}

/// A card is blank only when sector 0, the GPT header sector, the first 1 MiB,
/// and every sample block are all zeros.
///
/// A missing partition table is not enough. Leftover bytes with no partition
/// entry are still data.
pub fn card_is_blank(device: &mut dyn FakeBlockDevice) -> Result<bool, ProbeError> {
    let len = device.reported_len();
    if len < PROBE_BLOCK * 2 || !len.is_multiple_of(PROBE_BLOCK) {
        return Ok(false);
    }
    if !span_is_zero(device, 0, PROBE_BLOCK)? {
        return Ok(false);
    }
    if !span_is_zero(device, PROBE_BLOCK, PROBE_BLOCK)? {
        return Ok(false);
    }
    if !span_is_zero(device, 0, len.min(BLANK_HEAD))? {
        return Ok(false);
    }
    let offsets = sample_offsets(len);
    regions_all_zero(device, &offsets)
}

fn span_is_zero(
    device: &mut dyn FakeBlockDevice,
    offset: u64,
    len: u64,
) -> Result<bool, ProbeError> {
    let mut buf = vec![0u8; 64 * 1024];
    let end = offset.saturating_add(len);
    let mut at = offset;
    while at < end {
        let n = ((end - at) as usize).min(buf.len());
        device
            .read_at(at, &mut buf[..n])
            .map_err(|error| fail(false, format!("could not read the card at {at}: {error}")))?;
        if buf[..n].iter().any(|byte| *byte != 0) {
            return Ok(false);
        }
        at += n as u64;
    }
    Ok(true)
}

fn regions_all_zero(device: &mut dyn FakeBlockDevice, offsets: &[u64]) -> Result<bool, ProbeError> {
    let mut buf = vec![0u8; PROBE_BLOCK as usize];
    for offset in offsets {
        device.read_at(*offset, &mut buf).map_err(|error| {
            fail(
                false,
                format!("could not read the card at {offset}: {error}"),
            )
        })?;
        if buf.iter().any(|byte| *byte != 0) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn overwrite_notice(backed_up: bool) -> &'static str {
    if backed_up {
        "Part of this card was overwritten. Restore your backup before you use this card."
    } else {
        "Part of this card was overwritten. Format the card again before you use this card."
    }
}

pub fn plain_summary(
    kind: CardKind,
    claimed: u64,
    real: u64,
    write_mbps: f64,
    read_mbps: f64,
    backed_up: bool,
) -> String {
    let claimed_label = format_capacity(claimed);
    let real_label = format_capacity(real);
    let speeds = format!(
        "Sequential write is {} and sequential read is {}.",
        format_mbps(write_mbps),
        format_mbps(read_mbps)
    );
    let lead = match kind {
        CardKind::Fake => format!(
            "This card is fake: it says {claimed_label} but only holds about {real_label}."
        ),
        CardKind::Genuine => format!(
            "This card looks genuine: it holds {claimed_label}, the same size it reports."
        ),
        CardKind::Failing => format!(
            "This card is failing: it says {claimed_label} but only about {real_label} could be read back."
        ),
    };
    format!("{lead} {speeds} {}", overwrite_notice(backed_up))
}

pub fn cancel_summary(overwritten: bool, backed_up: bool) -> String {
    if !overwritten {
        return "Cancelled. Nothing on the card was overwritten.".into();
    }
    if backed_up {
        "Cancelled. Data on the card was overwritten. Restore your backup before you use this card."
            .into()
    } else {
        "Cancelled. Data on the card was overwritten. Format the card again before you use this card."
            .into()
    }
}

pub fn unknown_overwrite_summary() -> &'static str {
    "Cancelled. The log did not say whether the card was overwritten. Restore your backup, or format the card, before you use it."
}

pub fn run_probe(
    device: &mut dyn FakeBlockDevice,
    request: &ProbeRequest<'_>,
    cancel: &AtomicBool,
    cancel_file: Option<&Path>,
) -> Result<ProbeStatus, ProbeError> {
    let backed_up = request.backup.is_some();
    authorize_probe(request.disk, request.plan).map_err(ProbeError::Blocked)?;
    if device.reported_len() != request.disk.size_bytes {
        return Err(ProbeError::Blocked(ProbeBlock::Size));
    }
    if let Some(backup) = request.backup {
        verify_session_backup(backup, request.disk, device, cancel)?;
    } else if !card_is_blank(device)? {
        return Err(ProbeError::Blocked(ProbeBlock::NotBlank));
    }
    if stop_requested(cancel, cancel_file, false)? {
        return Ok(cancelled(false, backed_up));
    }

    let claimed = device.reported_len();
    let offsets = sample_offsets(claimed);
    if offsets.is_empty() {
        return Err(ProbeError::Blocked(ProbeBlock::Size));
    }
    let mut overwritten = false;
    for offset in &offsets {
        if stop_requested(cancel, cancel_file, overwritten)? {
            return Ok(cancelled(overwritten, backed_up));
        }
        let block = pattern(*offset);
        // A failed write may already have landed. Say so.
        overwritten = true;
        device.write_at(*offset, &block).map_err(|error| {
            fail(
                true,
                format!(
                    "could not write the test pattern at {offset}: {error}. {}",
                    overwrite_notice(backed_up)
                ),
            )
        })?;
    }
    if stop_requested(cancel, cancel_file, overwritten)? {
        return Ok(cancelled(overwritten, backed_up));
    }

    let mut samples = Vec::with_capacity(offsets.len());
    let mut buf = vec![0u8; PROBE_BLOCK as usize];
    for offset in &offsets {
        if stop_requested(cancel, cancel_file, overwritten)? {
            return Ok(cancelled(overwritten, backed_up));
        }
        device.read_at(*offset, &mut buf).map_err(|error| {
            fail(
                true,
                format!(
                    "could not read the test pattern at {offset}: {error}. {}",
                    overwrite_notice(backed_up)
                ),
            )
        })?;
        samples.push((*offset, decode_tag(&buf)));
    }
    let (kind, real_bytes, aliased) = judge(claimed, &samples);

    if stop_requested(cancel, cancel_file, overwritten)? {
        return Ok(cancelled(overwritten, backed_up));
    }
    let (write_mbps, read_mbps) =
        match measure_speed(device, cancel, cancel_file, &mut overwritten, backed_up) {
            Ok(speeds) => speeds,
            Err(ProbeError::Failed {
                message,
                overwritten,
            }) if message == "probe-cancelled" => {
                return Ok(cancelled(overwritten, backed_up));
            }
            Err(error) => return Err(error),
        };
    let summary = plain_summary(kind, claimed, real_bytes, write_mbps, read_mbps, backed_up);
    Ok(ProbeStatus::Finished(ProbeReport {
        kind,
        claimed_bytes: claimed,
        real_bytes,
        read_mbps,
        write_mbps,
        aliased,
        summary,
    }))
}

fn cancelled(overwritten: bool, backed_up: bool) -> ProbeStatus {
    ProbeStatus::Cancelled {
        overwritten,
        summary: cancel_summary(overwritten, backed_up),
    }
}

/// The recorded backup must be this card: size, model, serial, prefix hash,
/// sample-block hashes, gzip bytes, and sidecar. Size and model alone are
/// not accepted.
pub fn verify_session_backup(
    backup: &SessionBackup,
    disk: &ProbeDisk,
    device: &mut dyn FakeBlockDevice,
    cancel: &AtomicBool,
) -> Result<(), ProbeError> {
    if is_physical_disk_path(&backup.image) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    let stored = match load_identity(&backup.image) {
        Ok(identity) => identity,
        Err(_) => return Err(ProbeError::Blocked(ProbeBlock::Backup)),
    };
    if !same_identity(&stored, &backup.identity)
        || stored.bytes != backup.size_bytes
        || !restore::same_model(&stored.model, &backup.model)
    {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    if stored.bytes != disk.size_bytes || !restore::same_model(&stored.model, &disk.model) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    if normalize_serial(&stored.serial) != normalize_serial(&disk.serial) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    let image_prefix = match gzip_prefix_sha256(&backup.image, stored.bytes) {
        Ok(hash) => hash,
        Err(_) => return Err(ProbeError::Blocked(ProbeBlock::Backup)),
    };
    if !restore::hashes_match(&image_prefix, &stored.prefix_sha256) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    let image_samples = match gzip_sample_hashes(&backup.image, stored.bytes) {
        Ok(samples) => samples,
        Err(_) => return Err(ProbeError::Blocked(ProbeBlock::Backup)),
    };
    if !samples_match(&stored.samples, &image_samples) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    live_identity_matches(device, &stored)?;
    let inspected = match restore::inspect_backup(&backup.image, cancel, |_| {}) {
        Ok(inspected) => inspected,
        Err(RestoreError::Cancelled { .. }) => {
            return Err(fail(false, cancel_summary(false, true)));
        }
        Err(_) => return Err(ProbeError::Blocked(ProbeBlock::Backup)),
    };
    if inspected.image_bytes != disk.size_bytes
        || inspected.image_bytes != stored.bytes
        || !restore::hashes_match(&inspected.sha256, &backup.sha256)
    {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    Ok(())
}

fn stop_requested(
    cancel: &AtomicBool,
    cancel_file: Option<&Path>,
    overwritten: bool,
) -> Result<bool, ProbeError> {
    if cancel.load(Ordering::Acquire) {
        return Ok(true);
    }
    let Some(path) = cancel_file else {
        return Ok(false);
    };
    if is_physical_disk_path(path) {
        return Err(fail(
            overwritten,
            "refusing to read a cancel flag from a physical disk path",
        ));
    }
    restore::cancel_file_requests_stop(path).map_err(|error| fail(overwritten, error))
}

fn pattern(offset: u64) -> [u8; PROBE_BLOCK as usize] {
    let mut block = [0u8; PROBE_BLOCK as usize];
    block[..8].copy_from_slice(b"CFWPROBE");
    block[8..16].copy_from_slice(&offset.to_le_bytes());
    for (index, byte) in block[16..].iter_mut().enumerate() {
        *byte = offset.wrapping_add(index as u64).wrapping_add(0xA5) as u8;
    }
    block
}

fn decode_tag(block: &[u8]) -> Option<u64> {
    if block.len() != PROBE_BLOCK as usize || &block[..8] != b"CFWPROBE" {
        return None;
    }
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&block[8..16]);
    let offset = u64::from_le_bytes(raw);
    if block == pattern(offset) {
        Some(offset)
    } else {
        None
    }
}

fn judge(claimed: u64, samples: &[(u64, Option<u64>)]) -> (CardKind, u64, bool) {
    let mut diffs = Vec::new();
    let mut groups: Vec<(u64, Vec<u64>)> = Vec::new();
    let mut first_bad = None;
    let mut aliased = false;
    for &(offset, tag) in samples {
        match tag {
            Some(tag) if tag == offset => push_group(&mut groups, tag, offset),
            Some(tag) => {
                aliased = true;
                diffs.push(offset.abs_diff(tag));
                push_group(&mut groups, tag, offset);
                if first_bad.is_none() {
                    first_bad = Some(offset);
                }
            }
            None => {
                if first_bad.is_none() {
                    first_bad = Some(offset);
                }
            }
        }
    }
    for (_, offsets) in &groups {
        let mut ordered = offsets.clone();
        ordered.sort_unstable();
        for pair in ordered.windows(2) {
            let diff = pair[1].saturating_sub(pair[0]);
            if diff > 0 {
                diffs.push(diff);
            }
        }
    }
    if aliased {
        let period = gcd_all(&diffs);
        if period > 0 && period < claimed {
            return (CardKind::Fake, period, true);
        }
        return (CardKind::Failing, first_bad.unwrap_or(0), true);
    }
    if let Some(offset) = first_bad {
        return (CardKind::Failing, offset, false);
    }
    (CardKind::Genuine, claimed, false)
}

fn push_group(groups: &mut Vec<(u64, Vec<u64>)>, tag: u64, offset: u64) {
    if let Some((_, offsets)) = groups.iter_mut().find(|(existing, _)| *existing == tag) {
        offsets.push(offset);
    } else {
        groups.push((tag, vec![offset]));
    }
}

fn gcd_all(values: &[u64]) -> u64 {
    let mut period = 0u64;
    for value in values {
        if *value == 0 {
            continue;
        }
        period = if period == 0 {
            *value
        } else {
            gcd(period, *value)
        };
    }
    period
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    a
}

fn sequential_span(claimed: u64) -> u64 {
    let span = claimed.min(SEQUENTIAL_LIMIT);
    span - (span % PROBE_BLOCK)
}

fn measure_speed(
    device: &mut dyn FakeBlockDevice,
    cancel: &AtomicBool,
    cancel_file: Option<&Path>,
    overwritten: &mut bool,
    backed_up: bool,
) -> Result<(f64, f64), ProbeError> {
    let span = sequential_span(device.reported_len());
    if span == 0 {
        return Ok((0.0, 0.0));
    }
    let chunk = vec![0x5Au8; 64 * 1024];
    let started = Instant::now();
    let mut offset = 0u64;
    while offset < span {
        if stop_requested(cancel, cancel_file, *overwritten)? {
            return Err(fail(*overwritten, "probe-cancelled"));
        }
        let n = ((span - offset) as usize).min(chunk.len());
        *overwritten = true;
        device.write_at(offset, &chunk[..n]).map_err(|error| {
            fail(
                true,
                format!(
                    "could not measure write speed: {error}. {}",
                    overwrite_notice(backed_up)
                ),
            )
        })?;
        offset += n as u64;
    }
    let write_mbps = megabytes_per_second(span, started.elapsed());

    let started = Instant::now();
    let mut buf = vec![0u8; 64 * 1024];
    offset = 0;
    while offset < span {
        if stop_requested(cancel, cancel_file, *overwritten)? {
            return Err(fail(*overwritten, "probe-cancelled"));
        }
        let n = ((span - offset) as usize).min(buf.len());
        device.read_at(offset, &mut buf[..n]).map_err(|error| {
            fail(
                true,
                format!(
                    "could not measure read speed: {error}. {}",
                    overwrite_notice(backed_up)
                ),
            )
        })?;
        offset += n as u64;
    }
    let read_mbps = megabytes_per_second(span, started.elapsed());
    Ok((write_mbps, read_mbps))
}

fn megabytes_per_second(bytes: u64, elapsed: std::time::Duration) -> f64 {
    let secs = elapsed.as_secs_f64().max(1.0e-9);
    (bytes as f64 / 1_000_000.0) / secs
}

fn format_capacity(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        return unit(bytes, 1_000_000_000, "GB");
    }
    if bytes >= 1_000_000 {
        return unit(bytes, 1_000_000, "MB");
    }
    if bytes >= 1000 {
        return unit(bytes, 1000, "KB");
    }
    format!("{bytes} B")
}

fn unit(bytes: u64, scale: u64, name: &str) -> String {
    if bytes.is_multiple_of(scale) {
        format!("{} {name}", bytes / scale)
    } else {
        format!("{:.1} {name}", bytes as f64 / scale as f64)
    }
}

fn format_mbps(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return "an unknown speed".into();
    }
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < 0.05 {
        format!("{} MB/s", rounded.round() as u64)
    } else {
        format!("{rounded:.1} MB/s")
    }
}

pub fn normalize_serial(serial: &str) -> String {
    serial
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Prefix and serial only. Sample hashes stay in the identity file. The helper
/// compares those to the gzip backup and to the live card, so the CLI does not
/// carry them and an edited identity file cannot point at another card.
pub fn same_identity(recorded: &CardIdentity, other: &CardIdentity) -> bool {
    restore::hashes_match(&recorded.prefix_sha256, &other.prefix_sha256)
        && normalize_serial(&recorded.serial) == normalize_serial(&other.serial)
}

/// What the app does with its own blank or identity read, before elevation.
///
/// [`AppPrecheck::Deferred`] skips only that read. The elevated helper still
/// checks a blank card, or serial, prefix, and sample blocks, before it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppPrecheck {
    /// The card opened. A blank or identity failure still refuses the probe.
    Enforce,
    /// The card could not be read. Log `note` and let the helper be the gate.
    Deferred { note: String },
}

/// `None` means the card opened. `Some` is the open error.
pub fn app_precheck(open_error: Option<&str>) -> AppPrecheck {
    match open_error {
        None => AppPrecheck::Enforce,
        Some(error) => AppPrecheck::Deferred {
            note: unreadable_card_note(error),
        },
    }
}

pub fn unreadable_card_note(error: &str) -> String {
    let error = sanitize_precheck_error(error);
    format!(
        "The card could not be read without admin ({error}). Skipping the app-side blank and identity check. The elevated helper will check the card before it writes."
    )
}

/// Drop sentences that would claim the card was left untouched. The helper may
/// still write after this note is logged.
fn sanitize_precheck_error(error: &str) -> String {
    let mut text = one_line(error);
    for claim in [
        "Nothing on the card was overwritten.",
        "Nothing on the card was overwritten",
        "The card was not written.",
        "The card was not written",
    ] {
        text = text.replace(claim, "");
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        "the disk could not be opened".into()
    } else {
        text
    }
}

/// `Get-Disk` UniqueId, then SerialNumber. Does not open the disk.
pub fn disk_serial_script(number: u32) -> String {
    format!(
        "$d = Get-Disk -Number {number}; $id = [string]$d.UniqueId; if ([string]::IsNullOrWhiteSpace($id)) {{ $id = [string]$d.SerialNumber }}; $id = $id -replace '\\|','/'; $id = $id -replace \"[`r`n]\",' '; Write-Output $id.Trim()"
    )
}

pub fn prefix_len(card_bytes: u64) -> u64 {
    card_bytes.min(PREFIX_IDENTITY_BYTES)
}

pub fn prefix_sha256(device: &mut dyn FakeBlockDevice) -> Result<String, ProbeError> {
    let len = prefix_len(device.reported_len());
    if len == 0 {
        return Err(fail(
            false,
            "refusing to identify a card that reports size 0",
        ));
    }
    hash_device_prefix(device, len)
}

fn hash_device_prefix(device: &mut dyn FakeBlockDevice, len: u64) -> Result<String, ProbeError> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut offset = 0u64;
    while offset < len {
        let n = ((len - offset) as usize).min(buf.len());
        device
            .read_at(offset, &mut buf[..n])
            .map_err(|error| fail(false, format!("could not read the card identity: {error}")))?;
        hasher.update(&buf[..n]);
        offset += n as u64;
    }
    Ok(hex_hash(hasher.finalize()))
}

pub fn gzip_prefix_sha256(path: &Path, card_bytes: u64) -> Result<String, ProbeError> {
    if is_physical_disk_path(path) {
        return Err(fail(
            false,
            "refusing to read a backup from a physical disk path",
        ));
    }
    let len = prefix_len(card_bytes);
    if len == 0 {
        return Err(fail(false, "refusing to identify a backup of size 0"));
    }
    let file = File::open(path)
        .map_err(|error| fail(false, format!("could not open {}: {error}", path.display())))?;
    let mut decoder = GzDecoder::new(file);
    let mut hasher = Sha256::new();
    let mut left = len;
    let mut buf = vec![0u8; 64 * 1024];
    while left > 0 {
        let n = (left as usize).min(buf.len());
        decoder.read_exact(&mut buf[..n]).map_err(|error| {
            fail(
                false,
                format!("could not read the backup identity: {error}"),
            )
        })?;
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(hex_hash(hasher.finalize()))
}

/// Hash each block [`sample_offsets`] will overwrite on this card.
pub fn sample_block_hashes(
    device: &mut dyn FakeBlockDevice,
) -> Result<Vec<SampleHash>, ProbeError> {
    let offsets = sample_offsets(device.reported_len());
    hash_blocks_at(device, &offsets)
}

fn hash_blocks_at(
    device: &mut dyn FakeBlockDevice,
    offsets: &[u64],
) -> Result<Vec<SampleHash>, ProbeError> {
    let mut block = vec![0u8; PROBE_BLOCK as usize];
    let mut samples = Vec::with_capacity(offsets.len());
    for offset in offsets {
        device.read_at(*offset, &mut block).map_err(|error| {
            fail(
                false,
                format!("could not read the card sample at {offset}: {error}"),
            )
        })?;
        samples.push(SampleHash {
            offset: *offset,
            sha256: hash_block(&block),
        });
    }
    Ok(samples)
}

/// Prefix and sample blocks on `device` must match the identity.
pub fn live_identity_matches(
    device: &mut dyn FakeBlockDevice,
    identity: &CardIdentity,
) -> Result<(), ProbeError> {
    let live_prefix = prefix_sha256(device)?;
    if !restore::hashes_match(&live_prefix, &identity.prefix_sha256) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    let live_samples = sample_block_hashes(device)?;
    if !samples_match(&identity.samples, &live_samples) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    Ok(())
}

/// Sample hashes of the gzip image. One pass, in offset order.
pub fn gzip_sample_hashes(path: &Path, card_bytes: u64) -> Result<Vec<SampleHash>, ProbeError> {
    if is_physical_disk_path(path) {
        return Err(fail(
            false,
            "refusing to read a backup from a physical disk path",
        ));
    }
    let offsets = sample_offsets(card_bytes);
    let file = File::open(path)
        .map_err(|error| fail(false, format!("could not open {}: {error}", path.display())))?;
    if offsets.is_empty() {
        return Ok(Vec::new());
    }
    let mut decoder = GzDecoder::new(file);
    let mut pos = 0u64;
    let mut skip = vec![0u8; 64 * 1024];
    let mut block = vec![0u8; PROBE_BLOCK as usize];
    let mut samples = Vec::with_capacity(offsets.len());
    for offset in offsets {
        if offset < pos {
            return Err(fail(false, "sample offsets are out of order"));
        }
        while pos < offset {
            let n = ((offset - pos) as usize).min(skip.len());
            decoder.read_exact(&mut skip[..n]).map_err(|error| {
                fail(
                    false,
                    format!("could not read the backup sample blocks: {error}"),
                )
            })?;
            pos += n as u64;
        }
        decoder.read_exact(&mut block).map_err(|error| {
            fail(
                false,
                format!("could not read the backup sample at {offset}: {error}"),
            )
        })?;
        pos += PROBE_BLOCK;
        samples.push(SampleHash {
            offset,
            sha256: hash_block(&block),
        });
    }
    Ok(samples)
}

fn hash_block(block: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(block);
    hex_hash(hasher.finalize())
}

pub fn samples_match(recorded: &[SampleHash], other: &[SampleHash]) -> bool {
    recorded.len() == other.len()
        && recorded.iter().zip(other).all(|(left, right)| {
            left.offset == right.offset && restore::hashes_match(&left.sha256, &right.sha256)
        })
}

fn samples_cover(bytes: u64, samples: &[SampleHash]) -> bool {
    let expected = sample_offsets(bytes);
    samples.len() == expected.len()
        && samples.iter().zip(&expected).all(|(sample, offset)| {
            sample.offset == *offset && restore::is_sha256_hex(&sample.sha256)
        })
}

pub fn identity_path(image: &Path) -> PathBuf {
    let mut name = image.file_name().unwrap_or_default().to_os_string();
    name.push(".identity");
    image.with_file_name(name)
}

pub fn write_identity(image: &Path, identity: &CardIdentity) -> Result<(), String> {
    let path = identity_path(image);
    if is_physical_disk_path(image) || is_physical_disk_path(&path) {
        return Err("refusing to write card identity to a physical disk path".into());
    }
    if !restore::is_sha256_hex(&identity.prefix_sha256) {
        return Err("card identity prefix is not a sha256".into());
    }
    if !samples_cover(identity.bytes, &identity.samples) {
        return Err(
            "card identity sample hashes do not cover the blocks this probe overwrites".into(),
        );
    }
    let serial = normalize_serial(&identity.serial);
    let model = one_line(&identity.model);
    let mut body = format!(
        "prefix-sha256={}\nserial={serial}\nbytes={}\nmodel={model}\n",
        identity.prefix_sha256.trim().to_ascii_lowercase(),
        identity.bytes,
    );
    for sample in &identity.samples {
        body.push_str(&format!(
            "sample-sha256={}:{}\n",
            sample.offset,
            sample.sha256.trim().to_ascii_lowercase(),
        ));
    }
    restore::write_new_nofollow(&path, body.as_bytes())
        .map_err(|error| format!("could not write card identity: {error}"))
}

pub fn load_identity(image: &Path) -> Result<CardIdentity, String> {
    let path = identity_path(image);
    if is_physical_disk_path(image) || is_physical_disk_path(&path) {
        return Err("refusing to read card identity from a physical disk path".into());
    }
    let text = restore::read_nofollow_string(&path)
        .map_err(|error| format!("could not read card identity: {error}"))?;
    parse_identity(&text)
}

pub fn parse_identity(text: &str) -> Result<CardIdentity, String> {
    let mut prefix = None;
    let mut serial = None;
    let mut bytes = None;
    let mut model = None;
    let mut samples = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("prefix-sha256=") {
            prefix = Some(value.trim().to_ascii_lowercase());
        } else if let Some(value) = line.strip_prefix("serial=") {
            serial = Some(normalize_serial(value));
        } else if let Some(value) = line.strip_prefix("bytes=") {
            bytes = Some(
                value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| format!("bad card identity size in {line}"))?,
            );
        } else if let Some(value) = line.strip_prefix("model=") {
            model = Some(one_line(value));
        } else if let Some(value) = line.strip_prefix("sample-sha256=") {
            samples.push(parse_sample_hash(value).map_err(|error| format!("{error} in {line}"))?);
        } else {
            return Err(format!("bad card identity line {line}"));
        }
    }
    let prefix = prefix.ok_or("card identity is missing prefix-sha256")?;
    if !restore::is_sha256_hex(&prefix) {
        return Err("card identity prefix is not a sha256".into());
    }
    let bytes = bytes.ok_or("card identity is missing bytes")?;
    if !samples_cover(bytes, &samples) {
        if samples.is_empty() {
            return Err("card identity is missing sample-sha256".into());
        }
        return Err(
            "card identity sample hashes do not cover the blocks this probe overwrites".into(),
        );
    }
    Ok(CardIdentity {
        prefix_sha256: prefix,
        serial: serial.unwrap_or_default(),
        bytes,
        model: model.ok_or("card identity is missing model")?,
        samples,
    })
}

fn parse_sample_hash(value: &str) -> Result<SampleHash, String> {
    let (offset, hash) = value.split_once(':').ok_or("bad card identity sample")?;
    let offset = offset
        .trim()
        .parse::<u64>()
        .map_err(|_| "bad card identity sample offset")?;
    let sha256 = hash.trim().to_ascii_lowercase();
    if !restore::is_sha256_hex(&sha256) {
        return Err("card identity sample is not a sha256".into());
    }
    Ok(SampleHash { offset, sha256 })
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r', '|'], " ").trim().to_string()
}

fn hex_hash(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

/// Lock failure on the backup route refuses the write.
pub fn backup_write_allowed(locked: Result<(), String>) -> Result<(), String> {
    locked.map_err(|error| {
        format!("could not lock and dismount the card, so nothing was written: {error}")
    })
}

/// Arguments the app passes straight to the elevated `cfw-flash --probe` helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperRequest {
    pub disk_number: u32,
    pub confirmation: String,
    pub displayed_bytes: u64,
    pub displayed_model: String,
    pub backup_image: Option<PathBuf>,
    pub backup_sha256: Option<String>,
    pub backup_prefix_sha256: Option<String>,
    pub backup_serial: Option<String>,
    pub cancel_file: PathBuf,
    pub log_file: PathBuf,
}

/// One elevated `Start-Process` of `cfw-flash`. No temp script.
pub fn helper_launch_command(tool: &Path, request: &HelperRequest) -> Result<String, String> {
    for path in [
        tool,
        request.backup_image.as_deref().unwrap_or(Path::new("")),
        request.cancel_file.as_path(),
        request.log_file.as_path(),
    ] {
        if !path.as_os_str().is_empty() && is_physical_disk_path(path) {
            return Err("refusing a physical disk path".into());
        }
    }
    let mut args = vec![
        "--probe".to_string(),
        format!("--disk={}", request.disk_number),
        format!("--confirm={}", request.confirmation),
        format!("--expected-bytes={}", request.displayed_bytes),
        format!("--expected-model={}", request.displayed_model),
    ];
    match (
        &request.backup_image,
        &request.backup_sha256,
        &request.backup_prefix_sha256,
        &request.backup_serial,
    ) {
        (Some(image), Some(sha), Some(prefix), Some(serial)) => {
            if !restore::is_sha256_hex(sha) || !restore::is_sha256_hex(prefix) {
                return Err("a probe backup checksum is not a sha256".into());
            }
            args.push(format!("--backup-image={}", image.display()));
            args.push(format!("--backup-sha256={sha}"));
            args.push(format!("--backup-prefix-sha256={prefix}"));
            args.push(format!("--backup-serial={serial}"));
        }
        (None, None, None, None) => {}
        _ => {
            return Err(
                "a probe backup needs the image, checksum, and card identity from this session"
                    .into(),
            )
        }
    }
    args.push(format!("--cancel-file={}", request.cancel_file.display()));
    args.push(format!("--log-file={}", request.log_file.display()));
    for arg in &args {
        powershell_quote(arg)?;
    }
    let list = args
        .iter()
        .map(|arg| format!("'{}'", arg.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        "$p = Start-Process -FilePath '{tool}' -Verb RunAs -Wait -PassThru -ArgumentList @({list}); if ($null -eq $p -or $null -eq $p.ExitCode) {{ exit 1 }}; exit $p.ExitCode",
        tool = powershell_quote(&tool.display().to_string())?,
    ))
}

fn powershell_quote(text: &str) -> Result<String, String> {
    if text.chars().any(|ch| matches!(ch, '\n' | '\r' | '\0')) {
        return Err("a probe path contains a newline".into());
    }
    Ok(text.replace('\'', "''"))
}

pub fn format_failed_log(overwritten: bool, message: &str) -> String {
    format!(
        "OVERWRITTEN={flag}\nSUMMARY={summary}\nRESULT=FAIL\n",
        flag = yes_no(overwritten),
        summary = one_line(message),
    )
}

pub fn format_probe_log(status: &ProbeStatus) -> String {
    match status {
        ProbeStatus::Finished(report) => format!(
            "KIND={kind}\nCLAIMED={claimed}\nREAL={real}\nWRITE_MBPS={write}\nREAD_MBPS={read}\nALIASED={aliased}\nOVERWRITTEN=yes\nSUMMARY={summary}\nRESULT=OK\n",
            kind = report.kind.label(),
            claimed = report.claimed_bytes,
            real = report.real_bytes,
            write = report.write_mbps,
            read = report.read_mbps,
            aliased = if report.aliased { "yes" } else { "no" },
            summary = one_line(&report.summary),
        ),
        ProbeStatus::Cancelled {
            overwritten,
            summary,
        } => format!(
            "RESULT=CANCELLED\nOVERWRITTEN={flag}\nSUMMARY={summary}\n",
            flag = yes_no(*overwritten),
            summary = one_line(summary),
        ),
    }
}

fn yes_no(overwritten: bool) -> &'static str {
    if overwritten {
        "yes"
    } else {
        "no"
    }
}

/// `Ok` only when the log has exactly one `OVERWRITTEN=yes` or `OVERWRITTEN=no`.
pub fn overwritten_flag(log: &str) -> Result<bool, String> {
    let mut found = None;
    for line in log.lines() {
        let Some(value) = line.trim().strip_prefix("OVERWRITTEN=") else {
            continue;
        };
        let flag = match value.trim() {
            text if text.eq_ignore_ascii_case("yes") => true,
            text if text.eq_ignore_ascii_case("no") => false,
            _ => return Err(format!("bad overwritten flag in {line}")),
        };
        if found.is_some() {
            return Err("more than one OVERWRITTEN line".into());
        }
        found = Some(flag);
    }
    found.ok_or_else(|| "OVERWRITTEN line is missing".into())
}

pub fn parse_probe_log(log: &str) -> Result<ProbeLog, String> {
    let mut result = String::new();
    let mut kind = String::new();
    let mut claimed_bytes = 0u64;
    let mut real_bytes = 0u64;
    let mut read_mbps = 0.0f64;
    let mut write_mbps = 0.0f64;
    let mut summary = String::new();
    for line in log.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("RESULT=") {
            result = value.to_string();
        } else if let Some(value) = line.strip_prefix("KIND=") {
            kind = value.to_string();
        } else if let Some(value) = line.strip_prefix("CLAIMED=") {
            claimed_bytes = value
                .parse()
                .map_err(|_| format!("bad claimed size in {line}"))?;
        } else if let Some(value) = line.strip_prefix("REAL=") {
            real_bytes = value
                .parse()
                .map_err(|_| format!("bad real size in {line}"))?;
        } else if let Some(value) = line.strip_prefix("READ_MBPS=") {
            read_mbps = value
                .parse()
                .map_err(|_| format!("bad read speed in {line}"))?;
        } else if let Some(value) = line.strip_prefix("WRITE_MBPS=") {
            write_mbps = value
                .parse()
                .map_err(|_| format!("bad write speed in {line}"))?;
        } else if let Some(value) = line.strip_prefix("SUMMARY=") {
            summary = value.to_string();
        }
    }
    let overwritten = overwritten_flag(log)?;
    let status = match result.as_str() {
        "OK" if overwritten => "finished",
        "OK" => return Err("a finished probe must say the card was overwritten".into()),
        "CANCELLED" => "cancelled",
        _ => return Err("probe result is missing".into()),
    };
    Ok(ProbeLog {
        status: status.into(),
        kind,
        claimed_bytes,
        real_bytes,
        read_mbps,
        write_mbps,
        overwritten,
        summary,
    })
}

fn summary_line(log: &str) -> Option<String> {
    log.lines().find_map(|line| {
        line.trim()
            .strip_prefix("SUMMARY=")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// What the app should show after the elevated helper exits.
///
/// A complete log is used as written. A cancel is an exit code of 9, or the
/// in-app cancel flag, together with the `OVERWRITTEN` line. A missing line is
/// not treated as "nothing was overwritten."
#[derive(Debug, Clone, PartialEq)]
pub enum HelperFollowup {
    Parsed(ProbeLog),
    Cancelled { overwritten: bool, summary: String },
}

pub fn interpret_helper_log(
    exit_code: Option<i32>,
    log: &str,
    user_cancelled: bool,
    backed_up: bool,
) -> Result<HelperFollowup, String> {
    if let Ok(parsed) = parse_probe_log(log) {
        return Ok(HelperFollowup::Parsed(parsed));
    }
    let cancelled = exit_code == Some(9) || user_cancelled;
    match overwritten_flag(log) {
        Ok(overwritten) if cancelled => Ok(HelperFollowup::Cancelled {
            overwritten,
            summary: summary_line(log).unwrap_or_else(|| cancel_summary(overwritten, backed_up)),
        }),
        Ok(true) => {
            Err(summary_line(log).unwrap_or_else(|| overwrite_notice(backed_up).to_string()))
        }
        Ok(false) => Err(summary_line(log).unwrap_or_else(|| {
            "The test failed. The log says the card was not overwritten.".into()
        })),
        Err(_) => Err(if cancelled {
            unknown_overwrite_summary().into()
        } else {
            "The test did not finish, and the log did not say whether the card was overwritten. Restore your backup, or format the card, before you use it.".into()
        }),
    }
}
