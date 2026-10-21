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
    if fs::metadata(path)
        .map(|meta| meta.len() >= MAX_BYTES)
        .unwrap_or(false)
    {
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
    let (hh, mm, ss) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Facts the support zip is allowed to carry. Callers pass license status
/// fields only. The license key is never one of these fields.
pub struct SupportFacts {
    pub version: String,
    pub os_name: String,
    pub arch: String,
    pub library: Option<PathBuf>,
    pub license_state: String,
    pub license_reason: String,
    pub license_message: String,
    pub license_uses: Option<u64>,
    pub over_soft_limit: bool,
    pub stale: bool,
    pub signed_updates: bool,
    pub support: bool,
    pub safety_open: bool,
    pub usernames: Vec<String>,
    pub secrets: Vec<String>,
}

pub fn format_probe(
    status: &str,
    disk: u32,
    claimed_bytes: u64,
    real_bytes: u64,
    speeds_mbps: (f64, f64),
    overwritten: bool,
    summary: &str,
) -> String {
    let (read_mbps, write_mbps) = speeds_mbps;
    format!(
        "status={status}\ndisk={disk}\nclaimed_bytes={claimed_bytes}\nreal_bytes={real_bytes}\nread_mbps={read}\nwrite_mbps={write}\noverwritten={flag}\nsummary={summary}\n",
        read = finite_speed(read_mbps),
        write = finite_speed(write_mbps),
        flag = if overwritten { "true" } else { "false" },
        summary = one_line(summary),
    )
}

pub fn format_identity(
    disk: u32,
    serial: &str,
    size_bytes: u64,
    partition_table_id: &str,
) -> String {
    format!(
        "disk_number={disk}\nserial={serial}\nsize_bytes={size_bytes}\npartition_table_id={id}\n",
        serial = one_line(serial),
        id = one_line(partition_table_id),
    )
}

pub fn format_fake_card(kind: &str, claimed_bytes: u64, real_bytes: u64, summary: &str) -> String {
    format!(
        "kind={kind}\nclaimed_bytes={claimed_bytes}\nreal_bytes={real_bytes}\nsummary={summary}\n",
        summary = one_line(summary),
    )
}

pub fn remember_probe(text: &str) {
    write_support_note("probe.txt", text);
}

pub fn remember_identity(text: &str) {
    write_support_note("identity.txt", text);
}

pub fn remember_fake_card(text: &str) {
    write_support_note("fake-card.txt", text);
}

/// A support zip is a normal file. Disk devices are refused before anything is opened.
pub fn refuse_support_destination(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("Choose a file name for the support zip.".into());
    }
    if is_device_path(&path.to_string_lossy()) {
        return Err("The support file has to be a normal file, not a disk.".into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !name.to_ascii_lowercase().ends_with(".zip") {
        return Err("The support file name has to end in .zip.".into());
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(()),
        Ok(_) => Err("The support file has to be a normal file, not a disk.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not check the support file: {error}")),
    }
}

pub fn is_device_path(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("/dev/") || lower.contains("physicaldrive") {
        return true;
    }
    // `\\?\C:\folder\file.zip` is a normal file. Every other `\\?\`, `\\.`,
    // `//./`, and `//?/` path is a device namespace.
    if is_extended_drive_path(trimmed) {
        return false;
    }
    trimmed.starts_with(r"\\?\")
        || trimmed.starts_with(r"\\.\")
        || lower.starts_with("//?/")
        || lower.starts_with("//./")
}

/// `\\?\X:\` (and `\\?\X:` itself). Forward-slash `//?/` is not this form.
fn is_extended_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 6 || !text.starts_with(r"\\?\") {
        return false;
    }
    bytes[4].is_ascii_alphabetic()
        && bytes[5] == b':'
        && (bytes.len() == 6 || bytes[6] == b'\\' || bytes[6] == b'/')
}

/// Read the app's notes and the saved card results, scrub them, and zip them.
/// This does not open a card and does not write a file.
pub fn build_support_zip(facts: &SupportFacts) -> Vec<u8> {
    let loaded = load_support_sources();
    let privacy = privacy_for(facts, &loaded.secrets);
    let files = [
        ("README.txt", readme().to_string()),
        ("version.txt", format!("version={}\n", facts.version.trim())),
        (
            "os.txt",
            format!("os={}\narch={}\n", facts.os_name.trim(), facts.arch.trim()),
        ),
        (
            "logs/diagnostics.log",
            scrub_note(
                loaded.log,
                "No diagnostics log has been written yet.\n",
                &privacy,
            ),
        ),
        (
            "logs/diagnostics.old",
            scrub_note(
                loaded.old,
                "No older diagnostics log has been written yet.\n",
                &privacy,
            ),
        ),
        (
            "probe.txt",
            scrub_note(
                loaded.probe,
                "No quick card test has been saved in this app yet.\n",
                &privacy,
            ),
        ),
        (
            "identity.txt",
            scrub_note(
                loaded.identity,
                "No card identity check has been saved in this app yet.\n",
                &privacy,
            ),
        ),
        (
            "fake-card.txt",
            scrub_note(
                loaded.fake,
                "No fake-card test has been saved in this app yet.\n",
                &privacy,
            ),
        ),
        ("license.txt", license_text(facts, &privacy)),
    ];
    let stored: Vec<StoredFile> = files
        .into_iter()
        .map(|(name, text)| StoredFile {
            name: name.to_string(),
            data: text.into_bytes(),
        })
        .collect();
    zip_store(&stored)
}

pub fn write_support_bundle(destination: &Path, facts: &SupportFacts) -> Result<(), String> {
    refuse_support_destination(destination)?;
    let bytes = build_support_zip(facts);
    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() && !is_device_path(&parent.to_string_lossy()) {
            fs::create_dir_all(parent).map_err(|error| {
                format!("could not create the folder for the support file: {error}")
            })?;
        }
    }
    fs::write(destination, bytes)
        .map_err(|error| format!("could not save the support file: {error}"))?;
    Ok(())
}

/// Open a stored (uncompressed) zip written by [`build_support_zip`].
pub fn read_stored_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    if bytes.len() < 22 {
        return Err("support zip is too small".into());
    }
    let eocd = find_eocd(bytes).ok_or("support zip has no end record")?;
    let count = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]) as usize;
    let cd_size = u32::from_le_bytes(bytes[eocd + 12..eocd + 16].try_into().unwrap()) as usize;
    let cd_offset = u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    if cd_offset.saturating_add(cd_size) > bytes.len() {
        return Err("support zip directory is out of range".into());
    }
    let mut files = Vec::with_capacity(count);
    let mut cursor = cd_offset;
    let cd_end = cd_offset + cd_size;
    for _ in 0..count {
        if cursor + 46 > cd_end {
            return Err("support zip directory entry is truncated".into());
        }
        if u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) != 0x0201_4b50 {
            return Err("support zip directory entry is not valid".into());
        }
        let method = u16::from_le_bytes([bytes[cursor + 10], bytes[cursor + 11]]);
        if method != 0 {
            return Err("support zip entry is compressed".into());
        }
        let crc = u32::from_le_bytes(bytes[cursor + 16..cursor + 20].try_into().unwrap());
        let size = u32::from_le_bytes(bytes[cursor + 20..cursor + 24].try_into().unwrap()) as usize;
        let name_len = u16::from_le_bytes([bytes[cursor + 28], bytes[cursor + 29]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[cursor + 30], bytes[cursor + 31]]) as usize;
        let comment_len = u16::from_le_bytes([bytes[cursor + 32], bytes[cursor + 33]]) as usize;
        let local =
            u32::from_le_bytes(bytes[cursor + 42..cursor + 46].try_into().unwrap()) as usize;
        let name_at = cursor + 46;
        let name_end = name_at + name_len;
        if name_end + extra_len + comment_len > cd_end {
            return Err("support zip name is truncated".into());
        }
        let name = std::str::from_utf8(&bytes[name_at..name_end])
            .map_err(|_| "support zip name is not text".to_string())?
            .to_string();
        let data = read_local(bytes, local, &name, crc, size)?;
        files.push((name, data));
        cursor = name_end + extra_len + comment_len;
    }
    Ok(files)
}

