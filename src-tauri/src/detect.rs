//! Read-only identification of a stock handheld card.
//!
//! A match needs a DTB file name and sha256 from the profile feed plus the
//! partition layout (count, type, label, and size). Detection never opens a
//! disk for writing and never calls the elevated flash helper. Boot files are
//! read from a mounted directory. Disk 0, system disks, boot disks, size 0,
//! and anything that is not USB or SD are ignored before that read.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::block_device::is_physical_disk_path;
use crate::profiles::{Profile, StockVariant};

pub const MANUAL_DEVICE_MESSAGE: &str =
    "We couldn't tell which handheld this is. Please pick it from the list.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedDtb {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedPartition {
    pub type_code: String,
    pub label: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub dtbs: Vec<ObservedDtb>,
    pub partitions: Vec<ObservedPartition>,
    pub unreadable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    None,
    Manual,
    Match {
        profile_id: String,
        device: String,
        screen: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionView {
    pub status: String,
    pub profile_id: String,
    pub device: String,
    pub screen: String,
    pub profile_name: String,
    pub message: String,
}

/// A file handle opened read-only. `try_write` always refuses, and the
/// operating-system open never requests write access.
#[derive(Debug)]
pub struct ReadOnlyFile {
    file: fs::File,
}

impl ReadOnlyFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        if is_physical_disk_path(path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "refusing a physical disk path",
            ));
        }
        let meta = fs::symlink_metadata(path)?;
        let kind = meta.file_type();
        if kind.is_symlink() || !kind.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "refusing a path that is not a regular file",
            ));
        }
        let file = OpenOptions::new().read(true).open(path)?;
        Ok(Self { file })
    }

    pub fn read_to_end(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    pub fn try_write(&mut self, data: &[u8]) -> io::Result<()> {
        let _ = (data, &self.file);
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "stock-card detection is read-only",
        ))
    }
}

pub fn identify(profiles: &[Profile], cards: &[Observation]) -> Detection {
    if cards.is_empty() {
        return Detection::None;
    }
    let mut hits = Vec::new();
    let mut unknown = 0usize;
    for card in cards {
        let matched = matching_variants(profiles, card);
        if matched.is_empty() {
            unknown += 1;
        } else {
            hits.extend(matched);
        }
    }
    if unknown > 0 || hits.len() != 1 {
        return Detection::Manual;
    }
    let hit = hits.remove(0);
    Detection::Match {
        profile_id: hit.profile_id,
        device: hit.device,
        screen: hit.screen,
    }
}

struct Hit {
    profile_id: String,
    device: String,
    screen: String,
}

fn matching_variants(profiles: &[Profile], card: &Observation) -> Vec<Hit> {
    if card.unreadable {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for profile in profiles {
        let Some(fingerprint) = &profile.stock_fingerprint else {
            continue;
        };
        if fingerprint.manual_only {
            continue;
        }
        for variant in &fingerprint.variants {
            if variant_matches(variant, card) {
                hits.push(Hit {
                    profile_id: profile.id.clone(),
                    device: variant.device.clone(),
                    screen: variant.screen.clone(),
                });
            }
        }
    }
    hits
}

fn variant_matches(variant: &StockVariant, card: &Observation) -> bool {
    if variant.partitions.len() != card.partitions.len() || variant.dtbs.is_empty() {
        return false;
    }
    for (expected, actual) in variant.partitions.iter().zip(&card.partitions) {
        if !expected.type_code.eq_ignore_ascii_case(&actual.type_code)
            || !expected.label.eq_ignore_ascii_case(&actual.label)
            || actual.size_bytes == 0
        {
            return false;
        }
        if expected.size_bytes != 0 && expected.size_bytes != actual.size_bytes {
            return false;
        }
    }
    variant.dtbs.iter().all(|expected| {
        card.dtbs.iter().any(|actual| {
            actual.name.eq_ignore_ascii_case(&expected.name)
                && actual.sha256.eq_ignore_ascii_case(&expected.sha256)
        })
    })
}

pub fn read_boot_dtbs(dir: &Path) -> Result<Vec<ObservedDtb>, String> {
    if is_physical_disk_path(dir) {
        return Err("refusing a physical disk path".into());
    }
    let meta = fs::symlink_metadata(dir)
        .map_err(|error| format!("could not read {}: {error}", dir.display()))?;
    let kind = meta.file_type();
    if kind.is_symlink() || !kind.is_dir() {
        return Err(format!("{} is not a boot directory", dir.display()));
    }
    let mut dtbs = Vec::new();
    let entries =
        fs::read_dir(dir).map_err(|error| format!("could not list {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not list {}: {error}", dir.display()))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!("refusing to follow symlink {}", path.display()));
        }
        if !meta.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_ascii_lowercase().ends_with(".dtb") {
            continue;
        }
        let mut file = ReadOnlyFile::open(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let bytes = file
            .read_to_end()
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        dtbs.push(ObservedDtb {
            name,
            sha256: sha256_hex(&bytes),
        });
    }
    dtbs.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    Ok(dtbs)
}

