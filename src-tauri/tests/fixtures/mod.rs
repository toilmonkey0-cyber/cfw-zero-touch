//! Disk-image fixtures for probe, backup, flash, and firstboot tests.
//!
//! Images are generated in memory and written only through [`crate`]'s caller
//! into regular files. This module has no path that opens a physical disk.
//!
//! Layout, cited from the dArkOS first-boot scripts rather than invented:
//!
//! - p1 `LABEL=BOOT`, FAT, mounted at `/boot` on the device.
//!   fstab in [dArkOSen `reset_firstboot.sh`](https://github.com/djparentx/dArkOSen-R36S/blob/85cae786/reset_firstboot.sh).
//! - p2 `LABEL=ROOTFS`. The real partition is ext4 or btrfs. The fixture marks
//!   type `0x83` and does not pretend to be either filesystem.
//! - p3 EASYROMS. [`expandtoexfat.sh`](https://github.com/southoz/dArkOSRE-R36/blob/main/files/BOOT/expandtoexfat.sh)
//!   runs `mkfs.exfat -L EASYROMS` on `mmcblk0p3` and finishes with fdisk type
//!   `7` (`0x07`). While `expandtoexfat.sh` and `firstboot.sh` are on BOOT,
//!   first boot will format EASYROMS. `doneit` does not disarm that.
//!
//! DTB filenames are the public names, with synthetic payloads (not device
//! trees): original-card files from ArkOS-R3XS issue 110
//! (`rk3326-rg351mp-linux.dtb`, `rg351mp-kernel.dtb`, `gameconsole-r36s.dtb`)
//! and clone files from ArkOS4Clone (`rk3326-r36s-linux.dtb`,
//! `rk3326-r36s-type4-linux.dtb`). `console_detect` reads the active name out
//! of `boot.ini`.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

mod card;
pub mod exfat;
pub mod fat16;

pub use card::{
    boot_file, build_card, build_wrap_card, classify_dtb, extract_boot, lettered_volumes,
    probe_image, read_easyroms, wrap_reported_len, BuiltCard, CardOptions, DtbSet, Probe,
    ProbedPartition, BOOT_PARTITION_TYPE, EASYROMS_PARTITION_TYPE, ROOTFS_PARTITION_TYPE,
};

pub(crate) fn read_u16(buf: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = buf
        .get(offset..offset + 2)
        .ok_or_else(|| format!("need 2 bytes at {offset}"))?;
    Ok(u16::from_le_bytes(bytes.try_into().expect("2 bytes")))
}

static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

/// Temp directory removed when dropped. Tests never put this on a real disk.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(label: &str) -> Self {
        let seq = SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cfw-{label}-{}-{}-{seq}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("create fixture scratch dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn safe_relative(path: &str) -> Result<PathBuf, String> {
    let relative = PathBuf::from(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "fixture path escapes the extract directory: {path}"
        ));
    }
    Ok(relative)
}