fn read_local(
    bytes: &[u8],
    local: usize,
    name: &str,
    crc: u32,
    size: usize,
) -> Result<Vec<u8>, String> {
    if local + 30 > bytes.len() {
        return Err(format!("{name} header is truncated"));
    }
    if u32::from_le_bytes(bytes[local..local + 4].try_into().unwrap()) != 0x0403_4b50 {
        return Err(format!("{name} header is not valid"));
    }
    let name_len = u16::from_le_bytes([bytes[local + 26], bytes[local + 27]]) as usize;
    let extra_len = u16::from_le_bytes([bytes[local + 28], bytes[local + 29]]) as usize;
    let data_at = local + 30 + name_len + extra_len;
    let data_end = data_at + size;
    if data_end > bytes.len() {
        return Err(format!("{name} is truncated"));
    }
    let data = bytes[data_at..data_end].to_vec();
    if crc32(&data) != crc {
        return Err(format!("{name} checksum does not match"));
    }
    Ok(data)
}

fn find_eocd(bytes: &[u8]) -> Option<usize> {
    let start = bytes.len().saturating_sub(22 + 65_535);
    let mut index = bytes.len() - 22;
    loop {
        if bytes[index..index + 4] == [0x50, 0x4b, 0x05, 0x06] {
            return Some(index);
        }
        if index == start {
            return None;
        }
        index -= 1;
    }
}

