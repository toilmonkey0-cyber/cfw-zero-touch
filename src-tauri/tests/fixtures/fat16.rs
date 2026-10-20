//! FAT16 volume builder and reader for fixture images.
//!
//! A spec-valid FAT32 volume needs at least 65,525 clusters (about 32 MiB at
//! 512 bytes per cluster). These fixtures use FAT16, which is valid at 4,085
//! clusters, so a generated card stays near 2 MiB. The real ArkOS BOOT
//! partition is FAT32 (`vfat`, `LABEL=BOOT` in the dArkOS fstab). The fixture
//! keeps that label, the wipe-script names, and the DTB names; the filesystem
//! family is still FAT.

use super::read_u16;

const SECTOR: usize = 512;
const MIN_FAT16_CLUSTERS: usize = 4085;
const ROOT_ENTRIES: usize = 512;
const RESERVED_SECTORS: u16 = 1;
const FAT_COUNT: u8 = 2;

#[derive(Debug)]
pub struct Node {
    pub name: String,
    pub short_name: String,
    kind: NodeKind,
}

#[derive(Debug)]
enum NodeKind {
    File(Vec<u8>),
    Dir(Vec<Node>),
}

pub fn file(name: &str, short_name: &str, bytes: impl Into<Vec<u8>>) -> Node {
    Node {
        name: name.to_string(),
        short_name: short_name.to_string(),
        kind: NodeKind::File(bytes.into()),
    }
}

pub fn dir(name: &str, short_name: &str, children: Vec<Node>) -> Node {
    Node {
        name: name.to_string(),
        short_name: short_name.to_string(),
        kind: NodeKind::Dir(children),
    }
}

struct Volume {
    fat: Vec<u16>,
    data: Vec<u8>,
}

impl Volume {
    fn new() -> Self {
        Self {
            fat: vec![0xFFF8, 0xFFFF],
            data: Vec::new(),
        }
    }

    fn alloc(&mut self, bytes: &[u8]) -> u16 {
        let clusters = bytes.len().div_ceil(SECTOR).max(1);
        let first = u16::try_from(self.fat.len()).expect("fat16 cluster");
        for index in 0..clusters {
            let mut cluster = vec![0u8; SECTOR];
            let start = index * SECTOR;
            if start < bytes.len() {
                let end = (start + SECTOR).min(bytes.len());
                cluster[..end - start].copy_from_slice(&bytes[start..end]);
            }
            self.data.extend_from_slice(&cluster);
            let next = if index + 1 == clusters {
                0xFFFF
            } else {
                first + u16::try_from(index + 1).expect("fat16 cluster")
            };
            self.fat.push(next);
        }
        first
    }

    fn write_cluster(&mut self, cluster: u16, bytes: &[u8]) {
        let start = (cluster as usize - 2) * SECTOR;
        let n = bytes.len().min(SECTOR);
        self.data[start..start + n].copy_from_slice(&bytes[..n]);
    }

    fn add_dir(&mut self, parent: u16, nodes: &[Node]) -> u16 {
        let cluster = self.alloc(&[]);
        let mut entries = Vec::new();
        entries.extend_from_slice(&sfn_entry(&short_bytes("."), 0x10, cluster, 0));
        entries.extend_from_slice(&sfn_entry(&short_bytes(".."), 0x10, parent, 0));
        for node in nodes {
            entries.extend(self.entries_for(node, cluster));
        }
        assert!(
            entries.len() <= SECTOR,
            "fixture directory does not fit in one cluster"
        );
        self.write_cluster(cluster, &entries);
        cluster
    }

    fn entries_for(&mut self, node: &Node, parent: u16) -> Vec<u8> {
        match &node.kind {
            NodeKind::File(data) => {
                let cluster = if data.is_empty() { 0 } else { self.alloc(data) };
                lfn_and_sfn(
                    &node.name,
                    &node.short_name,
                    0x20,
                    cluster,
                    u32::try_from(data.len()).expect("file size"),
                )
            }
            NodeKind::Dir(children) => {
                let cluster = self.add_dir(parent, children);
                lfn_and_sfn(&node.name, &node.short_name, 0x10, cluster, 0)
            }
        }
    }

    fn pad_to_fat16(&mut self) {
        let have = self.fat.len() - 2;
        assert!(
            have < MIN_FAT16_CLUSTERS,
            "fixture has too many clusters for the small FAT16 layout"
        );
        let add = MIN_FAT16_CLUSTERS - have;
        self.fat.extend(std::iter::repeat_n(0, add));
        self.data.resize(self.data.len() + add * SECTOR, 0);
    }
}