pub fn parse_layout_descriptor(text: &str) -> Result<Vec<ObservedPartition>, String> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("layout descriptor is not JSON: {error}"))?;
    let rows = value
        .get("partitions")
        .and_then(|item| item.as_array())
        .ok_or("layout descriptor needs a partitions array")?;
    let mut partitions = Vec::new();
    for row in rows {
        let type_code = row
            .get("type")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let label = row
            .get("label")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string();
        let size_bytes = row
            .get("sizeBytes")
            .and_then(|item| item.as_u64())
            .unwrap_or(0);
        if type_code.len() != 2 || label.is_empty() || size_bytes == 0 {
            return Err("a layout partition needs a type, a label, and a positive size".into());
        }
        partitions.push(ObservedPartition {
            type_code,
            label,
            size_bytes,
        });
    }
    if partitions.is_empty() {
        return Err("layout descriptor has no partitions".into());
    }
    Ok(partitions)
}

#[derive(Debug)]
struct PartitionRow {
    disk_number: u32,
    bus: String,
    is_system: bool,
    is_boot: bool,
    disk_size: u64,
    partition_number: u32,
    type_code: String,
    size_bytes: u64,
    letter: Option<char>,
    label: String,
}

/// Static read-only query. No disk number from the user is interpolated.
pub fn partition_query_script() -> &'static str {
    "Get-Disk | Where-Object { ($_.BusType -eq 'USB' -or $_.BusType -eq 'SD') -and $_.Number -ne 0 -and -not $_.IsSystem -and -not $_.IsBoot -and $_.Size -gt 0 } | ForEach-Object { $disk = $_; Get-Partition -DiskNumber $disk.Number | ForEach-Object { $vol = $_ | Get-Volume -ErrorAction SilentlyContinue; [PSCustomObject]@{ DiskNumber = $disk.Number; BusType = [string]$disk.BusType; IsSystem = [bool]$disk.IsSystem; IsBoot = [bool]$disk.IsBoot; DiskSize = [uint64]$disk.Size; PartitionNumber = $_.PartitionNumber; MbrType = $(if ($null -eq $_.MbrType) { 0 } else { [int]$_.MbrType }); Size = [uint64]$_.Size; DriveLetter = if ($_.DriveLetter) { [string]$_.DriveLetter } else { $null }; Label = if ($vol) { [string]$vol.FileSystemLabel } else { '' } } } } | ConvertTo-Json -Compress"
}

pub fn parse_partition_snapshot(
    json: &str,
    mut read_letter: impl FnMut(char) -> Result<Vec<ObservedDtb>, String>,
) -> Result<Vec<Observation>, String> {
    let text = json.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("could not read the partition list: {error}"))?;
    let rows = if value.is_array() {
        value.as_array().cloned().unwrap_or_default()
    } else if value.is_object() {
        vec![value]
    } else {
        return Err("partition list was not a JSON object".into());
    };
    let mut by_disk: BTreeMap<u32, Vec<PartitionRow>> = BTreeMap::new();
    for row in rows {
        let parsed = parse_row(&row)?;
        by_disk.entry(parsed.disk_number).or_default().push(parsed);
    }
    let mut observations = Vec::new();
    for group in by_disk.into_values() {
        if group.iter().any(|row| !disk_is_readable(row)) {
            continue;
        }
        let mut parts = group;
        parts.sort_by_key(|row| row.partition_number);
        parts.retain(|row| row.size_bytes > 0);
        let boot_letters: Vec<char> = parts
            .iter()
            .filter(|row| row.label.eq_ignore_ascii_case("BOOT"))
            .filter_map(|row| row.letter)
            .collect();
        let (dtbs, unreadable) = if boot_letters.len() == 1 {
            match read_letter(boot_letters[0]) {
                Ok(dtbs) => (dtbs, false),
                Err(_) => (Vec::new(), true),
            }
        } else {
            (Vec::new(), true)
        };
        observations.push(Observation {
            dtbs,
            partitions: parts
                .into_iter()
                .map(|row| ObservedPartition {
                    type_code: row.type_code,
                    label: row.label,
                    size_bytes: row.size_bytes,
                })
                .collect(),
            unreadable,
        });
    }
    Ok(observations)
}

fn disk_is_readable(row: &PartitionRow) -> bool {
    row.disk_number != 0
        && !row.is_system
        && !row.is_boot
        && row.disk_size > 0
        && (row.bus.eq_ignore_ascii_case("USB") || row.bus.eq_ignore_ascii_case("SD"))
}

fn parse_row(row: &serde_json::Value) -> Result<PartitionRow, String> {
    let disk_number = json_u32(row, "DiskNumber").unwrap_or(0);
    let bus = row
        .get("BusType")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    let is_system = json_flag(row, "IsSystem");
    let is_boot = json_flag(row, "IsBoot");
    let disk_size = json_u64(row, "DiskSize").unwrap_or(0);
    let partition_number = json_u32(row, "PartitionNumber").unwrap_or(0);
    let mbr_type = json_u32(row, "MbrType").unwrap_or(0);
    let size_bytes = json_u64(row, "Size").unwrap_or(0);
    let letter = row
        .get("DriveLetter")
        .and_then(|item| item.as_str())
        .filter(|text| !text.is_empty())
        .and_then(|text| text.chars().next())
        .filter(|ch| ch.is_ascii_alphabetic());
    let label = row
        .get("Label")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(PartitionRow {
        disk_number,
        bus,
        is_system,
        is_boot,
        disk_size,
        partition_number,
        type_code: format!("{mbr_type:02x}"),
        size_bytes,
        letter,
        label,
    })
}