struct StoredFile {
    name: String,
    data: Vec<u8>,
}

fn zip_store(files: &[StoredFile]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for file in files {
        let local_at = u32::try_from(out.len()).unwrap_or(u32::MAX);
        let crc = crc32(&file.data);
        let size = u32::try_from(file.data.len()).unwrap_or(u32::MAX);
        let name = file.name.as_bytes();
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(u16::try_from(name.len()).unwrap_or(u16::MAX)).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&file.data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(u16::try_from(name.len()).unwrap_or(u16::MAX)).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&local_at.to_le_bytes());
        central.extend_from_slice(name);
    }
    let cd_offset = u32::try_from(out.len()).unwrap_or(u32::MAX);
    let cd_size = u32::try_from(central.len()).unwrap_or(u32::MAX);
    let count = u16::try_from(files.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

struct LoadedNotes {
    log: Option<String>,
    old: Option<String>,
    probe: Option<String>,
    identity: Option<String>,
    fake: Option<String>,
    secrets: Vec<String>,
}

fn load_support_sources() -> LoadedNotes {
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = store::store_root();
    LoadedNotes {
        log: read_text(&root.join("diagnostics.log")),
        old: read_text(&root.join("diagnostics.old")),
        probe: read_text(&root.join("support").join("probe.txt")),
        identity: read_text(&root.join("support").join("identity.txt")),
        fake: read_text(&root.join("support").join("fake-card.txt")),
        secrets: cache_needles(&root.join("license").join("activation.json")),
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn write_support_note(name: &str, body: &str) {
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let dir = store::store_root().join("support");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = fs::write(dir.join(name), body);
}

fn cache_needles(path: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    secret_needles_in_json(&text)
}

pub fn secret_needles_in_json(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    walk_secrets(&value, &mut found);
    found.retain(|item| item.len() >= 8);
    found.sort();
    found.dedup();
    found
}

fn walk_secrets(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if is_secret_key(key) {
                    if let Some(text) = child.as_str() {
                        found.push(text.to_string());
                    }
                }
                walk_secrets(child, found);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                walk_secrets(child, found);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "license_key"
            | "email"
            | "mac"
            | "token"
            | "secret"
            | "hmac"
            | "password"
            | "api_key"
            | "access_token"
            | "refresh_token"
    )
}

struct Privacy {
    paths: Vec<String>,
    rom_names: Vec<String>,
    usernames: Vec<String>,
    secrets: Vec<String>,
}

fn privacy_for(facts: &SupportFacts, cache_secrets: &[String]) -> Privacy {
    let mut paths = Vec::new();
    let mut rom_names = Vec::new();
    let mut usernames = facts.usernames.clone();
    if let Some(library) = &facts.library {
        let text = library.to_string_lossy();
        if !is_device_path(&text) {
            paths.extend(path_variants(library));
            rom_names.extend(rom_names_in(library));
        }
    }
    for home in home_directories() {
        paths.extend(path_variants(Path::new(&home)));
        if let Some(user) = username_in(&home) {
            usernames.push(user);
        }
        if let Some(short) = dos_short_path(&home) {
            if let Some(user) = username_in(&short) {
                usernames.push(user);
            }
        }
    }
    usernames.retain(|name| name.trim().len() >= 3);
    let mut secrets = facts.secrets.clone();
    secrets.extend(cache_secrets.iter().cloned());
    secrets.retain(|item| item.len() >= 8);
    Privacy {
        paths,
        rom_names,
        usernames,
        secrets,
    }
}

fn path_variants(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy().into_owned();
    let mut out = Vec::new();
    push_path_forms(&mut out, &text);
    if let Some(short) = dos_short_path(&text) {
        push_path_forms(&mut out, &short);
    }
    out.retain(|item| item.len() >= 3);
    out.sort();
    out.dedup();
    out
}

fn push_path_forms(out: &mut Vec<String>, text: &str) {
    let forward = text.replace('\\', "/");
    let back = text.replace('/', "\\");
    out.push(text.to_string());
    out.push(forward);
    out.push(back.clone());
    out.extend(extended_forms(&back));
}

fn extended_forms(back: &str) -> Vec<String> {
    if back.starts_with(r"\\?\") || back.starts_with(r"\\.\") {
        return Vec::new();
    }
    let bytes = back.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return vec![format!(r"\\?\{back}")];
    }
    if let Some(tail) = unc_tail(back) {
        return vec![format!(r"\\?\UNC\{tail}")];
    }
    Vec::new()
}

fn unc_tail(path: &str) -> Option<&str> {
    if path.len() < 3 || !path.starts_with(r"\\") {
        return None;
    }
    if path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return None;
    }
    Some(&path[2..])
}

