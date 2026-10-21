//! Optional full-card write and read-back.
//!
//! The scan writes a seeded, position-dependent pattern across the size the
//! card claims, then reads every block back. Each run draws a new seed and
//! keeps it in memory for the read-back, so a card that ignores writes and
//! still holds an older pattern does not pass. A mismatch or an unreadable
//! block is a bad spot. A block that comes back as another offset's pattern
//! for this seed is aliasing.
//!
//! The blank-card rule, backup identity, and removable-disk gate are the
//! probe's. This module calls them. It does not open `\\.\PhysicalDriveN`.

use std::collections::HashSet;
use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::block_device::FakeBlockDevice;
use crate::probe::{self, ProbeError, ProbeRequest};

/// Pattern block. Every write and read stays on this boundary.
pub const FULL_BLOCK: u64 = 512;

/// Old fixed seed. A scan does not use this. [`fresh_seed`] also refuses it,
/// so a card still holding that pattern cannot match a later run.
pub const FULL_SEED: u64 = 0xC0FF_EEF1_1C4E_0001;

/// How many distinct bad ranges are kept in memory and in `BAD=` lines.
pub const MAX_REPORTED_RANGES: usize = 32;

/// Failed 512-byte reads or writes before the scan stops instead of continuing.
pub const MAX_IO_ERRORS: u64 = 1024;

/// Windows unbuffered reads need this alignment. The scan buffer uses it too.
const RAW_ALIGN: usize = 4096;

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
    pub seed: u64,
    pub bad_ranges: Vec<BadRange>,
    pub range_count: u64,
    pub bad_bytes: u64,
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
    pub range_count: u64,
    pub bad_bytes: u64,
    pub seed: u64,
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

/// A new seed for one scan. Zero and [`FULL_SEED`] are skipped.
pub fn fresh_seed() -> u64 {
    use std::collections::hash_map::RandomState;
    for _ in 0..8 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(0xC0FF_EE01);
        let seed = hasher.finish();
        if seed != 0 && seed != FULL_SEED {
            return seed;
        }
    }
    FULL_SEED ^ 0x9E37_79B9_7F4A_7C15
}

pub fn pattern_block(seed: u64, offset: u64) -> [u8; FULL_BLOCK as usize] {
    let mut block = [0u8; FULL_BLOCK as usize];
    block[..8].copy_from_slice(MAGIC);
    block[8..16].copy_from_slice(&seed.to_le_bytes());
    block[16..24].copy_from_slice(&offset.to_le_bytes());
    let mut state = seed ^ offset.wrapping_mul(0x9E37_79B1_85EB_CA87);
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
pub fn decode_pattern_offset(seed: u64, block: &[u8]) -> Option<u64> {
    if block.len() != FULL_BLOCK as usize || block[..8] != MAGIC[..] {
        return None;
    }
    let mut tagged_seed = [0u8; 8];
    tagged_seed.copy_from_slice(&block[8..16]);
    if u64::from_le_bytes(tagged_seed) != seed {
        return None;
    }
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&block[16..24]);
    let offset = u64::from_le_bytes(raw);
    if block == pattern_block(seed, offset) {
        Some(offset)
    } else {
        None
    }
}

pub fn plain_summary(
    ranges: &[BadRange],
    range_count: u64,
    bad_bytes: u64,
    backed_up: bool,
    aliased: bool,
) -> String {
    let erased = erased_notice(backed_up);
    if range_count == 0 || ranges.is_empty() {
        return format!("No bad spots. {erased}");
    }
    let noun = if range_count == 1 {
        "bad spot"
    } else {
        "bad spots"
    };
    let alias = if aliased {
        " The card is repeating data from another place."
    } else {
        ""
    };
    let more = if range_count > ranges.len() as u64 {
        format!(
            " Showing the first {} spots. {} of the card failed.",
            ranges.len(),
            probe::format_capacity(bad_bytes)
        )
    } else {
        String::new()
    };
    format!(
        "Found {range_count} {noun} {}. Don't use this card.{alias}{more} {erased}",
        location_phrase(ranges)
    )
}