pub fn format_volume(nodes: &[Node], label: &str, hidden_sectors: u32) -> Vec<u8> {
    let mut volume = Volume::new();
    let mut root = Vec::new();
    root.extend_from_slice(&label_entry(label));
    for node in nodes {
        root.extend(volume.entries_for(node, 0));
    }
    let root_bytes = ROOT_ENTRIES * 32;
    assert!(root.len() <= root_bytes, "fixture root directory is full");
    volume.pad_to_fat16();

    let cluster_count = volume.fat.len() - 2;
    let fat_sectors = u16::try_from((volume.fat.len() * 2).div_ceil(SECTOR)).expect("fat sectors");
    let root_sectors = u16::try_from(root_bytes / SECTOR).expect("root sectors");
    let data_start = u32::from(RESERVED_SECTORS)
        + u32::from(fat_sectors) * u32::from(FAT_COUNT)
        + u32::from(root_sectors);
    let total_sectors = data_start + u32::try_from(cluster_count).expect("clusters");
    let mut image = vec![0u8; total_sectors as usize * SECTOR];
    write_boot(
        &mut image[..SECTOR],
        label,
        hidden_sectors,
        fat_sectors,
        u16::try_from(total_sectors).expect("fat16 total sectors fit in u16"),
    );

    let fat_bytes = encode_fat(&volume.fat);
    let fat_region = fat_sectors as usize * SECTOR;
    let fat1 = RESERVED_SECTORS as usize * SECTOR;
    image[fat1..fat1 + fat_bytes.len()].copy_from_slice(&fat_bytes);
    let fat2 = fat1 + fat_region;
    image[fat2..fat2 + fat_bytes.len()].copy_from_slice(&fat_bytes);

    let root_at = fat2 + fat_region;
    image[root_at..root_at + root.len()].copy_from_slice(&root);
    let data_at = data_start as usize * SECTOR;
    image[data_at..data_at + volume.data.len()].copy_from_slice(&volume.data);
    image
}

fn encode_fat(fat: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(fat.len() * 2);
    for entry in fat {
        out.extend_from_slice(&entry.to_le_bytes());
    }
    out
}

fn write_boot(sector: &mut [u8], label: &str, hidden_sectors: u32, fat_sectors: u16, total: u16) {
    sector[0] = 0xEB;
    sector[1] = 0x3C;
    sector[2] = 0x90;
    sector[3..11].copy_from_slice(b"MSDOS5.0");
    sector[11..13].copy_from_slice(&512u16.to_le_bytes());
    sector[13] = 1;
    sector[14..16].copy_from_slice(&RESERVED_SECTORS.to_le_bytes());
    sector[16] = FAT_COUNT;
    sector[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    sector[19..21].copy_from_slice(&total.to_le_bytes());
    sector[21] = 0xF8;
    sector[22..24].copy_from_slice(&fat_sectors.to_le_bytes());
    sector[24..26].copy_from_slice(&32u16.to_le_bytes());
    sector[26..28].copy_from_slice(&64u16.to_le_bytes());
    sector[28..32].copy_from_slice(&hidden_sectors.to_le_bytes());
    sector[36] = 0x80;
    sector[38] = 0x29;
    sector[39..43].copy_from_slice(&0x4357_4631u32.to_le_bytes());
    sector[43..54].copy_from_slice(&label_bytes(label));
    sector[54..62].copy_from_slice(b"FAT16   ");
    sector[510] = 0x55;
    sector[511] = 0xAA;
}

fn label_bytes(label: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    let bytes = label.as_bytes();
    assert!(bytes.len() <= 11, "volume label longer than 11 bytes");
    for (slot, byte) in out.iter_mut().zip(bytes.iter()) {
        *slot = byte.to_ascii_uppercase();
    }
    out
}

fn label_entry(label: &str) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[..11].copy_from_slice(&label_bytes(label));
    entry[11] = 0x08;
    entry
}

fn short_bytes(short_name: &str) -> [u8; 11] {
    if short_name == "." || short_name == ".." {
        let mut out = [b' '; 11];
        out[..short_name.len()].copy_from_slice(short_name.as_bytes());
        return out;
    }
    let (name, ext) = match short_name.split_once('.') {
        Some((name, ext)) => (name, ext),
        None => (short_name, ""),
    };
    assert!(
        name.len() <= 8 && ext.len() <= 3,
        "bad short name {short_name}"
    );
    let mut out = [b' '; 11];
    out[..name.len()].copy_from_slice(name.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    out
}

fn sfn_checksum(name: &[u8; 11]) -> u8 {
    let mut sum = 0u8;
    for &byte in name {
        sum = sum.rotate_right(1).wrapping_add(byte);
    }
    sum
}

fn lfn_and_sfn(name: &str, short_name: &str, attr: u8, cluster: u16, size: u32) -> Vec<u8> {
    let sfn = short_bytes(short_name);
    let checksum = sfn_checksum(&sfn);
    let mut units: Vec<u16> = name.encode_utf16().collect();
    units.push(0);
    while !units.len().is_multiple_of(13) {
        units.push(0xFFFF);
    }
    let chunks: Vec<[u16; 13]> = units
        .chunks(13)
        .map(|chunk| {
            let mut packed = [0u16; 13];
            packed.copy_from_slice(chunk);
            packed
        })
        .collect();
    let mut out = Vec::with_capacity((chunks.len() + 1) * 32);
    for (index, chunk) in chunks.iter().enumerate().rev() {
        let mut seq = u8::try_from(index + 1).expect("lfn sequence");
        if index + 1 == chunks.len() {
            seq |= 0x40;
        }
        out.extend_from_slice(&lfn_entry(seq, checksum, chunk));
    }
    out.extend_from_slice(&sfn_entry(&sfn, attr, cluster, size));
    out
}

fn lfn_entry(seq: u8, checksum: u8, chars: &[u16; 13]) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[0] = seq;
    entry[11] = 0x0F;
    entry[13] = checksum;
    let slots = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    for (index, slot) in slots.iter().enumerate() {
        entry[*slot..*slot + 2].copy_from_slice(&chars[index].to_le_bytes());
    }
    entry
}