fn home_directories() -> Vec<String> {
    let mut out = Vec::new();
    for key in ["USERPROFILE", "HOME"] {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        let trimmed = value.trim();
        if trimmed.len() < 5 || trimmed == "/" || trimmed.ends_with(':') || is_device_path(trimmed)
        {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

/// Windows 8.3 form of a path. On Windows the real short name comes from
/// `GetShortPathNameW`. The generated form is the fallback when that call
/// cannot see the folder.
fn dos_short_path(path: &str) -> Option<String> {
    #[cfg(windows)]
    if let Some(real) = windows_short_path(path) {
        return Some(real);
    }
    guessed_dos_short_path(path)
}

#[cfg(windows)]
fn windows_short_path(path: &str) -> Option<String> {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[link(name = "kernel32")]
    extern "system" {
        fn GetShortPathNameW(long_path: *const u16, short_path: *mut u16, buffer: u32) -> u32;
    }

    let back = path.replace('/', "\\");
    let wide: Vec<u16> = OsStr::new(&back)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let needed = GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0);
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u16; needed as usize];
        let written = GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), needed);
        if written == 0 || written >= needed {
            return None;
        }
        let short = OsString::from_wide(&buf[..written as usize]);
        let text = short.to_string_lossy().into_owned();
        if text.is_empty() || text.eq_ignore_ascii_case(&back) {
            None
        } else {
            Some(text)
        }
    }
}

fn guessed_dos_short_path(path: &str) -> Option<String> {
    let replaced = path.replace('/', "\\");
    let back = strip_extended_prefix(&replaced);
    let unc = back.starts_with(r"\\");
    let unix = !unc && path.replace('\\', "/").starts_with('/');
    let parts: Vec<&str> = back
        .split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let mut short_parts = Vec::with_capacity(parts.len());
    for part in parts {
        if part.len() == 2 && part.as_bytes()[1] == b':' {
            short_parts.push(part.to_ascii_uppercase());
        } else {
            short_parts.push(dos_component(part));
        }
    }
    let joined = if unix {
        short_parts.join("/")
    } else {
        short_parts.join("\\")
    };
    let result = if unc {
        format!(r"\\{joined}")
    } else if unix {
        format!("/{joined}")
    } else {
        joined
    };
    if result == back {
        None
    } else {
        Some(result)
    }
}

fn strip_extended_prefix(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with(r"\\?\unc\") || lower.starts_with("//?/unc/") {
        return format!(r"\\{}", &path[8..]);
    }
    if path.starts_with(r"\\?\")
        || path.starts_with(r"\\.\")
        || path.starts_with("//?/")
        || path.starts_with("//./")
    {
        return path[4..].to_string();
    }
    path.to_string()
}

fn dos_component(name: &str) -> String {
    let (base, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => (&name[..dot], Some(&name[dot + 1..])),
        _ => (name, None),
    };
    let base_short = dos_piece(base, 8);
    match ext {
        Some(ext) => format!("{base_short}.{}", dos_piece(ext, 3)),
        None => base_short,
    }
}

fn dos_piece(text: &str, limit: usize) -> String {
    let mut cleaned = String::new();
    let mut skipped = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            cleaned.push(ch.to_ascii_uppercase());
        } else {
            skipped = true;
        }
    }
    if cleaned.is_empty() {
        return if limit > 3 {
            "X~1".to_string()
        } else {
            "X".to_string()
        };
    }
    if limit <= 3 {
        return cleaned.chars().take(limit).collect();
    }
    if skipped || cleaned.len() > limit {
        let mut cut: String = cleaned.chars().take(6).collect();
        cut.push_str("~1");
        return cut;
    }
    cleaned
}

