//! Restore a U3 stock-card backup onto a removable disk.
//!
//! The backup is `name.img.gz` plus `name.img.gz.sha256` (the SHA-256 of the
//! uncompressed stored bytes). This module checks that pair, then writes
//! through [`flash::write_gzip`] and [`flash::SectorWriter`] — the same pair
//! `cfw-flash` uses. It does not open a physical disk. The app command sends
//! the checked image to `cfw-flash`, which applies [`flash::authorize_flash`]
//! again and is the only writer of `\\.\PhysicalDriveN`.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::backup::{self, BackupBlock, BackupDisk};
use crate::block_device::{is_physical_disk_path, DeviceCursor, FakeBlockDevice};
use crate::flash::{self, FlashBlock, FlashDisk, FlashPlan};

const CHUNK: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreDisk {
    pub number: u32,
    pub bus: String,
    pub size_bytes: u64,
    pub system_disk: bool,
    pub model: String,
}

/// What the operator confirmed on screen, plus the live disk from `Get-Disk`.
#[derive(Debug, Clone, Copy)]
pub struct RestoreTarget<'a> {
    pub disk: &'a RestoreDisk,
    pub confirmed_number: u32,
    pub confirmation: &'a str,
    pub displayed_bytes: u64,
    pub displayed_model: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedBackup {
    pub image: PathBuf,
    pub sidecar: PathBuf,
    pub sha256: String,
    pub compressed_sha256: String,
    pub image_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreBlock {
    Confirmation,
    NotRemovable,
    Size,
    Identity,
    Checksum,
    TooLarge,
    Model,
}

impl RestoreBlock {
    pub fn message(self) -> &'static str {
        match self {
            Self::Confirmation => {
                "type FLASH to restore onto the disk shown, including its size and model"
            }
            Self::NotRemovable => {
                "only a removable USB disk can be restored, and system or boot disks are refused"
            }
            Self::Size => {
                "refusing a disk whose size is 0 or no longer matches the size on screen"
            }
            Self::Identity => "the selected disk changed before the restore",
            Self::Checksum => "the backup does not match its SHA-256 sidecar",
            Self::TooLarge => "the backup is larger than the target card",
            Self::Model => {
                "the disk model on screen does not match this disk; refresh and confirm the model shown"
            }
        }
    }
}