/// A failure is not a cancel. The card may already have been overwritten.
pub fn error_summary(backed_up: bool) -> String {
    let next = if backed_up {
        "Restore your backup before you use this card."
    } else {
        "Format the card again before you use this card."
    };
    format!("The test stopped because of an error. {next}")
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

    let seed = fresh_seed();
    let total = claimed.saturating_mul(2);
    let started = Instant::now();
    let mut transferred = 0u64;
    let mut overwritten = false;
    let mut chunk = aligned_chunk(job.chunk_bytes as usize);
    let mut spots = Spots::default();

    let write_started = Instant::now();
    let mut offset = 0u64;
    while offset < claimed {
        if probe::stop_requested(job.cancel, job.cancel_file, overwritten)? {
            return Ok(cancelled(overwritten, backed_up));
        }
        let n = ((claimed - offset) as usize).min(chunk.len);
        let end = offset + n as u64;
        if n == 0 || !(n as u64).is_multiple_of(FULL_BLOCK) || end > claimed {
            return Err(failed(
                overwritten,
                format!(
                    "refusing a write that is outside the claimed size or not a multiple of 512 bytes. {}",
                    error_summary(backed_up)
                ),
            ));
        }
        {
            let buf = chunk.as_mut_slice();
            fill_pattern(seed, offset, &mut buf[..n]);
        }
        overwritten = true;
        if matches!(
            write_region(
                device,
                offset,
                &chunk.as_slice()[..n],
                &mut spots,
                job,
                backed_up,
            )?,
            Region::Cancel
        ) {
            return Ok(cancelled(true, backed_up));
        }
        offset = end;
        transferred += n as u64;
        progress(progress_at("write", transferred, total, started.elapsed()));
    }
    let write_mbps = megabytes_per_second(claimed, write_started.elapsed());
    if probe::stop_requested(job.cancel, job.cancel_file, true)? {
        return Ok(cancelled(true, backed_up));
    }

    let read_started = Instant::now();
    offset = 0;
    while offset < claimed {
        if probe::stop_requested(job.cancel, job.cancel_file, true)? {
            return Ok(cancelled(true, backed_up));
        }
        let n = ((claimed - offset) as usize).min(chunk.len);
        let end = offset + n as u64;
        if n == 0 || end > claimed {
            return Err(failed(
                true,
                format!(
                    "refusing a read that is outside the claimed size. {}",
                    error_summary(backed_up)
                ),
            ));
        }
        if matches!(
            read_region(
                device,
                seed,
                offset,
                &mut chunk.as_mut_slice()[..n],
                &mut spots,
                job,
                backed_up,
            )?,
            Region::Cancel
        ) {
            return Ok(cancelled(true, backed_up));
        }
        offset = end;
        transferred += n as u64;
        progress(progress_at("read", transferred, total, started.elapsed()));
    }
    let read_mbps = megabytes_per_second(claimed, read_started.elapsed());
    progress(progress_at("read", total, total, started.elapsed()));
    let summary = plain_summary(
        &spots.ranges,
        spots.range_count,
        spots.bad_bytes,
        backed_up,
        spots.aliased,
    );
    Ok(FullStatus::Finished(FullReport {
        claimed_bytes: claimed,
        seed,
        bad_ranges: spots.ranges,
        range_count: spots.range_count,
        bad_bytes: spots.bad_bytes,
        aliased: spots.aliased,
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

struct AlignedChunk {
    storage: Vec<u8>,
    offset: usize,
    len: usize,
}

fn aligned_chunk(len: usize) -> AlignedChunk {
    let storage = vec![0u8; len + RAW_ALIGN];
    let align_off = storage.as_ptr().align_offset(RAW_ALIGN);
    let offset = if align_off == usize::MAX {
        0
    } else {
        align_off
    };
    AlignedChunk {
        storage,
        offset,
        len,
    }
}

impl AlignedChunk {
    fn as_slice(&self) -> &[u8] {
        let start = self.offset;
        &self.storage[start..start + self.len]
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        let start = self.offset;
        let end = start + self.len;
        &mut self.storage[start..end]
    }
}

#[derive(Default)]
struct Spots {
    ranges: Vec<BadRange>,
    range_count: u64,
    bad_bytes: u64,
    aliased: bool,
    io_errors: u64,
    io_blocks: HashSet<u64>,
    /// End of the last logical range, including ranges dropped after the cap.
    open_end: Option<u64>,
}

impl Spots {
    fn note_bad(&mut self, offset: u64) {
        if self.open_end == Some(offset) {
            self.bad_bytes += FULL_BLOCK;
            self.open_end = Some(offset + FULL_BLOCK);
            if let Some(last) = self.ranges.last_mut() {
                if last.offset + last.length == offset {
                    last.length += FULL_BLOCK;
                }
            }
            return;
        }
        self.bad_bytes += FULL_BLOCK;
        self.range_count += 1;
        self.open_end = Some(offset + FULL_BLOCK);
        if self.ranges.len() < MAX_REPORTED_RANGES {
            self.ranges.push(BadRange {
                offset,
                length: FULL_BLOCK,
            });
        }
    }

    fn record_io(
        &mut self,
        offset: u64,
        backed_up: bool,
        detail: String,
    ) -> Result<(), ProbeError> {
        self.io_errors += 1;
        if self.io_blocks.insert(offset) {
            self.note_bad(offset);
        }
        if self.io_errors >= MAX_IO_ERRORS {
            return Err(failed(
                true,
                format!("{detail} {}", error_summary(backed_up)),
            ));
        }
        Ok(())
    }
}

enum Region {
    Done,
    Cancel,
}

fn write_region(
    device: &mut dyn FakeBlockDevice,
    offset: u64,
    data: &[u8],
    spots: &mut Spots,
    job: &FullRun<'_>,
    backed_up: bool,
) -> Result<Region, ProbeError> {
    if data.len() == FULL_BLOCK as usize {
        write_block(device, offset, data, spots, backed_up)?;
        return Ok(Region::Done);
    }
    if device.write_at(offset, data).is_ok() {
        return Ok(Region::Done);
    }
    let mut at = 0;
    while at < data.len() {
        if probe::stop_requested(job.cancel, job.cancel_file, true)? {
            return Ok(Region::Cancel);
        }
        let block_off = offset + at as u64;
        write_block(
            device,
            block_off,
            &data[at..at + FULL_BLOCK as usize],
            spots,
            backed_up,
        )?;
        at += FULL_BLOCK as usize;
    }
    Ok(Region::Done)
}

fn write_block(
    device: &mut dyn FakeBlockDevice,
    offset: u64,
    data: &[u8],
    spots: &mut Spots,
    backed_up: bool,
) -> Result<(), ProbeError> {
    if let Err(error) = device.write_at(offset, data) {
        spots.record_io(
            offset,
            backed_up,
            format!("could not write the test pattern at {offset}: {error}."),
        )?;
    }
    Ok(())
}

fn read_region(
    device: &mut dyn FakeBlockDevice,
    seed: u64,
    offset: u64,
    buf: &mut [u8],
    spots: &mut Spots,
    job: &FullRun<'_>,
    backed_up: bool,
) -> Result<Region, ProbeError> {
    if buf.len() == FULL_BLOCK as usize {
        read_block(device, seed, offset, buf, spots, backed_up)?;
        return Ok(Region::Done);
    }
    if device.read_at(offset, buf).is_ok() {
        note_mismatches(seed, offset, buf, spots);
        return Ok(Region::Done);
    }
    let mut at = 0;
    while at < buf.len() {
        if probe::stop_requested(job.cancel, job.cancel_file, true)? {
            return Ok(Region::Cancel);
        }
        let block_off = offset + at as u64;
        let end = at + FULL_BLOCK as usize;
        read_block(device, seed, block_off, &mut buf[at..end], spots, backed_up)?;
        at = end;
    }
    Ok(Region::Done)
}

fn read_block(
    device: &mut dyn FakeBlockDevice,
    seed: u64,
    offset: u64,
    buf: &mut [u8],
    spots: &mut Spots,
    backed_up: bool,
) -> Result<(), ProbeError> {
    match device.read_at(offset, buf) {
        Ok(()) => note_mismatches(seed, offset, buf, spots),
        Err(error) => spots.record_io(
            offset,
            backed_up,
            format!("could not read the test pattern at {offset}: {error}."),
        )?,
    }
    Ok(())
}

fn fill_pattern(seed: u64, offset: u64, buf: &mut [u8]) {
    let mut at = 0;
    while at < buf.len() {
        let block = pattern_block(seed, offset + at as u64);
        buf[at..at + FULL_BLOCK as usize].copy_from_slice(&block);
        at += FULL_BLOCK as usize;
    }
}

fn note_mismatches(seed: u64, offset: u64, got: &[u8], spots: &mut Spots) {
    let mut at = 0;
    while at + FULL_BLOCK as usize <= got.len() {
        let block_off = offset + at as u64;
        if spots.io_blocks.contains(&block_off) {
            at += FULL_BLOCK as usize;
            continue;
        }
        let slice = &got[at..at + FULL_BLOCK as usize];
        if slice != pattern_block(seed, block_off) {
            if let Some(tagged) = decode_pattern_offset(seed, slice) {
                if tagged != block_off {
                    spots.aliased = true;
                }
            }
            spots.note_bad(block_off);
        }
        at += FULL_BLOCK as usize;
    }
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
                "KIND={kind}\nCLAIMED={claimed}\nREAL={real}\nSEED={seed}\nWRITE_MBPS={write}\nREAD_MBPS={read}\nALIASED={aliased}\nBAD_BYTES={bad_bytes}\nBAD_COUNT={count}\n",
                claimed = report.claimed_bytes,
                seed = report.seed,
                write = report.write_mbps,
                read = report.read_mbps,
                aliased = if report.aliased { "yes" } else { "no" },
                bad_bytes = report.bad_bytes,
                count = report.range_count,
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
        probe::HelperFollowup::Parsed(parsed) => {
            let bad_ranges = parse_bad_ranges(log)?;
            let bad_bytes = field_u64(log, "BAD_BYTES")
                .unwrap_or_else(|| bad_ranges.iter().map(|range| range.length).sum());
            let range_count = field_u64(log, "BAD_COUNT").unwrap_or(bad_ranges.len() as u64);
            Ok(FullLog {
                status: parsed.status,
                overwritten: parsed.overwritten,
                summary: parsed.summary,
                bad_ranges,
                range_count,
                bad_bytes,
                seed: field_u64(log, "SEED").unwrap_or(0),
                claimed_bytes: parsed.claimed_bytes,
                write_mbps: parsed.write_mbps,
                read_mbps: parsed.read_mbps,
            })
        }
        probe::HelperFollowup::Cancelled {
            overwritten,
            summary,
        } => Ok(FullLog {
            status: "cancelled".into(),
            overwritten,
            summary,
            bad_ranges: Vec::new(),
            range_count: 0,
            bad_bytes: 0,
            seed: 0,
            claimed_bytes: 0,
            write_mbps: 0.0,
            read_mbps: 0.0,
        }),
    }
}

fn field_u64(log: &str, name: &str) -> Option<u64> {
    let prefix = format!("{name}=");
    log.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .and_then(|value| value.parse().ok())
    })
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r', '|'], " ").trim().to_string()
}