fn sfn_entry(name: &[u8; 11], attr: u8, cluster: u16, size: u32) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[..11].copy_from_slice(name);
    entry[11] = attr;
    entry[26..28].copy_from_slice(&cluster.to_le_bytes());
    entry[28..32].copy_from_slice(&size.to_le_bytes());
    entry
}

struct Geometry {
    reserved: u32,
    fat_sectors: u32,
    fat_count: u32,
    root_entries: usize,
    root_sectors: u32,
    data_start: u32,
    cluster_count: u32,
}

fn geometry(volume: &[u8]) -> Result<Geometry, String> {
    if volume.len() < SECTOR || volume[510] != 0x55 || volume[511] != 0xAA {
        return Err("not a FAT boot sector".into());
    }
    if volume[13] != 1 {
        return Err("fixture FAT reader expects 512-byte clusters".into());
    }
    let bytes_per_sector = read_u16(volume, 11)?;
    if bytes_per_sector != 512 {
        return Err("fixture FAT reader expects 512-byte sectors".into());
    }
    let reserved = u32::from(read_u16(volume, 14)?);
    let fat_count = u32::from(volume[16]);
    let root_entries = usize::from(read_u16(volume, 17)?);
    let total16 = u32::from(read_u16(volume, 19)?);
    let fat_sectors = u32::from(read_u16(volume, 22)?);
    let total32 = u32::from_le_bytes(
        volume
            .get(32..36)
            .ok_or("truncated FAT boot sector")?
            .try_into()
            .expect("4 bytes"),
    );
    let total = if total16 == 0 { total32 } else { total16 };
    let root_sectors = u32::try_from((root_entries * 32).div_ceil(SECTOR)).expect("root sectors");
    let data_start = reserved + fat_sectors * fat_count + root_sectors;
    if data_start >= total {
        return Err("FAT data region starts past the end of the volume".into());
    }
    let cluster_count = total - data_start;
    Ok(Geometry {
        reserved,
        fat_sectors,
        fat_count,
        root_entries,
        root_sectors,
        data_start,
        cluster_count,
    })
}

pub fn volume_label(volume: &[u8]) -> Result<String, String> {
    let geo = geometry(volume)?;
    let root = root_bytes(volume, &geo)?;
    for entry in root.chunks(32) {
        if entry.len() < 32 || entry[0] == 0 {
            break;
        }
        if entry[0] == 0xE5 || entry[11] == 0x0F {
            continue;
        }
        if entry[11] & 0x08 != 0 {
            return Ok(trim_label(&entry[..11]));
        }
    }
    Ok(trim_label(&volume[43..54]))
}

fn trim_label(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

pub fn list_files(volume: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let geo = geometry(volume)?;
    let fat = read_fat(volume, &geo)?;
    let root = root_bytes(volume, &geo)?;
    let mut out = Vec::new();
    walk_entries(volume, &geo, &fat, root, "", 0, &mut out)?;
    Ok(out)
}

fn root_bytes<'a>(volume: &'a [u8], geo: &Geometry) -> Result<&'a [u8], String> {
    let start = (geo.reserved + geo.fat_sectors * geo.fat_count) as usize * SECTOR;
    let len = geo.root_entries * 32;
    volume
        .get(start..start + len)
        .ok_or_else(|| String::from("FAT root directory is outside the volume"))
}

fn read_fat(volume: &[u8], geo: &Geometry) -> Result<Vec<u16>, String> {
    let start = geo.reserved as usize * SECTOR;
    let len = geo.fat_sectors as usize * SECTOR;
    let bytes = volume
        .get(start..start + len)
        .ok_or_else(|| String::from("FAT is outside the volume"))?;
    let entries = geo.cluster_count as usize + 2;
    if bytes.len() < entries * 2 {
        return Err("FAT is shorter than the cluster count".into());
    }
    let mut fat = Vec::with_capacity(entries);
    for index in 0..entries {
        fat.push(read_u16(bytes, index * 2)?);
    }
    Ok(fat)
}

