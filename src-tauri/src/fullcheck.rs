//! Optional full-card write and read-back.
//!
//! The scan writes a seeded, position-dependent pattern across the size the
//! card claims, then reads every block back. A mismatch is a bad spot. A
//! block that comes back as another offset's pattern is aliasing.
//!
//! The blank-card rule, backup identity, and removable-disk gate are the
//! probe's. This module calls them. It does not open `\\.\PhysicalDriveN`.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::block_device::FakeBlockDevice;
use crate::probe::{self, ProbeError, ProbeRequest};

/// Pattern block. Every write and read stays on this boundary.
pub const FULL_BLOCK: u64 = 512;

/// Seed mixed into every block. The offset is mixed in as well, so a card
/// that aliases still fails the read-back.
pub const FULL_SEED: u64 = 0xC0FF_EEF1_1C4E_0001;

/// Typed confirmation for the full-card test.
pub const FULL_WORD: &str = "FULL";

/// Helper write size. Tests pass a smaller chunk.
pub const DEFAULT_CHUNK: u64 = 1024 * 1024;

const MAGIC: &[u8; 8] = b"CFWFULL1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BadRange {
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullProgress {
    pub phase: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub percent: u8,
    pub mbps: f64,
    pub eta_seconds: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FullReport {
    pub claimed_bytes: u64,
    pub bad_ranges: Vec<BadRange>,
    pub aliased: bool,
    pub write_mbps: f64,
    pub read_mbps: f64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FullStatus {
    Finished(FullReport),
    Cancelled { overwritten: bool, summary: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FullLog {
    pub status: String,
    pub overwritten: bool,
    pub summary: String,
    pub bad_ranges: Vec<BadRange>,
    pub claimed_bytes: u64,
    pub write_mbps: f64,
    pub read_mbps: f64,
}

pub struct FullRun<'a> {
    pub request: &'a ProbeRequest<'a>,
    pub cancel: &'a AtomicBool,
    pub cancel_file: Option<&'a Path>,
    pub chunk_bytes: u64,
}

pub fn pattern_block(offset: u64) -> [u8; FULL_BLOCK as usize] {
    let mut block = [0u8; FULL_BLOCK as usize];
    block[..8].copy_from_slice(MAGIC);
    block[8..16].copy_from_slice(&FULL_SEED.to_le_bytes());
    block[16..24].copy_from_slice(&offset.to_le_bytes());
    let mut state = FULL_SEED ^ offset.wrapping_mul(0x9E37_79B1_85EB_CA87);
    for (index, byte) in block[24..].iter_mut().enumerate() {
        state = state
            .wrapping_mul(0x6C07_896D_F5A3_B1C3)
            .wrapping_add(index as u64)
            .wrapping_add(0xBEEF);
        *byte = (state >> 33) as u8;
    }
    block
}

/// Offset tagged in a block this scan wrote, when the block is intact.
pub fn decode_pattern_offset(block: &[u8]) -> Option<u64> {
    if block.len() != FULL_BLOCK as usize || block[..8] != MAGIC[..] {
        return None;
    }
    let mut seed = [0u8; 8];
    seed.copy_from_slice(&block[8..16]);
    if u64::from_le_bytes(seed) != FULL_SEED {
        return None;
    }
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&block[16..24]);
    let offset = u64::from_le_bytes(raw);
    if block == pattern_block(offset) {
        Some(offset)
    } else {
        None
    }
}

pub fn plain_summary(ranges: &[BadRange], backed_up: bool, aliased: bool) -> String {
    let erased = erased_notice(backed_up);
    if ranges.is_empty() {
        return format!("No bad spots. {erased}");
    }
    let spots = ranges.len();
    let noun = if spots == 1 { "bad spot" } else { "bad spots" };
    let alias = if aliased {
        " The card is repeating data from another place."
    } else {
        ""
    };
    format!(
        "Found {spots} {noun} {}. Don't use this card.{alias} {erased}",
        location_phrase(ranges)
    )
}

pub fn cancel_summary(overwritten: bool, backed_up: bool) -> String {
    if !overwritten {
        return probe::cancel_summary(false, backed_up);
    }
    let next = if backed_up {
        "Restore your backup before you use this card."
    } else {
        "Format the card again before you use this card."
    };
    format!("Cancelled. Data on the card was overwritten. {next}")
}

fn erased_notice(backed_up: bool) -> &'static str {
    if backed_up {
        "This card was fully erased. Restore your backup before you use it."
    } else {
        "This card was fully erased. Format the card again before you use it."
    }
}

fn location_phrase(ranges: &[BadRange]) -> String {
    let first = ranges[0].offset;
    let last = ranges[ranges.len() - 1].offset;
    let clustered = ranges.len() == 1 || last.saturating_sub(first) <= 1_000_000_000;
    if clustered {
        return format!("near {}", place_label(first));
    }
    let labels: Vec<String> = ranges
        .iter()
        .take(3)
        .map(|range| place_label(range.offset))
        .collect();
    match labels.as_slice() {
        [one] => format!("near {one}"),
        [left, right] => format!("near {left} and {right}"),
        [left, middle, right] => format!("near {left}, {middle}, and {right}"),
        _ => format!("near {}", place_label(first)),
    }
}

fn place_label(offset: u64) -> String {
    if offset == 0 {
        "the start of the card".into()
    } else {
        probe::format_capacity(offset)
    }
}

pub fn progress_at(
    phase: &str,
    bytes_done: u64,
    bytes_total: u64,
    elapsed: Duration,
) -> FullProgress {
    let percent = percent(bytes_done, bytes_total);
    let secs = elapsed.as_secs_f64().max(1.0e-9);
    let mbps = if bytes_done == 0 {
        0.0
    } else {
        (bytes_done as f64 / 1_000_000.0) / secs
    };
    let eta_seconds = if bytes_done == 0 || bytes_done >= bytes_total || mbps <= 0.0 {
        0
    } else {
        let rate = bytes_done as f64 / secs;
        let remaining = (bytes_total - bytes_done) as f64;
        (remaining / rate).ceil() as u64
    };
    FullProgress {
        phase: phase.to_string(),
        bytes_done,
        bytes_total,
        percent,
        mbps,
        eta_seconds,
    }
}

fn percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let pct = (done as u128).saturating_mul(100) / (total as u128);
    pct.min(100) as u8
}