#[derive(Debug)]
pub enum RestoreError {
    Blocked(RestoreBlock),
    Failed(String),
    Cancelled { bytes_written: u64 },
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(block) => formatter.write_str(block.message()),
            Self::Failed(message) => formatter.write_str(message),
            Self::Cancelled { bytes_written } => {
                formatter.write_str(&cancel_detail(*bytes_written))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreOutcome {
    Passed {
        sha256: String,
        bytes: u64,
        detail: String,
    },
    Failed {
        expected: String,
        actual: String,
        bytes: u64,
        detail: String,
    },
    Cancelled {
        bytes_written: u64,
        detail: String,
    },
}

impl RestoreOutcome {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Passed { .. } => "pass",
            Self::Failed { .. } => "fail",
            Self::Cancelled { .. } => "cancelled",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Passed { detail, .. }
            | Self::Failed { detail, .. }
            | Self::Cancelled { detail, .. } => detail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreProgress {
    pub phase: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

pub fn pass_detail(sha256: &str) -> String {
    format!("Read-back verify passed. The card matches the backup SHA-256 {sha256}.")
}

pub fn fail_detail(expected: &str, actual: &str) -> String {
    if actual.is_empty() {
        format!(
            "Read-back verify failed. The card could not be read back and does not match the backup SHA-256 {expected}. Restore again before using the card."
        )
    } else {
        format!(
            "Read-back verify failed. The card hash {actual} does not match the backup SHA-256 {expected}. Restore again before using the card."
        )
    }
}

pub fn cancel_detail(bytes_written: u64) -> String {
    if bytes_written == 0 {
        "Cancelled. The card was not written.".into()
    } else {
        "Cancelled while writing. The card was erased and does not match the backup. Restore again before using the card.".into()
    }
}

pub fn display_model(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "USB disk".into()
    } else {
        trimmed.to_string()
    }
}

pub fn same_model(displayed: &str, queried: &str) -> bool {
    let displayed = displayed.trim().replace('|', "/");
    let queried = queried.trim().replace('|', "/");
    !displayed.is_empty() && displayed == queried
}

/// Uncompressed image length must fit on the card. Zero-length images do not.
pub fn image_fits(image_bytes: u64, disk_bytes: u64) -> bool {
    image_bytes > 0 && image_bytes <= disk_bytes
}

/// True when `text` is 64 hexadecimal digits, ignoring surrounding whitespace.
pub fn is_sha256_hex(text: &str) -> bool {
    let text = text.trim();
    text.len() == 64 && text.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub fn hashes_match(left: &str, right: &str) -> bool {
    let left = left.trim();
    let right = right.trim();
    is_sha256_hex(left) && left.eq_ignore_ascii_case(right)
}

pub fn parse_sidecar(text: &str) -> Result<String, String> {
    let Some(token) = text.split_whitespace().next() else {
        return Err("backup checksum file is empty".into());
    };
    if token.len() == 64 && token.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(token.to_ascii_lowercase())
    } else {
        Err("backup checksum file does not contain a SHA-256 hex digest".into())
    }
}

/// `number|bus|size|IsSystem|IsBoot|FriendlyName`.
///
/// The first five fields use the U3 parser, so disk 0, IsSystem, and IsBoot
/// set `system_disk`. The model is everything after the fifth pipe.
pub fn parse_restore_disk(line: &str, expected_number: u32) -> Result<RestoreDisk, String> {
    let line = line.trim();
    let (head, model) = split_model(line);
    let queried = backup::parse_disk_query(head, expected_number)?;
    Ok(RestoreDisk {
        number: queried.number,
        bus: queried.bus,
        size_bytes: queried.size_bytes,
        system_disk: queried.system_disk,
        model: display_model(model),
    })
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

pub fn cancel_file_requests_stop(path: &Path) -> Result<bool, String> {
    match read_nofollow(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).trim() == "1"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/// Printed on its own line immediately before `Clear-Disk`.
pub const DISK_CLEAR_STARTED: &str = "disk-clear-started";
/// Printed on its own line after `Clear-Disk` returns success.
pub const DISK_CLEARED: &str = "disk-cleared";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopKind {
    /// Exit 9 and Clear-Disk had not been started.
    CancelledUntouched,
    /// Exit 9 after Clear-Disk was started.
    CancelledErased,
    /// The helper failed after Clear-Disk was started. The card is not intact.
    FailedAfterClear { detail: String },
    /// Pass, verify failure, or a failure that did not reach Clear-Disk.
    Other,
}

fn log_has_line(log: &str, marker: &str) -> bool {
    log.lines().any(|line| line.trim() == marker)
}

/// Last `EXIT=N` line written by the helper, if any.
pub fn parse_exit_code(log: &str) -> Option<i32> {
    log.lines().rev().find_map(|line| {
        let token = line.trim().strip_prefix("EXIT=")?;
        token.parse().ok()
    })
}

/// Choose the operator message from the helper exit code and the clear markers.
///
/// Exit 9 is cancel. `disk-clear-started` means Clear-Disk was invoked, so the
/// card must not be described as untouched.
pub fn classify_helper_stop(exit_code: i32, log: &str) -> StopKind {
    if log_has_line(log, "verify=fail") || log_has_line(log, "RESULT=OK") {
        return StopKind::Other;
    }
    let started = log_has_line(log, DISK_CLEAR_STARTED);
    let cleared = log_has_line(log, DISK_CLEARED);
    if exit_code == 9 && !started && !cleared {
        return StopKind::CancelledUntouched;
    }
    if exit_code == 9 {
        return StopKind::CancelledErased;
    }
    if started || cleared {
        let lead = if cleared {
            "The card was erased and does not match the backup. Restore again before using the card."
        } else {
            "Clear-Disk started and did not finish cleanly. The card may have been erased. Restore again before using the card."
        };
        let trimmed = log.trim();
        let detail = if trimmed.is_empty() {
            lead.to_string()
        } else {
            format!("{lead}\n{trimmed}")
        };
        return StopKind::FailedAfterClear { detail };
    }
    StopKind::Other
}

pub fn parse_progress_line(text: &str) -> Option<RestoreProgress> {
    let line = text.lines().last()?.trim();
    let mut parts = line.split_whitespace();
    let phase = parts.next()?.to_string();
    if !matches!(phase.as_str(), "verify" | "write" | "readback") {
        return None;
    }
    let bytes_done = parts.next()?.parse().ok()?;
    let bytes_total = parts.next()?.parse().ok()?;
    Some(RestoreProgress {
        phase,
        bytes_done,
        bytes_total,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperLog {
    pub cancelled_before_write: bool,
    pub cancelled_during_write: bool,
    pub verify_pass: bool,
    pub verify_fail: bool,
    pub result_ok: bool,
    pub card_sha256: String,
    pub image_sha256: String,
    pub wrote_bytes: Option<u64>,
}

pub fn parse_helper_log(log: &str) -> HelperLog {
    let mut parsed = HelperLog {
        cancelled_before_write: false,
        cancelled_during_write: false,
        verify_pass: false,
        verify_fail: false,
        result_ok: false,
        card_sha256: String::new(),
        image_sha256: String::new(),
        wrote_bytes: None,
    };
    if log.contains("restore cancelled before writing") {
        parsed.cancelled_before_write = true;
    } else if log.contains("restore cancelled") {
        parsed.cancelled_during_write = true;
    }
    for line in log.lines() {
        let line = line.trim();
        if line == "verify=pass" {
            parsed.verify_pass = true;
        } else if line == "verify=fail" {
            parsed.verify_fail = true;
        } else if line == "RESULT=OK" {
            parsed.result_ok = true;
        } else if let Some(rest) = line.strip_prefix("wrote ") {
            if let Some(token) = rest.split_whitespace().next() {
                parsed.wrote_bytes = token.parse().ok();
            }
        } else if let Some(hash) = hash_field(line, "card") {
            parsed.card_sha256 = hash;
        } else if let Some(hash) = hash_field(line, "image") {
            parsed.image_sha256 = hash;
        }
    }
    parsed
}

fn hash_field(line: &str, key: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    if parts.next()? != key {
        return None;
    }
    let token = parts.next()?;
    if token.len() == 64 && token.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(token.to_ascii_lowercase())
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperVerdict {
    Passed {
        sha256: String,
        bytes: u64,
        detail: String,
    },
    Failed {
        expected: String,
        actual: String,
        bytes: u64,
        detail: String,
    },
    Cancelled {
        detail: String,
    },
    Incomplete {
        detail: String,
    },
}

/// Decide pass, fail, or cancel from the privileged helper's log.
///
/// `RESULT=OK` by itself is the OS-flash success line. A restore pass also
/// needs `verify=pass` and a card hash equal to the sidecar.
pub fn judge_helper_log(log: &str, sidecar: &str) -> HelperVerdict {
    let parsed = parse_helper_log(log);
    if parsed.cancelled_before_write {
        return HelperVerdict::Cancelled {
            detail: cancel_detail(0),
        };
    }
    if parsed.cancelled_during_write {
        return HelperVerdict::Cancelled {
            detail: cancel_detail(1),
        };
    }
    let expected = sidecar.trim().to_ascii_lowercase();
    let bytes = parsed.wrote_bytes.unwrap_or(0);
    if !parsed.card_sha256.is_empty() && !hashes_match(&parsed.card_sha256, &expected) {
        return HelperVerdict::Failed {
            actual: parsed.card_sha256.clone(),
            detail: fail_detail(&expected, &parsed.card_sha256),
            expected,
            bytes,
        };
    }
    if parsed.verify_fail {
        return HelperVerdict::Failed {
            actual: parsed.card_sha256.clone(),
            detail: fail_detail(&expected, &parsed.card_sha256),
            expected,
            bytes,
        };
    }
    if parsed.result_ok && parsed.verify_pass && hashes_match(&parsed.card_sha256, &expected) {
        return HelperVerdict::Passed {
            detail: pass_detail(&parsed.card_sha256),
            sha256: parsed.card_sha256,
            bytes,
        };
    }
    let detail = if log.trim().is_empty() {
        "the restore did not finish. The approval prompt may have been dismissed.".into()
    } else {
        log.trim().to_string()
    };
    HelperVerdict::Incomplete { detail }
}

/// Check the backup name, sidecar, and decompressed SHA-256. Does not write.
pub fn inspect_backup(
    path: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(RestoreProgress),
) -> Result<InspectedBackup, RestoreError> {
    if cancel.load(Ordering::Acquire) {
        return Err(RestoreError::Cancelled { bytes_written: 0 });
    }
    validate_backup_path(path)?;
    let sidecar = backup::sidecar_path(path);
    validate_sidecar_path(&sidecar)?;
    let expected = fs::read_to_string(&sidecar).map_err(|error| {
        RestoreError::Failed(format!("could not read {}: {error}", sidecar.display()))
    })?;
    let expected = parse_sidecar(&expected).map_err(RestoreError::Failed)?;

    let file = File::open(path).map_err(|error| {
        RestoreError::Failed(format!("could not open {}: {error}", path.display()))
    })?;
    let compressed_len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let read = Arc::new(AtomicU64::new(0));
    let hashing = HashingReader {
        inner: file,
        hasher: Sha256::new(),
        read: Arc::clone(&read),
    };
    let mut decoder = GzDecoder::new(hashing);
    let mut plain = Sha256::new();
    let mut image_bytes = 0u64;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(RestoreError::Cancelled { bytes_written: 0 });
        }
        let n = decoder.read(&mut buf).map_err(|error| {
            RestoreError::Failed(format!("could not decompress the backup: {error}"))
        })?;
        if n == 0 {
            break;
        }
        plain.update(&buf[..n]);
        image_bytes += n as u64;
        on_progress(RestoreProgress {
            phase: "verify".into(),
            bytes_done: read.load(Ordering::Relaxed),
            bytes_total: compressed_len,
        });
    }
    let mut hashing = decoder.into_inner();
    let mut extra = [0u8; 8192];
    loop {
        let n = hashing.read(&mut extra).map_err(|error| {
            RestoreError::Failed(format!("could not read {}: {error}", path.display()))
        })?;
        if n == 0 {
            break;
        }
    }
    let actual = hex_hash(&plain.finalize());
    if image_bytes == 0 {
        return Err(RestoreError::Failed(
            "refusing a backup that decompresses to 0 bytes".into(),
        ));
    }
    if !hashes_match(&actual, &expected) {
        return Err(RestoreError::Failed(format!(
            "backup does not match its SHA-256 sidecar: file {actual} sidecar {expected}"
        )));
    }
    Ok(InspectedBackup {
        image: path.to_path_buf(),
        sidecar,
        sha256: actual,
        compressed_sha256: hex_hash(&hashing.hasher.finalize()),
        image_bytes,
    })
}

fn validate_backup_path(path: &Path) -> Result<(), RestoreError> {
    if is_physical_disk_path(path) {
        return Err(RestoreError::Failed(
            "refusing to read a backup from a physical disk path".into(),
        ));
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(RestoreError::Failed(
            "backup file name is not valid text".into(),
        ));
    };
    if name.to_ascii_lowercase().contains(".partial") {
        return Err(RestoreError::Failed(format!(
            "refusing a partial backup ({name}); choose a finished .img.gz"
        )));
    }
    if !name.to_ascii_lowercase().ends_with(".img.gz") {
        return Err(RestoreError::Failed(
            "backup file must end in .img.gz".into(),
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(RestoreError::Failed(
            "refusing to follow a symlink at the backup path".into(),
        )),
        Ok(meta) if !meta.file_type().is_file() => {
            Err(RestoreError::Failed("backup path is not a file".into()))
        }
        Ok(_) => Ok(()),
        Err(error) => Err(RestoreError::Failed(format!(
            "could not inspect {}: {error}",
            path.display()
        ))),
    }
}

fn validate_sidecar_path(path: &Path) -> Result<(), RestoreError> {
    if is_physical_disk_path(path) {
        return Err(RestoreError::Failed(
            "refusing to read a checksum from a physical disk path".into(),
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(RestoreError::Failed(
            "refusing to follow a symlink at the checksum path".into(),
        )),
        Ok(meta) if !meta.file_type().is_file() => Err(RestoreError::Failed(
            "backup checksum path is not a file".into(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(RestoreError::Failed(
            format!("backup checksum file is missing ({})", path.display()),
        )),
        Err(error) => Err(RestoreError::Failed(format!(
            "could not inspect {}: {error}",
            path.display()
        ))),
    }
}

/// Removable, system, boot, size, confirmation, and model checks that do not
/// need the decompressed hash. [`gate_restore`] repeats them and also calls
/// [`flash::authorize_flash`], which is what refuses a disk-number mismatch.
pub fn precheck_target(target: &RestoreTarget<'_>) -> Result<(), RestoreError> {
    if target.confirmation != "FLASH" {
        return Err(RestoreError::Blocked(RestoreBlock::Confirmation));
    }
    if !same_model(target.displayed_model, &target.disk.model) {
        return Err(RestoreError::Blocked(RestoreBlock::Model));
    }
    let backup_disk = BackupDisk {
        number: target.disk.number,
        bus: target.disk.bus.clone(),
        size_bytes: target.disk.size_bytes,
        system_disk: target.disk.system_disk || target.disk.number == 0,
        displayed_bytes: target.displayed_bytes,
    };
    backup::authorize_backup(&backup_disk).map_err(|block| {
        RestoreError::Blocked(match block {
            BackupBlock::NotRemovable => RestoreBlock::NotRemovable,
            BackupBlock::Size => RestoreBlock::Size,
        })
    })?;
    Ok(())
}

/// Inspect the backup and run every gate. Does not write.
pub fn prepare_restore(
    image: &Path,
    target: &RestoreTarget<'_>,
    cancel: &AtomicBool,
    on_progress: impl FnMut(RestoreProgress),
) -> Result<InspectedBackup, RestoreError> {
    precheck_target(target)?;
    let inspected = inspect_backup(image, cancel, on_progress)?;
    gate_restore(&inspected, target)?;
    if cancel.load(Ordering::Acquire) {
        return Err(RestoreError::Cancelled { bytes_written: 0 });
    }
    Ok(inspected)
}

/// Flash guards plus the U3 removable/system/boot/size gates.
///
/// `authorize_flash` sees the sidecar as both the expected and actual
/// checksum because [`inspect_backup`] already required them to be equal.
/// A disk-number mismatch still returns [`FlashBlock::Identity`] from that
/// call. Size 0 is [`BackupBlock::Size`] from [`backup::authorize_backup`],
/// which `authorize_flash` does not treat as a refusal when both sizes are 0.
pub fn gate_restore(
    inspected: &InspectedBackup,
    target: &RestoreTarget<'_>,
) -> Result<(), RestoreError> {
    if !same_model(target.displayed_model, &target.disk.model) {
        return Err(RestoreError::Blocked(RestoreBlock::Model));
    }
    let flash_disk = FlashDisk {
        number: target.disk.number,
        bus: target.disk.bus.clone(),
        size_bytes: target.disk.size_bytes,
        system_disk: target.disk.system_disk || target.disk.number == 0,
    };
    let plan = FlashPlan {
        confirmation: target.confirmation.to_string(),
        disk_number: target.confirmed_number,
        displayed_bytes: target.displayed_bytes,
        expected_sha256: inspected.sha256.clone(),
        actual_sha256: inspected.sha256.clone(),
    };
    flash::authorize_flash(&flash_disk, &plan).map_err(|block| {
        RestoreError::Blocked(match block {
            FlashBlock::Confirmation => RestoreBlock::Confirmation,
            FlashBlock::NotRemovable => RestoreBlock::NotRemovable,
            FlashBlock::Size => RestoreBlock::Size,
            FlashBlock::Identity => RestoreBlock::Identity,
            FlashBlock::Checksum => RestoreBlock::Checksum,
        })
    })?;
    let backup_disk = BackupDisk {
        number: target.disk.number,
        bus: target.disk.bus.clone(),
        size_bytes: target.disk.size_bytes,
        system_disk: target.disk.system_disk || target.disk.number == 0,
        displayed_bytes: target.displayed_bytes,
    };
    backup::authorize_backup(&backup_disk).map_err(|block| {
        RestoreError::Blocked(match block {
            BackupBlock::NotRemovable => RestoreBlock::NotRemovable,
            BackupBlock::Size => RestoreBlock::Size,
        })
    })?;
    if !image_fits(inspected.image_bytes, target.disk.size_bytes) {
        return Err(RestoreError::Blocked(RestoreBlock::TooLarge));
    }
    Ok(())
}

/// Inspect, authorize, write with the flash gzip path, then hash the card.
///
/// The device is always returned so a refusal can be checked for zero writes.
pub fn restore_to_device<D: FakeBlockDevice>(
    image: &Path,
    device: D,
    target: &RestoreTarget<'_>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(RestoreProgress),
) -> (D, Result<RestoreOutcome, RestoreError>) {
    let inspected = match prepare_restore(image, target, cancel, &mut on_progress) {
        Ok(inspected) => inspected,
        Err(RestoreError::Cancelled { bytes_written }) => {
            return (
                device,
                Ok(RestoreOutcome::Cancelled {
                    detail: cancel_detail(bytes_written),
                    bytes_written,
                }),
            );
        }
        Err(error) => return (device, Err(error)),
    };
    if device.reported_len() < inspected.image_bytes {
        return (device, Err(RestoreError::Blocked(RestoreBlock::TooLarge)));
    }
    if cancel.load(Ordering::Acquire) {
        return (
            device,
            Ok(RestoreOutcome::Cancelled {
                bytes_written: 0,
                detail: cancel_detail(0),
            }),
        );
    }

    let mut cursor = DeviceCursor::new(device);
    let (write_result, bytes_written) = {
        let mut sector = flash::SectorWriter::new(&mut cursor);
        let mut watched = WatchedWrite::new(
            &mut sector,
            inspected.image_bytes,
            Some(cancel),
            None,
            None,
            Some(&mut on_progress),
        );
        let opened = File::open(image)
            .map_err(|error| format!("could not open {}: {error}", image.display()));
        match opened {
            Ok(file) => {
                let result = flash::write_gzip(file, &mut watched);
                (result, watched.written())
            }
            Err(error) => (Err(error), 0),
        }
    };

    let cancelled = match &write_result {
        Err(error) if error.contains("restore cancelled") => true,
        Err(_) if cancel.load(Ordering::Acquire) => true,
        _ => false,
    };
    if cancelled {
        let device = cursor.into_device();
        return (
            device,
            Ok(RestoreOutcome::Cancelled {
                detail: cancel_detail(bytes_written),
                bytes_written,
            }),
        );
    }
    let written = match write_result {
        Ok(written) => written,
        Err(error) => {
            let device = cursor.into_device();
            return (
                device,
                Err(RestoreError::Failed(format!(
                    "{error}. The card may be incomplete; restore again before using it."
                ))),
            );
        }
    };

    on_progress(RestoreProgress {
        phase: "readback".into(),
        bytes_done: 0,
        bytes_total: written,
    });
    cursor.rewind();
    let mut reader = ProgressRead {
        inner: &mut cursor,
        done: 0,
        total: written,
        on_progress: &mut on_progress,
    };
    let actual = flash::sha256_prefix(&mut reader, written);
    on_progress(RestoreProgress {
        phase: "readback".into(),
        bytes_done: written,
        bytes_total: written,
    });
    let device = cursor.into_device();
    let outcome = match actual {
        Ok(actual) if hashes_match(&actual, &inspected.sha256) => RestoreOutcome::Passed {
            detail: pass_detail(&inspected.sha256),
            sha256: inspected.sha256,
            bytes: written,
        },
        Ok(actual) => RestoreOutcome::Failed {
            detail: fail_detail(&inspected.sha256, &actual),
            expected: inspected.sha256,
            actual,
            bytes: written,
        },
        Err(error) => RestoreOutcome::Failed {
            detail: format!(
                "Read-back verify failed. {error} The card does not match the backup SHA-256 {}. Restore again before using the card.",
                inspected.sha256
            ),
            expected: inspected.sha256,
            actual: String::new(),
            bytes: written,
        },
    };
    (device, Ok(outcome))
}

/// Counts bytes and can stop a [`flash::write_gzip`] stream.
///
/// The helper wraps [`flash::SectorWriter`] with this when a restore passes
/// `--cancel-file` or `--progress-file`. OS flash does not construct it.
pub struct WatchedWrite<'a, W> {
    inner: W,
    written: u64,
    total: u64,
    next_report: u64,
    cancel_flag: Option<&'a AtomicBool>,
    cancel_file: Option<&'a Path>,
    progress_file: Option<&'a Path>,
    on_progress: Option<&'a mut dyn FnMut(RestoreProgress)>,
}

impl<'a, W> WatchedWrite<'a, W> {
    pub fn new(
        inner: W,
        total: u64,
        cancel_flag: Option<&'a AtomicBool>,
        cancel_file: Option<&'a Path>,
        progress_file: Option<&'a Path>,
        on_progress: Option<&'a mut dyn FnMut(RestoreProgress)>,
    ) -> Self {
        Self {
            inner,
            written: 0,
            total,
            next_report: 0,
            cancel_flag,
            cancel_file,
            progress_file,
            on_progress,
        }
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    fn stop_requested(&self) -> io::Result<bool> {
        if self
            .cancel_flag
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            return Ok(true);
        }
        if let Some(path) = self.cancel_file {
            return match cancel_file_requests_stop(path) {
                Ok(stop) => Ok(stop),
                Err(error) => Err(io::Error::other(error)),
            };
        }
        Ok(false)
    }

    fn report(&mut self, force: bool) {
        let file_due = force || self.written >= self.next_report;
        if file_due {
            self.next_report = self.written.saturating_add(CHUNK as u64);
            if let Some(path) = self.progress_file {
                let _ = write_nofollow(
                    path,
                    format!("write {} {}\n", self.written, self.total).as_bytes(),
                );
            }
        }
        if let Some(callback) = self.on_progress.as_mut() {
            callback(RestoreProgress {
                phase: "write".into(),
                bytes_done: self.written,
                bytes_total: self.total,
            });
        }
    }
}

impl<W: Write> Write for WatchedWrite<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.stop_requested()? {
            // `Write::write_all` retries `ErrorKind::Interrupted` forever.
            return Err(io::Error::other("restore cancelled"));
        }
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        self.report(false);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.stop_requested()? {
            return Err(io::Error::other("restore cancelled"));
        }
        self.inner.flush()?;
        self.report(true);
        Ok(())
    }
}

/// Stops a flash write once `limit` device bytes have been accepted.
///
/// The helper puts this under [`flash::SectorWriter`] so a backup that is
/// larger than the card cannot keep writing past the queried disk size.
pub struct BoundedWrite<W> {
    inner: W,
    written: u64,
    limit: u64,
}

impl<W> BoundedWrite<W> {
    pub fn new(inner: W, limit: u64) -> Self {
        Self {
            inner,
            written: 0,
            limit,
        }
    }

    pub fn written(&self) -> u64 {
        self.written
    }
}

impl<W: Write> Write for BoundedWrite<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let room = self.limit.saturating_sub(self.written);
        if buf.len() as u64 > room {
            return Err(io::Error::other("image is larger than the target disk"));
        }
        let n = self.inner.write(buf)?;
        if n == 0 {
            return Err(io::Error::other("short write while flashing"));
        }
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Create a regular file. An existing symlink is not replaced or followed.
pub fn write_new_nofollow(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = create_nofollow(path)?;
    file.write_all(data)?;
    file.flush()
}

/// Replace the bytes of an existing regular file. Symlinks are refused.
pub fn write_nofollow(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = open_nofollow(path, true)?;
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(data)?;
    file.flush()
}

/// Open an existing regular file for writing from the start.
///
/// The returned handle stays on that file if the directory entry is replaced
/// later. Callers that write a log from an elevated process use this so a
/// symlink planted in the user temp directory is not followed.
pub fn open_nofollow_write(path: &Path) -> io::Result<File> {
    let mut file = open_nofollow(path, true)?;
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    Ok(file)
}

pub fn read_nofollow(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_nofollow(path, false)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    Ok(data)
}

pub fn read_nofollow_string(path: &Path) -> io::Result<String> {
    let bytes = read_nofollow(path)?;
    String::from_utf8(bytes).map_err(|error| io::Error::other(format!("log is not utf-8: {error}")))
}

/// Write `1` into the cancel file without following a symlink.
pub fn signal_cancel_file(path: &Path) -> io::Result<()> {
    if symlink_metadata_is_link(path) {
        fs::remove_file(path)?;
        return write_new_nofollow(path, b"1\n");
    }
    match write_nofollow(path, b"1\n") {
        Err(error) if error.kind() == io::ErrorKind::NotFound => write_new_nofollow(path, b"1\n"),
        other => other,
    }
}

fn create_nofollow(path: &Path) -> io::Result<File> {
    if symlink_metadata_is_link(path) {
        return Err(link_error());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    apply_nofollow(&mut options);
    match options.open(path) {
        Ok(file) => reject_reparse(file),
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists && symlink_metadata_is_link(path) =>
        {
            Err(link_error())
        }
        Err(error) => Err(map_link_error(error)),
    }
}

fn open_nofollow(path: &Path, write: bool) -> io::Result<File> {
    if symlink_metadata_is_link(path) {
        return Err(link_error());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    if write {
        options.write(true);
    }
    apply_nofollow(&mut options);
    match options.open(path) {
        Ok(file) => reject_reparse(file),
        Err(error) => Err(map_link_error(error)),
    }
}

fn symlink_metadata_is_link(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn link_error() -> io::Error {
    io::Error::other("refusing to follow a link")
}

fn map_link_error(error: io::Error) -> io::Error {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return link_error();
    }
    error
}

fn apply_nofollow(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
}

fn reject_reparse(file: File) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        let attributes = file.metadata()?.file_attributes();
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(link_error());
        }
    }
    Ok(file)
}

struct ProgressRead<'a, R> {
    inner: R,
    done: u64,
    total: u64,
    on_progress: &'a mut dyn FnMut(RestoreProgress),
}

impl<R: Read> Read for ProgressRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        if n > 0 {
            (self.on_progress)(RestoreProgress {
                phase: "readback".into(),
                bytes_done: self.done,
                bytes_total: self.total,
            });
        }
        Ok(n)
    }
}

struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    read: Arc<AtomicU64>,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn hex_hash(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}