fn rom_names_in(library: &Path) -> Vec<String> {
    let mut names = Vec::new();
    walk_library(library, 0, &mut names);
    names.sort();
    names.dedup();
    names
}

fn walk_library(dir: &Path, depth: u32, names: &mut Vec<String>) {
    if depth > 8 || names.len() > 5_000 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if names.len() > 5_000 {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk_library(&entry.path(), depth + 1, names);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if file_name.len() < 3 {
            continue;
        }
        if let Some(stem) = Path::new(&file_name)
            .file_stem()
            .and_then(|stem| stem.to_str())
        {
            if stem.len() >= 4 && stem != file_name {
                names.push(stem.to_string());
            }
        }
        names.push(file_name);
    }
}

fn scrub_note(text: Option<String>, empty: &str, privacy: &Privacy) -> String {
    match text {
        Some(text) => scrub(recent(text.as_str()), privacy),
        None => empty.to_string(),
    }
}

fn recent(text: &str) -> &str {
    const MAX: usize = 1024 * 1024;
    if text.len() <= MAX {
        return text;
    }
    let mut start = text.len() - MAX;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    if let Some(newline) = text[start..].find('\n') {
        start += newline + 1;
    }
    &text[start..]
}

fn license_text(facts: &SupportFacts, privacy: &Privacy) -> String {
    let uses = match facts.license_uses {
        Some(uses) => uses.to_string(),
        None => "none".to_string(),
    };
    let message = scrub(&one_line(&facts.license_message), privacy);
    format!(
        "state={}\nreason={}\nmessage={message}\nuses={uses}\nover_soft_limit={}\nstale={}\nsigned_updates={}\nsupport={}\nsafety_open={}\n",
        one_line(&facts.license_state),
        one_line(&facts.license_reason),
        facts.over_soft_limit,
        facts.stale,
        facts.signed_updates,
        facts.support,
        facts.safety_open,
    )
}

fn readme() -> &'static str {
    "\
CFW Card Studio support file

This zip is saved on your computer so you can send it to support. The app does not upload it.

What's inside:
- version.txt: the Card Studio version
- os.txt: the operating system and processor type
- logs/diagnostics.log: recent notes from this app
- logs/diagnostics.old: the previous notes, if the log rolled over
- probe.txt: the latest quick card test
- identity.txt: the latest card identity check. This file includes the card serial and the partition table id
- fake-card.txt: the latest fake-card test
- license.txt: whether a license is turned on for this computer

Game names, folder paths, and account names are removed from the notes. The license key is not included. Email addresses are not included. A payment receipt is not included.

Saving this file does not need a license. Card backup, restore, and card tests keep working either way.
"
}

#[derive(Clone)]
struct Hit {
    start: usize,
    end: usize,
    replacement: &'static str,
}

fn scrub(text: &str, privacy: &Privacy) -> String {
    let redacted = redact_secret_json(text);
    let hits = collect_hits(&redacted, privacy);
    apply_hits(&redacted, &hits)
}

fn collect_hits(text: &str, privacy: &Privacy) -> Vec<Hit> {
    let mut hits = Vec::new();
    for path in &privacy.paths {
        push_matches(text, path, "[path]", false, &mut hits);
    }
    for name in &privacy.rom_names {
        push_matches(text, name, "[rom]", false, &mut hits);
    }
    for user in &privacy.usernames {
        push_matches(text, user.trim(), "[user]", true, &mut hits);
    }
    for secret in &privacy.secrets {
        push_matches(text, secret, "[redacted]", false, &mut hits);
    }
    for (start, end) in discover_home_paths(text) {
        if let Some(user) = username_in(&text[start..end]) {
            push_matches(text, &user, "[user]", true, &mut hits);
        }
        hits.push(Hit {
            start,
            end,
            replacement: "[path]",
        });
    }
    for (start, end) in find_rom_words(text) {
        hits.push(Hit {
            start,
            end,
            replacement: "[rom]",
        });
    }
    for (start, end) in find_emails(text) {
        hits.push(Hit {
            start,
            end,
            replacement: "[email]",
        });
    }
    for (start, end) in find_hyphen_secrets(text) {
        hits.push(Hit {
            start,
            end,
            replacement: "[redacted]",
        });
    }
    for (start, end) in find_labeled_secrets(text) {
        hits.push(Hit {
            start,
            end,
            replacement: "[redacted]",
        });
    }
    hits
}