pub fn format_progress(progress: &FullProgress) -> String {
    format!(
        "{} {} {} {} {:.4} {}",
        progress.phase,
        progress.bytes_done,
        progress.bytes_total,
        progress.percent,
        progress.mbps,
        progress.eta_seconds
    )
}

pub fn parse_progress_line(text: &str) -> Option<FullProgress> {
    let line = text.lines().last()?.trim();
    let mut parts = line.split_whitespace();
    let phase = parts.next()?.to_string();
    if !matches!(phase.as_str(), "write" | "read") {
        return None;
    }
    let bytes_done = parts.next()?.parse().ok()?;
    let bytes_total = parts.next()?.parse().ok()?;
    let percent = parts.next()?.parse().ok()?;
    let mbps = parts.next()?.parse().ok()?;
    let eta_seconds = parts.next()?.parse().ok()?;
    Some(FullProgress {
        phase,
        bytes_done,
        bytes_total,
        percent,
        mbps,
        eta_seconds,
    })
}

pub fn run_full_check(
    device: &mut dyn FakeBlockDevice,
    job: &FullRun<'_>,
    progress: &mut dyn FnMut(FullProgress),
) -> Result<FullStatus, ProbeError> {
    let backed_up = job.request.backup.is_some();
    probe::gate_before_write(device, job.request, FULL_WORD, job.cancel)?;
    if job.chunk_bytes == 0 || !job.chunk_bytes.is_multiple_of(FULL_BLOCK) {
        return Err(ProbeError::Blocked(probe::ProbeBlock::Size));
    }
    if probe::stop_requested(job.cancel, job.cancel_file, false)? {
        return Ok(cancelled(false, backed_up));
    }
    let claimed = device.reported_len();
    if claimed < FULL_BLOCK || !claimed.is_multiple_of(FULL_BLOCK) {
        return Err(ProbeError::Blocked(probe::ProbeBlock::Size));
    }

    let total = claimed.saturating_mul(2);
    let started = Instant::now();
    let mut transferred = 0u64;
    let mut overwritten = false;
    let mut chunk = vec![0u8; job.chunk_bytes as usize];

    let write_started = Instant::now();
    let mut offset = 0u64;
    while offset < claimed {
        if probe::stop_requested(job.cancel, job.cancel_file, overwritten)? {
            return Ok(cancelled(overwritten, backed_up));
        }
        let n = ((claimed - offset) as usize).min(chunk.len());
        let end = offset + n as u64;
        if n == 0 || !(n as u64).is_multiple_of(FULL_BLOCK) || end > claimed {
            return Err(failed(
                overwritten,
                "refusing a write that is outside the claimed size or not a multiple of 512 bytes",
            ));
        }
        fill_pattern(offset, &mut chunk[..n]);
        overwritten = true;
        device.write_at(offset, &chunk[..n]).map_err(|error| {
            failed(
                true,
                format!(
                    "could not write the test pattern at {offset}: {error}. {}",
                    cancel_summary(true, backed_up)
                ),
            )
        })?;
        offset = end;
        transferred += n as u64;
        progress(progress_at("write", transferred, total, started.elapsed()));
    }
    let write_mbps = megabytes_per_second(claimed, write_started.elapsed());
    if probe::stop_requested(job.cancel, job.cancel_file, true)? {
        return Ok(cancelled(true, backed_up));
    }

    let read_started = Instant::now();
    let mut bad_ranges = Vec::new();
    let mut aliased = false;
    offset = 0;
    while offset < claimed {
        if probe::stop_requested(job.cancel, job.cancel_file, true)? {
            return Ok(cancelled(true, backed_up));
        }
        let n = ((claimed - offset) as usize).min(chunk.len());
        let end = offset + n as u64;
        if n == 0 || end > claimed {
            return Err(failed(
                true,
                "refusing a read that is outside the claimed size",
            ));
        }
        device.read_at(offset, &mut chunk[..n]).map_err(|error| {
            failed(
                true,
                format!(
                    "could not read the test pattern at {offset}: {error}. {}",
                    cancel_summary(true, backed_up)
                ),
            )
        })?;
        note_mismatches(offset, &chunk[..n], &mut bad_ranges, &mut aliased);
        offset = end;
        transferred += n as u64;
        progress(progress_at("read", transferred, total, started.elapsed()));
    }
    let read_mbps = megabytes_per_second(claimed, read_started.elapsed());
    progress(progress_at("read", total, total, started.elapsed()));
    let summary = plain_summary(&bad_ranges, backed_up, aliased);
    Ok(FullStatus::Finished(FullReport {
        claimed_bytes: claimed,
        bad_ranges,
        aliased,
        write_mbps,
        read_mbps,
        summary,
    }))
}

