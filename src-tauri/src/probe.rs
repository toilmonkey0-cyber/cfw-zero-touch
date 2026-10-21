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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::backup::{self, BackupBlock, BackupDisk};
use crate::block_device::{is_physical_disk_path, FakeBlockDevice};
use crate::restore::{self, RestoreError};

/// Pattern block. Samples and the sequential window stay on this boundary.
pub const PROBE_BLOCK: u64 = 512;

const SEQUENTIAL_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeDisk {
    pub number: u32,
    pub bus: String,
    pub size_bytes: u64,
    pub system_disk: bool,
    pub model: String,
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
                "the backup does not match this card's size, model, and checksum file. The card was not written."
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
    Failed(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(block) => formatter.write_str(block.message()),
            Self::Failed(message) => formatter.write_str(message),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBackup {
    pub image: PathBuf,
    pub size_bytes: u64,
    pub model: String,
    pub sha256: String,
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

/// No partition table, or every sample block is still zeros.
pub fn card_is_blank(device: &mut dyn FakeBlockDevice) -> Result<bool, ProbeError> {
    if !has_partitions(device)? {
        return Ok(true);
    }
    let offsets = sample_offsets(device.reported_len());
    regions_all_zero(device, &offsets)
}

fn has_partitions(device: &mut dyn FakeBlockDevice) -> Result<bool, ProbeError> {
    if device.reported_len() < PROBE_BLOCK {
        return Ok(false);
    }
    let mut sector = [0u8; PROBE_BLOCK as usize];
    device.read_at(0, &mut sector).map_err(|error| {
        ProbeError::Failed(format!("could not read the partition table: {error}"))
    })?;
    let mut parts = false;
    if sector[510] == 0x55 && sector[511] == 0xAA {
        for slot in 0..4 {
            if sector[446 + slot * 16 + 4] != 0 {
                parts = true;
                break;
            }
        }
    }
    if !parts && device.reported_len() >= PROBE_BLOCK * 2 {
        let mut gpt = [0u8; 8];
        device.read_at(PROBE_BLOCK, &mut gpt).map_err(|error| {
            ProbeError::Failed(format!("could not read the partition table: {error}"))
        })?;
        if &gpt == b"EFI PART" {
            parts = true;
        }
    }
    Ok(parts)
}

fn regions_all_zero(device: &mut dyn FakeBlockDevice, offsets: &[u64]) -> Result<bool, ProbeError> {
    let mut buf = vec![0u8; PROBE_BLOCK as usize];
    for offset in offsets {
        device.read_at(*offset, &mut buf).map_err(|error| {
            ProbeError::Failed(format!("could not read the card at {offset}: {error}"))
        })?;
        if buf.iter().any(|byte| *byte != 0) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn plain_summary(
    kind: CardKind,
    claimed: u64,
    real: u64,
    write_mbps: f64,
    read_mbps: f64,
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
    format!("{lead} {speeds}")
}

pub fn cancel_summary(overwritten: bool) -> String {
    if overwritten {
        "Cancelled. Data on the card was overwritten. Restore your backup before you use this card."
            .into()
    } else {
        "Cancelled. Nothing on the card was overwritten.".into()
    }
}

pub fn run_probe(
    device: &mut dyn FakeBlockDevice,
    request: &ProbeRequest<'_>,
    cancel: &AtomicBool,
    cancel_file: Option<&Path>,
) -> Result<ProbeStatus, ProbeError> {
    authorize_probe(request.disk, request.plan).map_err(ProbeError::Blocked)?;
    if device.reported_len() != request.disk.size_bytes {
        return Err(ProbeError::Blocked(ProbeBlock::Size));
    }
    if let Some(backup) = request.backup {
        verify_session_backup(backup, request.disk, cancel)?;
    } else if !card_is_blank(device)? {
        return Err(ProbeError::Blocked(ProbeBlock::NotBlank));
    }
    if stop_requested(cancel, cancel_file)? {
        return Ok(cancelled(false));
    }

    let claimed = device.reported_len();
    let offsets = sample_offsets(claimed);
    if offsets.is_empty() {
        return Err(ProbeError::Blocked(ProbeBlock::Size));
    }
    let mut overwritten = false;
    for offset in &offsets {
        if stop_requested(cancel, cancel_file)? {
            return Ok(cancelled(overwritten));
        }
        let block = pattern(*offset);
        device.write_at(*offset, &block).map_err(|error| {
            ProbeError::Failed(format!(
                "could not write the test pattern at {offset}: {error}"
            ))
        })?;
        overwritten = true;
    }
    if stop_requested(cancel, cancel_file)? {
        return Ok(cancelled(overwritten));
    }

    let mut samples = Vec::with_capacity(offsets.len());
    let mut buf = vec![0u8; PROBE_BLOCK as usize];
    for offset in &offsets {
        if stop_requested(cancel, cancel_file)? {
            return Ok(cancelled(overwritten));
        }
        device.read_at(*offset, &mut buf).map_err(|error| {
            ProbeError::Failed(format!(
                "could not read the test pattern at {offset}: {error}"
            ))
        })?;
        samples.push((*offset, decode_tag(&buf)));
    }
    let (kind, real_bytes, aliased) = judge(claimed, &samples);

    if stop_requested(cancel, cancel_file)? {
        return Ok(cancelled(overwritten));
    }
    let (write_mbps, read_mbps) = match measure_speed(device, cancel, cancel_file, &mut overwritten)
    {
        Ok(speeds) => speeds,
        Err(ProbeError::Failed(message)) if message == "probe-cancelled" => {
            return Ok(cancelled(overwritten));
        }
        Err(error) => return Err(error),
    };
    let summary = plain_summary(kind, claimed, real_bytes, write_mbps, read_mbps);
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

fn cancelled(overwritten: bool) -> ProbeStatus {
    ProbeStatus::Cancelled {
        overwritten,
        summary: cancel_summary(overwritten),
    }
}

fn verify_session_backup(
    backup: &SessionBackup,
    disk: &ProbeDisk,
    cancel: &AtomicBool,
) -> Result<(), ProbeError> {
    if backup.size_bytes != disk.size_bytes || !restore::same_model(&backup.model, &disk.model) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    if is_physical_disk_path(&backup.image) {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    let inspected = match restore::inspect_backup(&backup.image, cancel, |_| {}) {
        Ok(inspected) => inspected,
        Err(RestoreError::Cancelled { .. }) => {
            return Err(ProbeError::Failed(cancel_summary(false)));
        }
        Err(_) => return Err(ProbeError::Blocked(ProbeBlock::Backup)),
    };
    if inspected.image_bytes != disk.size_bytes
        || !restore::hashes_match(&inspected.sha256, &backup.sha256)
    {
        return Err(ProbeError::Blocked(ProbeBlock::Backup));
    }
    Ok(())
}

fn stop_requested(cancel: &AtomicBool, cancel_file: Option<&Path>) -> Result<bool, ProbeError> {
    if cancel.load(Ordering::Acquire) {
        return Ok(true);
    }
    let Some(path) = cancel_file else {
        return Ok(false);
    };
    if is_physical_disk_path(path) {
        return Err(ProbeError::Failed(
            "refusing to read a cancel flag from a physical disk path".into(),
        ));
    }
    restore::cancel_file_requests_stop(path).map_err(ProbeError::Failed)
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
) -> Result<(f64, f64), ProbeError> {
    let span = sequential_span(device.reported_len());
    if span == 0 {
        return Ok((0.0, 0.0));
    }
    let chunk = vec![0x5Au8; 64 * 1024];
    let started = Instant::now();
    let mut offset = 0u64;
    while offset < span {
        if stop_requested(cancel, cancel_file)? {
            return Err(ProbeError::Failed("probe-cancelled".into()));
        }
        let n = ((span - offset) as usize).min(chunk.len());
        device.write_at(offset, &chunk[..n]).map_err(|error| {
            ProbeError::Failed(format!("could not measure write speed: {error}"))
        })?;
        *overwritten = true;
        offset += n as u64;
    }
    let write_mbps = megabytes_per_second(span, started.elapsed());

    let started = Instant::now();
    let mut buf = vec![0u8; 64 * 1024];
    offset = 0;
    while offset < span {
        if stop_requested(cancel, cancel_file)? {
            return Err(ProbeError::Failed("probe-cancelled".into()));
        }
        let n = ((span - offset) as usize).min(buf.len());
        device.read_at(offset, &mut buf[..n]).map_err(|error| {
            ProbeError::Failed(format!("could not measure read speed: {error}"))
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

/// Arguments the app passes to the elevated `cfw-flash --probe` helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperRequest {
    pub disk_number: u32,
    pub confirmation: String,
    pub displayed_bytes: u64,
    pub displayed_model: String,
    pub backup_image: Option<PathBuf>,
    pub backup_sha256: Option<String>,
    pub cancel_file: PathBuf,
    pub log_file: PathBuf,
}

pub fn helper_script_line(tool: &Path, request: &HelperRequest) -> Result<String, String> {
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
    let backup = match (&request.backup_image, &request.backup_sha256) {
        (Some(path), Some(sha)) => format!(
            " --backup-image='{}' --backup-sha256='{}'",
            powershell_quote(&path.display().to_string())?,
            powershell_quote(sha)?
        ),
        (None, None) => String::new(),
        _ => {
            return Err(
                "a probe backup needs both the image and the checksum from this session".into(),
            )
        }
    };
    Ok(format!(
        "& '{tool}' --probe --disk={disk} --confirm='{confirm}' --expected-bytes={bytes} --expected-model='{model}'{backup} --cancel-file='{cancel}' --log-file='{log}'\r\nexit $LASTEXITCODE\r\n",
        tool = powershell_quote(&tool.display().to_string())?,
        disk = request.disk_number,
        confirm = powershell_quote(&request.confirmation)?,
        bytes = request.displayed_bytes,
        model = powershell_quote(&request.displayed_model)?,
        cancel = powershell_quote(&request.cancel_file.display().to_string())?,
        log = powershell_quote(&request.log_file.display().to_string())?,
    ))
}

fn powershell_quote(text: &str) -> Result<String, String> {
    if text.chars().any(|ch| matches!(ch, '\n' | '\r' | '\0')) {
        return Err("a probe path contains a newline".into());
    }
    Ok(text.replace('\'', "''"))
}

pub fn format_probe_log(status: &ProbeStatus) -> String {
    match status {
        ProbeStatus::Finished(report) => format!(
            "KIND={kind}\nCLAIMED={claimed}\nREAL={real}\nWRITE_MBPS={write}\nREAD_MBPS={read}\nALIASED={aliased}\nSUMMARY={summary}\nRESULT=OK\n",
            kind = report.kind.label(),
            claimed = report.claimed_bytes,
            real = report.real_bytes,
            write = report.write_mbps,
            read = report.read_mbps,
            aliased = if report.aliased { "yes" } else { "no" },
            summary = report.summary,
        ),
        ProbeStatus::Cancelled {
            overwritten,
            summary,
        } => format!(
            "RESULT=CANCELLED\nOVERWRITTEN={flag}\nSUMMARY={summary}\n",
            flag = if *overwritten { "yes" } else { "no" },
        ),
    }
}

pub fn parse_probe_log(log: &str) -> Result<ProbeLog, String> {
    let mut result = String::new();
    let mut kind = String::new();
    let mut claimed_bytes = 0u64;
    let mut real_bytes = 0u64;
    let mut read_mbps = 0.0f64;
    let mut write_mbps = 0.0f64;
    let mut overwritten = false;
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
        } else if let Some(value) = line.strip_prefix("OVERWRITTEN=") {
            overwritten = value.eq_ignore_ascii_case("yes");
        } else if let Some(value) = line.strip_prefix("SUMMARY=") {
            summary = value.to_string();
        }
    }
    let status = match result.as_str() {
        "OK" => "finished",
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
