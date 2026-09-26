use std::fs;
use std::path::Path;

/// Files the dArkOS `expandtoexfat.sh` removes after it finishes formatting
/// EASYROMS. Deleting them is the disarm. Creating `/boot/doneit` is not:
/// that file only skips the first-stage reboot and lets the same script
/// `mkfs.exfat` the games partition.
///
/// Source: https://github.com/southoz/dArkOSRE-R36/blob/main/files/BOOT/expandtoexfat.sh
const WIPE_SCRIPTS: &[&str] = &["expandtoexfat.sh", "firstboot.sh"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisarmReport {
    pub removed: Vec<String>,
}

pub fn user_roms_are_safe(boot: &Path) -> Result<(), String> {
    let armed: Vec<&str> = WIPE_SCRIPTS
        .iter()
        .copied()
        .filter(|name| boot.join(name).is_file())
        .collect();
    if armed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "firstboot is still armed ({}); it will format EASYROMS and extract roms.tar",
            armed.join(", ")
        ))
    }
}

pub fn disarm_easyroms_firstboot(boot: &Path) -> Result<DisarmReport, String> {
    if !boot.is_dir() {
        return Err(format!("{} is not a boot directory", boot.display()));
    }
    if !boot.join("expandtoexfat.sh").is_file() {
        return Err(
            "expandtoexfat.sh is not in this boot folder, so this is not an armed ArkOS/dArkOS firstboot"
                .into(),
        );
    }
    let mut removed = Vec::new();
    for name in WIPE_SCRIPTS {
        let path = boot.join(name);
        if path.is_file() {
            fs::remove_file(&path).map_err(|error| format!("could not remove {name}: {error}"))?;
            removed.push((*name).to_string());
        }
    }
    Ok(DisarmReport { removed })
}

/// Whether ROMs may be copied onto an ArkOS/dArkOS single-card EASYROMS volume.
///
/// A just-flashed card exposes a small FAT32 placeholder already labeled
/// EASYROMS, so the volume label alone cannot tell it from the expanded
/// volume. The BOOT partition of the same disk can: while the wipe scripts
/// are still there, firstboot has not run and will format EASYROMS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardSafety {
    /// The BOOT partition holds no wipe scripts; firstboot has already run.
    Safe,
    /// The BOOT partition still holds the wipe scripts, so firstboot will
    /// format EASYROMS and erase anything copied onto it now.
    Armed(String),
    /// No BOOT partition was found next to EASYROMS, or it could not be read.
    Unknown(String),
}

impl CardSafety {
    pub fn is_safe(&self) -> bool {
        matches!(self, CardSafety::Safe)
    }
}

/// The lettered volumes of one disk as `(drive letter, volume label)`.
pub type DiskVolumes = Vec<(char, String)>;

/// Pick the BOOT-labeled volume among the lettered volumes of one disk.
pub fn boot_letter(disk_volumes: &DiskVolumes) -> Option<char> {
    disk_volumes
        .iter()
        .find(|(_, label)| label.trim().eq_ignore_ascii_case("BOOT"))
        .map(|(letter, _)| *letter)
}

/// Judge a card from the lettered volumes of its disk plus the BOOT root.
///
/// `boot_root` is `Some` only for the volume `boot_letter` picked; passing a
/// different root is a caller bug the check will still report honestly.
pub fn card_safety(disk_volumes: &DiskVolumes, boot_root: Option<&Path>) -> CardSafety {
    let Some(letter) = boot_letter(disk_volumes) else {
        return CardSafety::Unknown(
            "no BOOT volume is visible next to EASYROMS on this card".into(),
        );
    };
    let Some(root) = boot_root else {
        return CardSafety::Unknown(format!("could not open the BOOT volume {letter}:"));
    };
    match user_roms_are_safe(root) {
        Ok(()) => CardSafety::Safe,
        Err(reason) => CardSafety::Armed(reason),
    }
}
