#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    pub id: String,
    pub letter: String,
    pub label: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub is_removable: bool,
    pub is_empty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeDecision {
    Ready { relabel: bool },
    NeedsFormat { reason: String },
    Rejected { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Removable,
    Other,
}

impl DriveKind {
    pub fn from_win32(drive_type: u32) -> Self {
        if drive_type == 2 {
            Self::Removable
        } else {
            Self::Other
        }
    }

    pub fn is_removable(self) -> bool {
        matches!(self, Self::Removable)
    }
}

pub fn allow_copy(volume: &VolumeInfo, expected_label: &str) -> Result<(), String> {
    if !volume.is_removable {
        return Err("only removable drives can receive ROMs".into());
    }
    if !is_fat_family(&volume.file_system) {
        return Err(format!(
            "file system {} is not FAT or exFAT",
            volume.file_system
        ));
    }
    if !labels_match(&volume.label, expected_label) {
        return Err(format!(
            "volume label is '{}', expected {expected_label}",
            volume.label
        ));
    }
    Ok(())
}

pub fn decide(volume: &VolumeInfo, expected_label: &str) -> VolumeDecision {
    if !volume.is_removable {
        return VolumeDecision::Rejected {
            reason: "only removable drives can be prepared".into(),
        };
    }
    if !is_fat_family(&volume.file_system) {
        return VolumeDecision::NeedsFormat {
            reason: format!("file system {} is not FAT or exFAT", volume.file_system),
        };
    }
    let label_ok = labels_match(&volume.label, expected_label);
    if !volume.is_empty && !label_ok {
        return VolumeDecision::NeedsFormat {
            reason: "volume is not empty; formatting erases everything on it".into(),
        };
    }
    VolumeDecision::Ready { relabel: !label_ok }
}

fn is_fat_family(file_system: &str) -> bool {
    matches!(
        file_system.trim().to_ascii_uppercase().as_str(),
        "FAT" | "FAT12" | "FAT16" | "FAT32" | "EXFAT"
    )
}

fn labels_match(actual: &str, expected: &str) -> bool {
    let expected = expected.trim();
    expected.is_empty() || actual.trim().eq_ignore_ascii_case(expected)
}

const IGNORED_ROOT_NAMES: &[&str] = &[
    "System Volume Information",
    "$RECYCLE.BIN",
    "RECYCLER",
    "Recycler",
    ".Spotlight-V100",
    ".fseventsd",
    ".Trashes",
    "desktop.ini",
    "Thumbs.db",
];

pub fn directory_is_empty(path: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ignored = IGNORED_ROOT_NAMES
            .iter()
            .any(|skip| name.eq_ignore_ascii_case(skip));
        if !ignored {
            return false;
        }
    }
    true
}

#[cfg(windows)]
pub fn list_volumes() -> Result<Vec<VolumeInfo>, String> {
    use windows::{
        core::PCWSTR,
        Win32::Storage::FileSystem::{
            GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        },
    };

    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Err("Windows did not return a drive list".into());
    }

    let mut volumes = Vec::new();
    for index in 0..26 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = (b'A' + index) as char;
        let root = format!("{letter}:\\");
        let wide = wide_null(&root);
        let kind = DriveKind::from_win32(unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) });
        if !kind.is_removable() {
            continue;
        }

        let mut name_buf = [0u16; 64];
        let mut fs_buf = [0u16; 64];
        let info_ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(wide.as_ptr()),
                Some(&mut name_buf),
                None,
                None,
                None,
                Some(&mut fs_buf),
            )
        };
        let (label, file_system) = if info_ok.is_ok() {
            (wide_string(&name_buf), wide_string(&fs_buf))
        } else {
            (String::new(), String::new())
        };

        let mut total_bytes = 0u64;
        let space_ok = unsafe {
            GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), None, Some(&mut total_bytes), None)
        };
        if space_ok.is_err() {
            total_bytes = 0;
        }

        volumes.push(VolumeInfo {
            id: format!("{letter}:"),
            letter: letter.to_string(),
            label,
            file_system,
            total_bytes,
            is_removable: true,
            is_empty: directory_is_empty(std::path::Path::new(&root)),
        });
    }
    Ok(volumes)
}

#[cfg(not(windows))]
pub fn list_volumes() -> Result<Vec<VolumeInfo>, String> {
    Err("removable drive listing is implemented for Windows".into())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn wide_string(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end]).trim().to_string()
}