fn cancelled(overwritten: bool, backed_up: bool) -> FullStatus {
    FullStatus::Cancelled {
        overwritten,
        summary: cancel_summary(overwritten, backed_up),
    }
}

fn failed(overwritten: bool, message: impl Into<String>) -> ProbeError {
    ProbeError::Failed {
        overwritten,
        message: message.into(),
    }
}

fn fill_pattern(offset: u64, buf: &mut [u8]) {
    let mut at = 0;
    while at < buf.len() {
        let block = pattern_block(offset + at as u64);
        buf[at..at + FULL_BLOCK as usize].copy_from_slice(&block);
        at += FULL_BLOCK as usize;
    }
}

fn note_mismatches(offset: u64, got: &[u8], bad: &mut Vec<BadRange>, aliased: &mut bool) {
    let mut at = 0;
    while at + FULL_BLOCK as usize <= got.len() {
        let block_off = offset + at as u64;
        let slice = &got[at..at + FULL_BLOCK as usize];
        if slice != pattern_block(block_off) {
            if let Some(tagged) = decode_pattern_offset(slice) {
                if tagged != block_off {
                    *aliased = true;
                }
            }
            push_range(bad, block_off, FULL_BLOCK);
        }
        at += FULL_BLOCK as usize;
    }
}

fn push_range(ranges: &mut Vec<BadRange>, offset: u64, length: u64) {
    if let Some(last) = ranges.last_mut() {
        if last.offset + last.length == offset {
            last.length += length;
            return;
        }
    }
    ranges.push(BadRange { offset, length });
}