fn json_flag(row: &serde_json::Value, key: &str) -> bool {
    match row.get(key) {
        Some(value) if value.as_bool() == Some(true) => true,
        Some(value) => value
            .as_str()
            .is_some_and(|text| text.eq_ignore_ascii_case("true") || text == "1"),
        None => false,
    }
}

fn json_u32(row: &serde_json::Value, key: &str) -> Option<u32> {
    json_u64(row, key).and_then(|value| u32::try_from(value).ok())
}

fn json_u64(row: &serde_json::Value, key: &str) -> Option<u64> {
    let value = row.get(key)?;
    if let Some(number) = value.as_u64() {
        return Some(number);
    }
    value.as_str().and_then(|text| text.parse().ok())
}

pub fn view(profiles: &[Profile], detection: Detection) -> DetectionView {
    match detection {
        Detection::None => DetectionView {
            status: "none".into(),
            profile_id: String::new(),
            device: String::new(),
            screen: String::new(),
            profile_name: String::new(),
            message: String::new(),
        },
        Detection::Manual => DetectionView {
            status: "manual".into(),
            profile_id: String::new(),
            device: String::new(),
            screen: String::new(),
            profile_name: String::new(),
            message: MANUAL_DEVICE_MESSAGE.into(),
        },
        Detection::Match {
            profile_id,
            device,
            screen,
        } => {
            let profile_name = profiles
                .iter()
                .find(|profile| profile.id == profile_id)
                .map(|profile| profile.name.clone())
                .unwrap_or_default();
            DetectionView {
                status: "matched".into(),
                profile_id,
                device,
                screen,
                profile_name,
                message: String::new(),
            }
        }
    }
}

/// Every shipped profile is manual-only or has fingerprints, and no two
/// variants share a DTB-plus-layout key.
pub fn audit_stock_fingerprints(profiles: &[Profile]) -> Result<(), String> {
    if profiles.is_empty() {
        return Err("no profiles to audit".into());
    }
    let mut seen = BTreeMap::<String, String>::new();
    for profile in profiles {
        let Some(fingerprint) = &profile.stock_fingerprint else {
            return Err(format!(
                "{} has no stockFingerprint; mark it manualOnly or add a fingerprint",
                profile.id
            ));
        };
        if fingerprint.manual_only {
            if !fingerprint.variants.is_empty() {
                return Err(format!(
                    "{} is manual-only but still has fingerprints",
                    profile.id
                ));
            }
            continue;
        }
        if fingerprint.variants.is_empty() {
            return Err(format!(
                "{} is not manual-only and has no fingerprints",
                profile.id
            ));
        }
        for variant in &fingerprint.variants {
            if variant.device.trim().is_empty() || variant.screen.trim().is_empty() {
                return Err(format!("{} has a blank device or screen", profile.id));
            }
            if variant.dtbs.is_empty() || variant.partitions.is_empty() {
                return Err(format!(
                    "{} fingerprint for {} is missing dtbs or partitions",
                    profile.id, variant.screen
                ));
            }
            let key = variant_key(variant);
            if let Some(other) = seen.insert(key, profile.id.clone()) {
                return Err(format!(
                    "fingerprint collision between {other} and {}",
                    profile.id
                ));
            }
        }
    }
    Ok(())
}

pub fn variant_key(variant: &StockVariant) -> String {
    let mut dtbs: Vec<String> = variant
        .dtbs
        .iter()
        .map(|dtb| {
            format!(
                "{}:{}",
                dtb.name.to_ascii_lowercase(),
                dtb.sha256.to_ascii_lowercase()
            )
        })
        .collect();
    dtbs.sort();
    let parts: Vec<String> = variant
        .partitions
        .iter()
        .map(|part| {
            format!(
                "{}:{}:{}",
                part.type_code.to_ascii_lowercase(),
                part.label.to_ascii_lowercase(),
                part.size_bytes
            )
        })
        .collect();
    format!("dtb={}|part={}", dtbs.join(","), parts.join(","))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Live card scan. On non-Windows there is no removable-disk query, so the
/// caller shows the manual picker only when a card was actually read.
pub fn scan_observations() -> Result<Vec<Observation>, String> {
    #[cfg(windows)]
    {
        scan_observations_windows()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
fn scan_observations_windows() -> Result<Vec<Observation>, String> {
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", partition_query_script()])
        .output()
        .map_err(|error| format!("could not list partitions: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_partition_snapshot(text.as_ref(), |letter| {
        let dir = std::path::PathBuf::from(format!("{letter}:\\"));
        read_boot_dtbs(&dir)
    })
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_of_empty_is_well_known() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