fn push_matches(
    text: &str,
    needle: &str,
    replacement: &'static str,
    whole_word: bool,
    hits: &mut Vec<Hit>,
) {
    if needle.len() < 3 {
        return;
    }
    if !whole_word && replacement == "[redacted]" && needle.len() < 8 {
        return;
    }
    let hay = text.as_bytes();
    let needle_bytes = needle.as_bytes();
    if needle_bytes.is_empty() || needle_bytes.len() > hay.len() {
        return;
    }
    let mut index = 0;
    while index + needle_bytes.len() <= hay.len() {
        let end = index + needle_bytes.len();
        if hay[index..end].eq_ignore_ascii_case(needle_bytes)
            && text.is_char_boundary(index)
            && text.is_char_boundary(end)
            && (!whole_word || word_bounded(hay, index, end))
        {
            hits.push(Hit {
                start: index,
                end,
                replacement,
            });
            index = end;
        } else {
            index += 1;
        }
    }
}

fn word_bounded(hay: &[u8], start: usize, end: usize) -> bool {
    let before = start == 0 || !is_word_byte(hay[start - 1]);
    let after = end >= hay.len() || !is_word_byte(hay[end]);
    before && after
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn apply_hits(text: &str, hits: &[Hit]) -> String {
    let mut hits = hits.to_vec();
    hits.sort_by(|left, right| left.start.cmp(&right.start).then(right.end.cmp(&left.end)));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for hit in hits {
        if hit.start < cursor || hit.end <= hit.start || hit.end > text.len() {
            continue;
        }
        if !text.is_char_boundary(hit.start) || !text.is_char_boundary(hit.end) {
            continue;
        }
        out.push_str(&text[cursor..hit.start]);
        out.push_str(hit.replacement);
        cursor = hit.end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn discover_home_paths(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if path_boundary(bytes, index) {
            if let Some(end) = sensitive_path_end(text, index) {
                found.push((index, end));
                index = end;
                continue;
            }
        }
        index += 1;
    }
    found
}

fn path_boundary(bytes: &[u8], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let prev = bytes[index - 1];
    if prev == b':' {
        return !drive_letter_colon(bytes, index - 1);
    }
    prev.is_ascii_whitespace()
        || matches!(
            prev,
            b'=' | b'(' | b'[' | b',' | b';' | b'\'' | b'"' | b'`' | b'{' | b'<'
        )
}

/// `E:\` uses the colon as part of the drive letter. `note:D:\games` does not.
fn drive_letter_colon(bytes: &[u8], colon_at: usize) -> bool {
    if colon_at == 0 || colon_at + 1 >= bytes.len() {
        return false;
    }
    let letter_at = colon_at - 1;
    if !bytes[letter_at].is_ascii_alphabetic() {
        return false;
    }
    if bytes[colon_at + 1] != b'\\' && bytes[colon_at + 1] != b'/' {
        return false;
    }
    letter_at == 0 || !bytes[letter_at - 1].is_ascii_alphanumeric()
}

fn sensitive_path_end(text: &str, start: usize) -> Option<usize> {
    let min = path_prefix_len(&text[start..])?;
    let end = path_end(text, start);
    if end >= start + min && end > start {
        Some(end)
    } else {
        None
    }
}

fn path_prefix_len(rest: &str) -> Option<usize> {
    let lower = rest.to_ascii_lowercase();
    if starts_with_ascii(&lower, r"\\?\unc\") || starts_with_ascii(&lower, "//?/unc/") {
        return Some(8);
    }
    if starts_with_ascii(rest, r"\\?\")
        || starts_with_ascii(rest, r"\\.\")
        || starts_with_ascii(rest, "//?/")
        || starts_with_ascii(rest, "//./")
    {
        return Some(4);
    }
    if let Some(len) = unc_prefix_len(rest) {
        return Some(len);
    }
    let bytes = rest.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        // `E:\` by itself is a volume letter. A folder path has something after it.
        if bytes.len() == 3 || is_path_stop(bytes[3]) || bytes[3].is_ascii_whitespace() {
            return None;
        }
        return Some(3);
    }
    for marker in ["/home/", "/users/", "\\home\\", "\\users\\"] {
        if starts_with_ascii(&lower, marker) {
            return Some(marker.len());
        }
    }
    None
}

fn starts_with_ascii(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn unc_prefix_len(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    if bytes.len() < 5 {
        return None;
    }
    let sep = if rest.starts_with(r"\\") {
        b'\\'
    } else if rest.starts_with("//") {
        b'/'
    } else {
        return None;
    };
    if matches!(bytes[2], b'\\' | b'/' | b'?' | b'.') {
        return None;
    }
    let mut index = 2;
    while index < bytes.len() && bytes[index] != sep && !is_path_stop(bytes[index]) {
        index += 1;
    }
    if index == 2 || index >= bytes.len() || bytes[index] != sep {
        return None;
    }
    let share = index + 1;
    let mut end = share;
    while end < bytes.len()
        && bytes[end] != sep
        && !is_path_stop(bytes[end])
        && !bytes[end].is_ascii_whitespace()
    {
        end += 1;
    }
    if end == share {
        None
    } else {
        Some(end)
    }
}

fn find_ci(text: &str, needle: &str) -> Vec<usize> {
    let hay = text.as_bytes();
    let needle = needle.as_bytes();
    let mut found = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return found;
    }
    let mut index = 0;
    while index + needle.len() <= hay.len() {
        if hay[index..index + needle.len()].eq_ignore_ascii_case(needle)
            && text.is_char_boundary(index)
        {
            found.push(index);
            index += needle.len();
        } else {
            index += 1;
        }
    }
    found
}

fn path_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while index < bytes.len() && !is_path_stop(bytes[index]) {
        if bytes[index] == b' ' && starts_with_ascii(&text[index..], " key=") {
            break;
        }
        index += 1;
    }
    while index > start && bytes[index - 1].is_ascii_whitespace() {
        index -= 1;
    }
    index
}

fn find_rom_words(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'.' {
            let ext_start = index + 1;
            let mut ext_end = ext_start;
            while ext_end < bytes.len() && bytes[ext_end].is_ascii_alphanumeric() {
                ext_end += 1;
            }
            if ext_end > ext_start
                && is_rom_extension(&text[ext_start..ext_end])
                && (ext_end == bytes.len() || !is_rom_name_byte(bytes[ext_end]))
            {
                let mut start = index;
                while start > 0 && is_rom_name_byte(bytes[start - 1]) {
                    start -= 1;
                }
                if start < index {
                    found.push((start, ext_end));
                    index = ext_end;
                    continue;
                }
            }
        }
        index += 1;
    }
    found
}

fn is_rom_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'~' | b'(' | b')')
}