fn cluster_bytes<'a>(volume: &'a [u8], geo: &Geometry, cluster: u16) -> Result<&'a [u8], String> {
    if u32::from(cluster) < 2 || u32::from(cluster) >= geo.cluster_count + 2 {
        return Err(format!("FAT cluster {cluster} is out of range"));
    }
    let sector = geo.data_start + u32::from(cluster - 2);
    let start = sector as usize * SECTOR;
    volume
        .get(start..start + SECTOR)
        .ok_or_else(|| format!("FAT cluster {cluster} is outside the volume"))
}

fn read_chain(
    volume: &[u8],
    geo: &Geometry,
    fat: &[u16],
    start: u16,
    max: Option<usize>,
) -> Result<Vec<u8>, String> {
    if start < 2 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut cluster = start;
    let mut guard = 0u32;
    while (2..0xFFF8).contains(&cluster) {
        out.extend_from_slice(cluster_bytes(volume, geo, cluster)?);
        if max.is_some_and(|limit| out.len() >= limit) {
            break;
        }
        cluster = *fat
            .get(cluster as usize)
            .ok_or_else(|| format!("FAT entry {cluster} is missing"))?;
        guard += 1;
        if guard > geo.cluster_count {
            return Err("FAT cluster chain looped".into());
        }
    }
    if let Some(limit) = max {
        out.truncate(limit);
    }
    Ok(out)
}

fn walk_entries(
    volume: &[u8],
    geo: &Geometry,
    fat: &[u16],
    entries: &[u8],
    prefix: &str,
    depth: u32,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    if depth > 8 {
        return Err("FAT directory nesting is deeper than a fixture".into());
    }
    let mut lfn: Vec<(u8, [u16; 13], u8)> = Vec::new();
    for entry in entries.chunks(32) {
        if entry.len() < 32 || entry[0] == 0 {
            break;
        }
        if entry[0] == 0xE5 {
            lfn.clear();
            continue;
        }
        let attr = entry[11];
        if attr == 0x0F {
            lfn.push(decode_lfn(entry)?);
            continue;
        }
        if attr & 0x08 != 0 {
            lfn.clear();
            continue;
        }
        let sfn = entry[..11].try_into().expect("short name is 11 bytes");
        let name = lfn_to_string(&lfn, sfn_checksum(&sfn)).unwrap_or_else(|| short_to_string(&sfn));
        lfn.clear();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let cluster = read_u16(entry, 26)?;
        let size = u32::from_le_bytes(entry[28..32].try_into().expect("file size"));
        let path = format!("{prefix}{name}");
        if attr & 0x10 != 0 {
            let bytes = read_chain(volume, geo, fat, cluster, None)?;
            walk_entries(
                volume,
                geo,
                fat,
                &bytes,
                &format!("{path}/"),
                depth + 1,
                out,
            )?;
        } else {
            let bytes = read_chain(volume, geo, fat, cluster, Some(size as usize))?;
            out.push((path, bytes));
        }
    }
    Ok(())
}

fn decode_lfn(entry: &[u8]) -> Result<(u8, [u16; 13], u8), String> {
    if entry.len() < 32 {
        return Err("truncated LFN entry".into());
    }
    let mut chars = [0u16; 13];
    let slots = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    for (index, slot) in slots.iter().enumerate() {
        chars[index] = read_u16(entry, *slot)?;
    }
    Ok((entry[0], chars, entry[13]))
}

fn lfn_to_string(slots: &[(u8, [u16; 13], u8)], checksum: u8) -> Option<String> {
    if slots.is_empty() || slots.iter().any(|(_, _, sum)| *sum != checksum) {
        return None;
    }
    let count = slots.len();
    let mut ordered = vec![[0u16; 13]; count];
    for (seq, chars, _) in slots {
        let index = usize::from(seq & 0x1F);
        if index == 0 || index > count {
            return None;
        }
        ordered[index - 1] = *chars;
    }
    let mut units = Vec::new();
    for chars in ordered {
        for unit in chars {
            if unit == 0 || unit == 0xFFFF {
                return Some(String::from_utf16_lossy(&units));
            }
            units.push(unit);
        }
    }
    Some(String::from_utf16_lossy(&units))
}

fn short_to_string(name: &[u8; 11]) -> String {
    let base = String::from_utf8_lossy(&name[..8]).trim().to_string();
    let ext = String::from_utf8_lossy(&name[8..]).trim().to_string();
    if ext.is_empty() {
        base
    } else {
        format!("{base}.{ext}")
    }
}