fn megabytes_per_second(bytes: u64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64().max(1.0e-9);
    (bytes as f64 / 1_000_000.0) / secs
}

pub fn format_full_log(status: &FullStatus) -> String {
    match status {
        FullStatus::Finished(report) => {
            let kind = if report.bad_ranges.is_empty() {
                "clean"
            } else {
                "bad"
            };
            let real = report
                .bad_ranges
                .first()
                .map(|range| range.offset)
                .unwrap_or(report.claimed_bytes);
            let mut body = format!(
                "KIND={kind}\nCLAIMED={claimed}\nREAL={real}\nWRITE_MBPS={write}\nREAD_MBPS={read}\nALIASED={aliased}\n",
                claimed = report.claimed_bytes,
                write = report.write_mbps,
                read = report.read_mbps,
                aliased = if report.aliased { "yes" } else { "no" },
            );
            for range in &report.bad_ranges {
                body.push_str(&format!("BAD={}:{}\n", range.offset, range.length));
            }
            body.push_str(&format!(
                "OVERWRITTEN=yes\nSUMMARY={}\nRESULT=OK\n",
                one_line(&report.summary)
            ));
            body
        }
        FullStatus::Cancelled {
            overwritten,
            summary,
        } => format!(
            "RESULT=CANCELLED\nOVERWRITTEN={flag}\nSUMMARY={summary}\n",
            flag = if *overwritten { "yes" } else { "no" },
            summary = one_line(summary),
        ),
    }
}

pub fn parse_bad_ranges(log: &str) -> Result<Vec<BadRange>, String> {
    let mut ranges = Vec::new();
    for line in log.lines() {
        let Some(value) = line.trim().strip_prefix("BAD=") else {
            continue;
        };
        let (offset, length) = value
            .split_once(':')
            .ok_or_else(|| format!("bad range in {line}"))?;
        let offset = offset
            .parse::<u64>()
            .map_err(|_| format!("bad range offset in {line}"))?;
        let length = length
            .parse::<u64>()
            .map_err(|_| format!("bad range length in {line}"))?;
        ranges.push(BadRange { offset, length });
    }
    Ok(ranges)
}

/// Same cancel honesty as the probe. A missing `OVERWRITTEN` line is not
/// treated as "nothing was overwritten."
pub fn interpret_full_log(
    exit_code: Option<i32>,
    log: &str,
    user_cancelled: bool,
    backed_up: bool,
) -> Result<FullLog, String> {
    match probe::interpret_helper_log(exit_code, log, user_cancelled, backed_up)? {
        probe::HelperFollowup::Parsed(parsed) => Ok(FullLog {
            status: parsed.status,
            overwritten: parsed.overwritten,
            summary: parsed.summary,
            bad_ranges: parse_bad_ranges(log)?,
            claimed_bytes: parsed.claimed_bytes,
            write_mbps: parsed.write_mbps,
            read_mbps: parsed.read_mbps,
        }),
        probe::HelperFollowup::Cancelled {
            overwritten,
            summary,
        } => Ok(FullLog {
            status: "cancelled".into(),
            overwritten,
            summary,
            bad_ranges: Vec::new(),
            claimed_bytes: 0,
            write_mbps: 0.0,
            read_mbps: 0.0,
        }),
    }
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r', '|'], " ").trim().to_string()
}
