use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::store;

/// Rotate once the log reaches this size; one generation is kept.
const MAX_BYTES: u64 = 1024 * 1024;

pub fn log_path() -> PathBuf {
    store::store_root().join("diagnostics.log")
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Append one best-effort line. Logging must never fail the operation that
/// calls it, so write errors are swallowed.
pub fn log(level: &str, event: &str, detail: &str) {
    let _guard = write_lock();
    let path = log_path();
    rotate_if_oversized(&path);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{} {} {} {}", timestamp(), level, event, detail);
    }
}

fn rotate_if_oversized(path: &Path) {
    if fs::metadata(path).map(|meta| meta.len() >= MAX_BYTES).unwrap_or(false) {
        let _ = fs::rename(path, path.with_extension("old"));
    }
}

pub fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    timestamp_from_unix(now.as_secs() as i64)
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
pub fn timestamp_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}