fn is_rom_extension(ext: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        "7z", "bin", "chd", "cso", "cue", "gb", "gba", "gbc", "gen", "gg", "iso", "md", "ms",
        "n64", "nes", "pbp", "pce", "sfc", "smc", "smd", "sms", "z64", "zip",
    ];
    EXTENSIONS
        .iter()
        .any(|known| ext.eq_ignore_ascii_case(known))
}

fn is_path_stop(byte: u8) -> bool {
    byte == b'\n' || byte == b'\r' || matches!(byte, b'"' | b'\'' | b'<' | b'>' | b'|')
}

fn username_in(path: &str) -> Option<String> {
    let parts: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    for (index, part) in parts.iter().enumerate() {
        if part.eq_ignore_ascii_case("home") || part.eq_ignore_ascii_case("users") {
            let user = parts.get(index + 1)?.trim();
            if valid_username(user) {
                return Some(user.to_string());
            }
        }
    }
    None
}

fn valid_username(user: &str) -> bool {
    user.len() >= 3
        && user.bytes().any(|byte| byte.is_ascii_alphanumeric())
        && user.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'~' | b' ')
        })
}

fn find_emails(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'@' {
            let mut start = index;
            while start > 0 && is_email_local(bytes[start - 1]) {
                start -= 1;
            }
            let mut end = index + 1;
            while end < bytes.len() && is_email_domain(bytes[end]) {
                end += 1;
            }
            let domain = index + 1;
            if start < index
                && end > domain
                && bytes[domain..end].contains(&b'.')
                && end - start >= 6
            {
                found.push((start, end));
                index = end;
                continue;
            }
        }
        index += 1;
    }
    found
}

