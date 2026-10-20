//! MBR cards with a FAT BOOT partition and an exFAT EASYROMS partition.
//!
//! Partition numbers match the device names in `expandtoexfat.sh`: p1 BOOT,
//! p2 ROOTFS, p3 EASYROMS. Alignment is 64 sectors. A real image uses 1 MiB
//! alignment; the smaller alignment keeps the generated file small and does
//! not change the partition numbers or filesystem labels.

use std::fs;
use std::path::Path;

use super::exfat;
use super::fat16;
use super::safe_relative;

pub const BOOT_PARTITION_TYPE: u8 = 0x0E;
pub const ROOTFS_PARTITION_TYPE: u8 = 0x83;
pub const EASYROMS_PARTITION_TYPE: u8 = 0x07;
const ALIGN: u32 = 64;
const ROOTFS_SECTORS: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtbSet {
    Original,
    Clone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardOptions {
    pub armed: bool,
    pub dtb: DtbSet,
}

impl CardOptions {
    pub fn armed_original() -> Self {
        Self {
            armed: true,
            dtb: DtbSet::Original,
        }
    }

    pub fn armed_clone() -> Self {
        Self {
            armed: true,
            dtb: DtbSet::Clone,
        }
    }

    pub fn disarmed_original() -> Self {
        Self {
            armed: false,
            dtb: DtbSet::Original,
        }
    }

    pub fn disarmed_clone() -> Self {
        Self {
            armed: false,
            dtb: DtbSet::Clone,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltCard {
    pub bytes: Vec<u8>,
    pub reported_len: u64,
}

impl BuiltCard {
    pub fn stored_len(&self) -> u64 {
        u64::try_from(self.bytes.len()).expect("image length")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedPartition {
    pub number: u32,
    pub type_code: u8,
    pub start_lba: u32,
    pub sector_count: u32,
    pub filesystem: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub partitions: Vec<ProbedPartition>,
}

pub fn build_card(options: CardOptions) -> BuiltCard {
    let bytes = assemble(options);
    let reported_len = u64::try_from(bytes.len()).expect("image length");
    BuiltCard {
        bytes,
        reported_len,
    }
}

/// Same bytes as an armed original card, claiming four times the real size.
pub fn build_wrap_card() -> BuiltCard {
    let card = build_card(CardOptions::armed_original());
    BuiltCard {
        bytes: card.bytes,
        reported_len: wrap_reported_len(card.reported_len),
    }
}

pub fn wrap_reported_len(stored_len: u64) -> u64 {
    stored_len
        .saturating_mul(4)
        .max(stored_len.saturating_add(1))
}

pub fn probe_image(image: &[u8]) -> Result<Probe, String> {
    if image.len() < 512 || image[510] != 0x55 || image[511] != 0xAA {
        return Err("disk image has no MBR signature".into());
    }
    let mut partitions = Vec::new();
    for slot in 0..4 {
        let at = 446 + slot * 16;
        let type_code = image[at + 4];
        if type_code == 0 {
            continue;
        }
        let start_lba = u32::from_le_bytes(image[at + 8..at + 12].try_into().expect("lba"));
        let sector_count = u32::from_le_bytes(image[at + 12..at + 16].try_into().expect("count"));
        let slice = partition_bytes(image, start_lba, sector_count)?;
        let (filesystem, label) = identify(slice);
        partitions.push(ProbedPartition {
            number: u32::try_from(slot + 1).expect("partition number"),
            type_code,
            start_lba,
            sector_count,
            filesystem,
            label,
        });
    }
    Ok(Probe { partitions })
}

pub fn extract_boot(image: &[u8], dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest)
        .map_err(|error| format!("could not create {}: {error}", dest.display()))?;
    for (path, bytes) in boot_files(image)? {
        let relative = safe_relative(&path)?;
        let full = dest.join(relative);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        fs::write(&full, bytes)
            .map_err(|error| format!("could not write {}: {error}", full.display()))?;
    }
    Ok(())
}

pub fn boot_file(image: &[u8], name: &str) -> Result<Vec<u8>, String> {
    boot_files(image)?
        .into_iter()
        .find(|(path, _)| path == name)
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| format!("{name} is not on the BOOT partition"))
}

pub fn read_easyroms(image: &[u8], path: &str) -> Result<Vec<u8>, String> {
    let slice = slice_labeled(image, "EASYROMS")?;
    exfat::list_files(slice)?
        .into_iter()
        .find(|(name, _)| name == path)
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| format!("{path} is not on EASYROMS"))
}

pub fn lettered_volumes(probe: &Probe) -> Vec<(char, String)> {
    let mut volumes = Vec::new();
    for part in &probe.partitions {
        if part.label.eq_ignore_ascii_case("BOOT") {
            volumes.push(('D', part.label.clone()));
        } else if part.label.eq_ignore_ascii_case("EASYROMS") {
            volumes.push(('E', part.label.clone()));
        }
    }
    volumes
}

pub fn classify_dtb<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<DtbSet, String> {
    let names: Vec<&str> = names.into_iter().collect();
    let clone = names
        .iter()
        .any(|name| name.contains("r36s-type") || *name == "rk3326-r36s-linux.dtb");
    let original = names
        .iter()
        .any(|name| *name == "rk3326-rg351mp-linux.dtb" || *name == "gameconsole-r36s.dtb");
    match (original, clone) {
        (true, false) => Ok(DtbSet::Original),
        (false, true) => Ok(DtbSet::Clone),
        _ => Err(format!("DTB set is mixed or missing: {names:?}")),
    }
}

fn boot_files(image: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let slice = slice_labeled(image, "BOOT")?;
    fat16::list_files(slice)
}

fn slice_labeled<'a>(image: &'a [u8], label: &str) -> Result<&'a [u8], String> {
    let probe = probe_image(image)?;
    let part = probe
        .partitions
        .iter()
        .find(|part| part.label.eq_ignore_ascii_case(label))
        .ok_or_else(|| format!("{label} partition is missing"))?;
    partition_bytes(image, part.start_lba, part.sector_count)
}

fn partition_bytes(image: &[u8], start_lba: u32, sectors: u32) -> Result<&[u8], String> {
    let start = start_lba as usize * 512;
    let len = sectors as usize * 512;
    image
        .get(start..start + len)
        .ok_or_else(|| format!("partition at LBA {start_lba} does not fit in the image"))
}

fn identify(slice: &[u8]) -> (String, String) {
    if slice.len() >= 11 && &slice[3..11] == b"EXFAT   " {
        let label = exfat::volume_label(slice).unwrap_or_default();
        return ("exfat".into(), label);
    }
    if slice.len() >= 512 && slice[510] == 0x55 && slice[511] == 0xAA {
        let kind = fat_kind(slice);
        if let Some(kind) = kind {
            let label = fat16::volume_label(slice).unwrap_or_default();
            return (kind, label);
        }
    }
    ("unknown".into(), String::new())
}

fn fat_kind(slice: &[u8]) -> Option<String> {
    let at54 = std::str::from_utf8(slice.get(54..62)?).ok()?.trim();
    let at82 = std::str::from_utf8(slice.get(82..90)?).ok()?.trim();
    for text in [at54, at82] {
        let upper = text.to_ascii_uppercase();
        if upper.starts_with("FAT16") {
            return Some("fat16".into());
        }
        if upper.starts_with("FAT32") {
            return Some("fat32".into());
        }
        if upper.starts_with("FAT12") {
            return Some("fat12".into());
        }
    }
    None
}

fn assemble(options: CardOptions) -> Vec<u8> {
    let boot_lba = ALIGN;
    let boot = fat16::format_volume(&boot_nodes(options), "BOOT", boot_lba);
    let boot_sectors = u32::try_from(boot.len() / 512).expect("boot sectors");
    let root_lba = align_up(boot_lba + boot_sectors, ALIGN);
    let easy_lba = align_up(root_lba + ROOTFS_SECTORS, ALIGN);
    let easy = exfat::format_volume(&easy_nodes(), "EASYROMS", u64::from(easy_lba));
    let easy_sectors = u32::try_from(easy.len() / 512).expect("easyroms sectors");
    let total = easy_lba + easy_sectors;
    let mut disk = vec![0u8; total as usize * 512];
    write_mbr(
        &mut disk,
        [
            (BOOT_PARTITION_TYPE, boot_lba, boot_sectors),
            (ROOTFS_PARTITION_TYPE, root_lba, ROOTFS_SECTORS),
            (EASYROMS_PARTITION_TYPE, easy_lba, easy_sectors),
        ],
    );
    copy_at(&mut disk, boot_lba, &boot);
    copy_at(&mut disk, root_lba, rootfs_note().as_bytes());
    copy_at(&mut disk, easy_lba, &easy);
    disk
}

fn align_up(value: u32, align: u32) -> u32 {
    value.div_ceil(align) * align
}

fn copy_at(disk: &mut [u8], lba: u32, bytes: &[u8]) {
    let start = lba as usize * 512;
    disk[start..start + bytes.len()].copy_from_slice(bytes);
}

fn write_mbr(disk: &mut [u8], parts: [(u8, u32, u32); 3]) {
    for (index, (type_code, start, sectors)) in parts.iter().enumerate() {
        let at = 446 + index * 16;
        disk[at + 1] = 0xFE;
        disk[at + 2] = 0xFF;
        disk[at + 3] = 0xFF;
        disk[at + 4] = *type_code;
        disk[at + 5] = 0xFE;
        disk[at + 6] = 0xFF;
        disk[at + 7] = 0xFF;
        disk[at + 8..at + 12].copy_from_slice(&start.to_le_bytes());
        disk[at + 12..at + 16].copy_from_slice(&sectors.to_le_bytes());
    }
    disk[510] = 0x55;
    disk[511] = 0xAA;
}

fn rootfs_note() -> String {
    "\
CFW-FIXTURE-ROOTFS
This is partition 2, the ROOTFS slot from the dArkOS fstab.
It is not an ext4 or btrfs filesystem.
"
    .to_string()
}

fn boot_nodes(options: CardOptions) -> Vec<fat16::Node> {
    let mut nodes = Vec::new();
    if options.armed {
        nodes.push(fat16::file(
            "expandtoexfat.sh",
            short_for("expandtoexfat.sh"),
            EXPAND_SCRIPT,
        ));
        nodes.push(fat16::file(
            "firstboot.sh",
            short_for("firstboot.sh"),
            FIRSTBOOT_SCRIPT,
        ));
        nodes.push(fat16::file("doneit", short_for("doneit"), "marker\n"));
    }
    let active = match options.dtb {
        DtbSet::Original => "rk3326-rg351mp-linux.dtb",
        DtbSet::Clone => "rk3326-r36s-type4-linux.dtb",
    };
    nodes.push(fat16::file(
        "boot.ini",
        short_for("boot.ini"),
        format!(
            "# CFW fixture boot.ini. Not a bootloader script.\n\
             # ArkOS4Clone console_detect reads the dtb name from this line.\n\
             load mmc 1:1 ${{dtb_loadaddr}} {active}\n"
        ),
    ));
    for name in dtb_names(options.dtb) {
        nodes.push(fat16::file(
            name,
            short_for(name),
            format!(
                "CFW-FIXTURE-DTB\nset={}\nfile={name}\n",
                dtb_label(options.dtb)
            ),
        ));
    }
    nodes
}

fn dtb_names(set: DtbSet) -> &'static [&'static str] {
    match set {
        DtbSet::Original => &[
            "rk3326-rg351mp-linux.dtb",
            "rg351mp-kernel.dtb",
            "gameconsole-r36s.dtb",
        ],
        DtbSet::Clone => &["rk3326-r36s-linux.dtb", "rk3326-r36s-type4-linux.dtb"],
    }
}

fn dtb_label(set: DtbSet) -> &'static str {
    match set {
        DtbSet::Original => "original",
        DtbSet::Clone => "clone",
    }
}

fn short_for(name: &str) -> &'static str {
    match name {
        "expandtoexfat.sh" => "EXPAND~1.SH",
        "firstboot.sh" => "FIRSTB~1.SH",
        "doneit" => "DONEIT",
        "boot.ini" => "BOOT.INI",
        "rk3326-rg351mp-linux.dtb" => "RK3326~1.DTB",
        "rg351mp-kernel.dtb" => "RG351M~1.DTB",
        "gameconsole-r36s.dtb" => "GAMECO~1.DTB",
        "rk3326-r36s-linux.dtb" => "RK3326~2.DTB",
        "rk3326-r36s-type4-linux.dtb" => "RK3326~3.DTB",
        other => panic!("no 8.3 name for fixture file {other}"),
    }
}

fn easy_nodes() -> Vec<exfat::Node> {
    vec![exfat::dir(
        "gba",
        vec![exfat::file("game.gba", b"keep-me".as_slice())],
    )]
}

const EXPAND_SCRIPT: &str = "\
#!/bin/sh
# Fixture stand-in for dArkOS BOOT/expandtoexfat.sh.
# The real script runs mkfs.exfat -L EASYROMS on mmcblk0p3.
# Source: https://github.com/southoz/dArkOSRE-R36/blob/main/files/BOOT/expandtoexfat.sh
echo fixture-armed
";

const FIRSTBOOT_SCRIPT: &str = "\
#!/bin/sh
# Fixture stand-in for /boot/firstboot.sh, which runs expandtoexfat.sh.
/boot/expandtoexfat.sh
";