fn is_email_local(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_email_domain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')
}

fn find_hyphen_secrets(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_alphanumeric() {
            index += 1;
            continue;
        }
        let start = index;
        let mut parts = 0u32;
        let mut part_len = 0u32;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte.is_ascii_alphanumeric() {
                part_len += 1;
                index += 1;
                continue;
            }
            if byte == b'-' && part_len >= 2 {
                parts += 1;
                part_len = 0;
                index += 1;
                continue;
            }
            break;
        }
        if part_len >= 2 {
            parts += 1;
        }
        if parts >= 4 && part_len >= 2 && index - start >= 8 {
            found.push((start, index));
        }
    }
    found
}

fn find_labeled_secrets(text: &str) -> Vec<(usize, usize)> {
    let labels = ["license_key=", "token=", "secret=", "hmac=", "mac="];
    let mut found = Vec::new();
    for label in labels {
        for at in find_ci(text, label) {
            let value_at = at + label.len();
            let end = labeled_value_end(text, value_at);
            if end >= value_at + 8 {
                found.push((value_at, end));
            }
        }
    }
    found
}

fn labeled_value_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'+' | b'/' | b'=' | b'.' | b'@')
        {
            index += 1;
            continue;
        }
        break;
    }
    index
}

fn redact_secret_json(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'{' {
            if let Some(end) = json_value_end(text, index) {
                if json_slice_has_secret(&text[index..end]) {
                    out.push_str("[redacted]");
                    index = end;
                    continue;
                }
            }
        }
        let ch = text[index..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn json_slice_has_secret(slice: &str) -> bool {
    contains_json_key(slice, "license_key") || contains_json_key(slice, "email")
}

fn contains_json_key(slice: &str, key: &str) -> bool {
    let pattern = format!("\"{key}\"");
    find_ci(slice, &pattern).into_iter().any(|at| {
        let after = at + pattern.len();
        let rest = slice[after..].trim_start();
        rest.starts_with(':')
    })
}

fn json_value_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut index = start;
    let mut in_string = false;
    let mut escape = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn finite_speed(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "0".to_string()
    }
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod scrub_tests {
    use super::{is_device_path, path_end, path_variants, username_in};
    use std::path::Path;

    #[test]
    fn path_end_includes_spaces_until_the_line_ends() {
        let text = "C:\\Users\\Jane Doe\\My Games\\Super Mario.sfc\nnext";
        assert_eq!(
            &text[..path_end(text, 0)],
            "C:\\Users\\Jane Doe\\My Games\\Super Mario.sfc"
        );
    }

    #[test]
    fn path_end_stops_before_a_key_field_and_keeps_a_bare_drive() {
        let text = r"D:\Games\Old Lib\Metroid.gba key=finished disk=2";
        assert_eq!(&text[..path_end(text, 0)], r"D:\Games\Old Lib\Metroid.gba");
        let bare = r"E:\";
        assert_eq!(path_end(bare, 0), bare.len());
    }

    #[test]
    fn username_in_keeps_dos_short_names() {
        assert_eq!(
            username_in(r"C:\Users\JANEDO~1\Games"),
            Some("JANEDO~1".to_string())
        );
    }

    #[test]
    fn extended_drive_paths_are_files_and_other_device_prefixes_are_not() {
        assert!(!is_device_path(r"\\?\C:\support\file.zip"));
        assert!(is_device_path(r"\\?\UNC\nas\share\file.zip"));
        assert!(is_device_path(r"//./COM1"));
        assert!(is_device_path(r"//?/Volume{abc}"));
        assert!(is_device_path(r"\\.\PhysicalDrive1"));
    }

    #[test]
    fn path_variants_include_extended_unc_and_short_names() {
        let drive = path_variants(Path::new(r"D:\Games\Old Lib"));
        assert!(
            drive
                .iter()
                .any(|item| item.eq_ignore_ascii_case(r"\\?\D:\Games\Old Lib")),
            "{drive:?}"
        );
        assert!(
            drive.iter().any(|item| item.contains("OLDLIB~1")),
            "{drive:?}"
        );
        let unc = path_variants(Path::new(r"\\nas\roms"));
        assert!(
            unc.iter()
                .any(|item| item.to_ascii_lowercase().starts_with(r"\\?\unc\")),
            "{unc:?}"
        );
    }
}
