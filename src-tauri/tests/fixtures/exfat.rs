//! Minimal exFAT volume builder and reader.
//!
//! The on-disk fields follow the Microsoft exFAT specification (boot checksum,
//! directory entries, FAT). The volume label and cluster size are the ones
//! `expandtoexfat.sh` asks `mkfs.exfat` for (`-L EASYROMS`); the fixture uses
//! 512-byte clusters so the image stays small. A real card uses 16K clusters
//! (`-c 16K` in the southoz script, `-s 16K` in ArkOS4Clone).

use super::read_u16;

const SECTOR: usize = 512;

#[derive(Debug)]
pub struct Node {
    name: String,
    kind: NodeKind,
}

#[derive(Debug)]
enum NodeKind {
    File(Vec<u8>),
    Dir(Vec<Node>),
}

pub fn file(name: &str, bytes: impl Into<Vec<u8>>) -> Node {
    Node {
        name: name.to_string(),
        kind: NodeKind::File(bytes.into()),
    }
}

pub fn dir(name: &str, children: Vec<Node>) -> Node {
    Node {
        name: name.to_string(),
        kind: NodeKind::Dir(children),
    }
}

struct Heap {
    fat: Vec<u32>,
    clusters: Vec<[u8; SECTOR]>,
}

impl Heap {
    fn new() -> Self {
        Self {
            fat: vec![0xFFFF_FFF8, 0xFFFF_FFFF],
            clusters: Vec::new(),
        }
    }

    fn alloc(&mut self, bytes: &[u8]) -> (u32, u64) {
        let clusters = bytes.len().div_ceil(SECTOR).max(1);
        let first = u32::try_from(self.fat.len()).expect("exfat cluster");
        for index in 0..clusters {
            let mut cluster = [0u8; SECTOR];
            let start = index * SECTOR;
            if start < bytes.len() {
                let end = (start + SECTOR).min(bytes.len());
                cluster[..end - start].copy_from_slice(&bytes[start..end]);
            }
            self.clusters.push(cluster);
            let next = if index + 1 == clusters {
                0xFFFF_FFFF
            } else {
                first + u32::try_from(index + 1).expect("exfat cluster")
            };
            self.fat.push(next);
        }
        (first, bytes.len() as u64)
    }

    fn overwrite(&mut self, cluster: u32, bytes: &[u8]) {
        let start = cluster as usize - 2;
        for (index, chunk) in bytes.chunks(SECTOR).enumerate() {
            self.clusters[start + index][..chunk.len()].copy_from_slice(chunk);
        }
    }
}

struct Placed {
    cluster: u32,
    data_length: u64,
}

pub fn format_volume(nodes: &[Node], label: &str, partition_offset: u64) -> Vec<u8> {
    let upcase = upcase_table();
    let upcase_checksum = table_checksum(&upcase);
    let mut heap = Heap::new();
    let (bitmap_cluster, _) = heap.alloc(&[]);
    let (upcase_cluster, upcase_len) = heap.alloc(&upcase);
    let placed = place_tree(&mut heap, nodes);
    let root_bytes = root_directory(
        label,
        bitmap_cluster,
        upcase_cluster,
        upcase_len,
        upcase_checksum,
        &placed,
    );
    let (root_cluster, _) = heap.alloc(&root_bytes);
    let bitmap_len = heap.clusters.len().div_ceil(8);
    let mut bitmap = vec![0u8; bitmap_len];
    for index in 0..heap.clusters.len() {
        bitmap[index / 8] |= 1 << (index % 8);
    }
    heap.overwrite(bitmap_cluster, &bitmap);

    let cluster_count = u32::try_from(heap.clusters.len()).expect("cluster count");
    let fat_entries = cluster_count + 2;
    let fat_sectors = u32::try_from((fat_entries as usize * 4).div_ceil(SECTOR)).expect("fat");
    let heap_offset = 24 + fat_sectors;
    let volume_sectors = heap_offset + cluster_count;
    let mut image = vec![0u8; volume_sectors as usize * SECTOR];
    write_boot(
        &mut image,
        partition_offset,
        volume_sectors.into(),
        fat_sectors,
        heap_offset,
        cluster_count,
        root_cluster,
    );
    let fat_at = 24 * SECTOR;
    for (index, entry) in heap.fat.iter().enumerate() {
        let at = fat_at + index * 4;
        image[at..at + 4].copy_from_slice(&entry.to_le_bytes());
    }
    let data_at = heap_offset as usize * SECTOR;
    for (index, cluster) in heap.clusters.iter().enumerate() {
        let at = data_at + index * SECTOR;
        image[at..at + SECTOR].copy_from_slice(cluster);
    }
    image
}

fn place_tree(heap: &mut Heap, nodes: &[Node]) -> Vec<(Node, Placed)> {
    let mut placed = Vec::new();
    for node in nodes {
        let found = match &node.kind {
            NodeKind::File(data) => {
                let (cluster, data_length) = heap.alloc(data);
                Placed {
                    cluster,
                    data_length,
                }
            }
            NodeKind::Dir(children) => {
                let nested = place_tree(heap, children);
                let bytes = directory_bytes(&nested);
                let (cluster, data_length) = heap.alloc(&bytes);
                Placed {
                    cluster,
                    data_length,
                }
            }
        };
        placed.push((
            Node {
                name: node.name.clone(),
                kind: match &node.kind {
                    NodeKind::File(data) => NodeKind::File(data.clone()),
                    NodeKind::Dir(_) => NodeKind::Dir(Vec::new()),
                },
            },
            found,
        ));
    }
    placed
}

fn directory_bytes(entries: &[(Node, Placed)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (node, placed) in entries {
        let attr = match &node.kind {
            NodeKind::Dir(_) => 0x10,
            NodeKind::File(_) => 0x20,
        };
        push_set(
            &mut out,
            &node.name,
            attr,
            placed.cluster,
            placed.data_length,
        );
    }
    out
}

fn root_directory(
    label: &str,
    bitmap_cluster: u32,
    upcase_cluster: u32,
    upcase_len: u64,
    upcase_checksum: u32,
    entries: &[(Node, Placed)],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&bitmap_entry(bitmap_cluster));
    out.extend_from_slice(&upcase_entry(upcase_cluster, upcase_len, upcase_checksum));
    out.extend_from_slice(&label_entry(label));
    out.extend(directory_bytes(entries));
    out
}

fn bitmap_entry(cluster: u32) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[0] = 0x81;
    entry[20..24].copy_from_slice(&cluster.to_le_bytes());
    // DataLength is filled after the heap size is known. The reader does not
    // need it to walk the root; the checksum test reads the upcase table.
    entry[24..32].copy_from_slice(&512u64.to_le_bytes());
    entry
}

fn upcase_entry(cluster: u32, data_length: u64, checksum: u32) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[0] = 0x82;
    entry[4..8].copy_from_slice(&checksum.to_le_bytes());
    entry[20..24].copy_from_slice(&cluster.to_le_bytes());
    entry[24..32].copy_from_slice(&data_length.to_le_bytes());
    entry
}

fn label_entry(label: &str) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[0] = 0x83;
    let units: Vec<u16> = label.encode_utf16().collect();
    assert!(units.len() <= 11, "exFAT label longer than 11 characters");
    entry[1] = u8::try_from(units.len()).expect("label length");
    for (index, unit) in units.iter().enumerate() {
        let at = 2 + index * 2;
        entry[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
    entry
}

fn push_set(out: &mut Vec<u8>, name: &str, attr: u16, cluster: u32, data_length: u64) {
    let units: Vec<u16> = name.encode_utf16().collect();
    let name_entries = units.len().div_ceil(15).max(1);
    let secondary = 1 + name_entries;
    let mut set = vec![0u8; (1 + secondary) * 32];
    set[0] = 0x85;
    set[1] = u8::try_from(secondary).expect("secondary count");
    set[4..6].copy_from_slice(&attr.to_le_bytes());
    let stamp = timestamp();
    set[8..12].copy_from_slice(&stamp.to_le_bytes());
    set[12..16].copy_from_slice(&stamp.to_le_bytes());
    set[16..20].copy_from_slice(&stamp.to_le_bytes());
    set[32] = 0xC0;
    set[33] = 0x03;
    set[35] = u8::try_from(units.len()).expect("name length");
    set[36..38].copy_from_slice(&name_hash(&units).to_le_bytes());
    set[40..48].copy_from_slice(&data_length.to_le_bytes());
    set[52..56].copy_from_slice(&cluster.to_le_bytes());
    set[56..64].copy_from_slice(&data_length.to_le_bytes());
    for (index, chunk) in units.chunks(15).enumerate() {
        let at = (2 + index) * 32;
        set[at] = 0xC1;
        for (slot, unit) in chunk.iter().enumerate() {
            let pos = at + 2 + slot * 2;
            set[pos..pos + 2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    let checksum = entry_set_checksum(&set);
    set[2..4].copy_from_slice(&checksum.to_le_bytes());
    out.extend_from_slice(&set);
}

fn timestamp() -> u32 {
    // 2026-01-01 00:00:00, exFAT's 32-bit local timestamp.
    (46 << 25) | (1 << 21) | (1 << 16)
}

fn name_hash(units: &[u16]) -> u16 {
    let mut hash = 0u16;
    for unit in units {
        let up = upcase_unit(*unit);
        hash = hash.rotate_right(1).wrapping_add(up & 0xFF);
        hash = hash.rotate_right(1).wrapping_add(up >> 8);
    }
    hash
}

fn upcase_unit(unit: u16) -> u16 {
    if (0x61..=0x7A).contains(&unit) {
        unit - 0x20
    } else {
        unit
    }
}

fn entry_set_checksum(bytes: &[u8]) -> u16 {
    let mut checksum = 0u16;
    for (index, &byte) in bytes.iter().enumerate() {
        if index == 2 || index == 3 {
            continue;
        }
        checksum = checksum.rotate_right(1).wrapping_add(u16::from(byte));
    }
    checksum
}

fn table_checksum(bytes: &[u8]) -> u32 {
    let mut checksum = 0u32;
    for &byte in bytes {
        checksum = checksum.rotate_right(1).wrapping_add(u32::from(byte));
    }
    checksum
}

fn upcase_table() -> Vec<u8> {
    let mut table = Vec::with_capacity(65536 * 2);
    for unit in 0u32..65536 {
        let mapped = if (0x61..=0x7A).contains(&unit) {
            unit - 0x20
        } else {
            unit
        };
        table.extend_from_slice(&(mapped as u16).to_le_bytes());
    }
    table
}

fn write_boot(
    image: &mut [u8],
    partition_offset: u64,
    volume_length: u64,
    fat_length: u32,
    heap_offset: u32,
    cluster_count: u32,
    root_cluster: u32,
) {
    let mut sector = [0u8; SECTOR];
    sector[0] = 0xEB;
    sector[1] = 0x76;
    sector[2] = 0x90;
    sector[3..11].copy_from_slice(b"EXFAT   ");
    sector[64..72].copy_from_slice(&partition_offset.to_le_bytes());
    sector[72..80].copy_from_slice(&volume_length.to_le_bytes());
    sector[80..84].copy_from_slice(&24u32.to_le_bytes());
    sector[84..88].copy_from_slice(&fat_length.to_le_bytes());
    sector[88..92].copy_from_slice(&heap_offset.to_le_bytes());
    sector[92..96].copy_from_slice(&cluster_count.to_le_bytes());
    sector[96..100].copy_from_slice(&root_cluster.to_le_bytes());
    sector[100..104].copy_from_slice(&0x4357_4632u32.to_le_bytes());
    sector[104..106].copy_from_slice(&0x0100u16.to_le_bytes());
    sector[108] = 9;
    sector[109] = 0;
    sector[110] = 1;
    sector[111] = 0x80;
    sector[112] = 100;
    sector[510] = 0x55;
    sector[511] = 0xAA;
    image[..SECTOR].copy_from_slice(&sector);
    for index in 1..9 {
        let at = index * SECTOR;
        image[at + 510] = 0x55;
        image[at + 511] = 0xAA;
    }
    let checksum = boot_checksum(&image[..11 * SECTOR]);
    let checksum_bytes = checksum.to_le_bytes();
    for slot in 0..(SECTOR / 4) {
        let at = 11 * SECTOR + slot * 4;
        image[at..at + 4].copy_from_slice(&checksum_bytes);
    }
    let main = image[..12 * SECTOR].to_vec();
    image[12 * SECTOR..24 * SECTOR].copy_from_slice(&main);
}

fn boot_checksum(bytes: &[u8]) -> u32 {
    let mut checksum = 0u32;
    for (index, &byte) in bytes.iter().enumerate() {
        if index == 106 || index == 107 || index == 112 {
            continue;
        }
        checksum = checksum.rotate_right(1).wrapping_add(u32::from(byte));
    }
    checksum
}

struct Boot {
    fat_offset: u32,
    heap_offset: u32,
    cluster_count: u32,
    root: u32,
}

fn boot_of(volume: &[u8]) -> Result<Boot, String> {
    if volume.len() < SECTOR || &volume[3..11] != b"EXFAT   " {
        return Err("not an exFAT boot sector".into());
    }
    if volume[510] != 0x55 || volume[511] != 0xAA {
        return Err("exFAT boot signature is missing".into());
    }
    if volume[108] != 9 || volume[109] != 0 {
        return Err("fixture exFAT reader expects 512-byte clusters".into());
    }
    Ok(Boot {
        fat_offset: read_u32(volume, 80)?,
        heap_offset: read_u32(volume, 88)?,
        cluster_count: read_u32(volume, 92)?,
        root: read_u32(volume, 96)?,
    })
}

fn read_u32(buf: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = buf
        .get(offset..offset + 4)
        .ok_or_else(|| format!("need 4 bytes at {offset}"))?;
    Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
}

pub fn boot_region_ok(volume: &[u8]) -> Result<(), String> {
    if volume.len() < 24 * SECTOR {
        return Err("exFAT boot regions are truncated".into());
    }
    let checksum = boot_checksum(&volume[..11 * SECTOR]);
    let stored = read_u32(volume, 11 * SECTOR)?;
    if stored != checksum {
        return Err(format!(
            "exFAT boot checksum is {stored:#x}, computed {checksum:#x}"
        ));
    }
    if volume[..12 * SECTOR] != volume[12 * SECTOR..24 * SECTOR] {
        return Err("exFAT backup boot region does not match the main region".into());
    }
    Ok(())
}

pub fn volume_label(volume: &[u8]) -> Result<String, String> {
    let boot = boot_of(volume)?;
    let entries = read_chain(volume, &boot, boot.root)?;
    for entry in entries.chunks(32) {
        if entry.len() < 32 || entry[0] == 0 {
            break;
        }
        if entry[0] == 0x83 {
            let count = usize::from(entry[1]).min(11);
            let mut units = Vec::with_capacity(count);
            for index in 0..count {
                units.push(read_u16(entry, 2 + index * 2)?);
            }
            return Ok(String::from_utf16_lossy(&units));
        }
    }
    Err("exFAT volume label is missing".into())
}

pub fn list_files(volume: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let boot = boot_of(volume)?;
    let mut out = Vec::new();
    walk(volume, &boot, boot.root, "", 0, &mut out)?;
    Ok(out)
}

fn walk(
    volume: &[u8],
    boot: &Boot,
    cluster: u32,
    prefix: &str,
    depth: u32,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    if depth > 8 {
        return Err("exFAT directory nesting is deeper than a fixture".into());
    }
    let entries = read_chain(volume, boot, cluster)?;
    let mut index = 0;
    while index + 32 <= entries.len() {
        let entry_type = entries[index];
        if entry_type == 0 {
            break;
        }
        if entry_type != 0x85 {
            index += 32;
            continue;
        }
        let secondary = usize::from(entries[index + 1]);
        let set_len = (1 + secondary) * 32;
        if index + set_len > entries.len() {
            return Err("exFAT directory entry set is truncated".into());
        }
        let set = &entries[index..index + set_len];
        index += set_len;
        let parsed = parse_set(set)?;
        let path = format!("{prefix}{}", parsed.name);
        if parsed.attr & 0x10 != 0 {
            walk(
                volume,
                boot,
                parsed.cluster,
                &format!("{path}/"),
                depth + 1,
                out,
            )?;
        } else {
            let bytes = read_bytes(volume, boot, parsed.cluster, parsed.valid_length)?;
            out.push((path, bytes));
        }
    }
    Ok(())
}

struct ParsedSet {
    name: String,
    attr: u16,
    cluster: u32,
    valid_length: u64,
}

fn parse_set(set: &[u8]) -> Result<ParsedSet, String> {
    if set.len() < 64 || set[0] != 0x85 || set[32] != 0xC0 {
        return Err("exFAT file set is missing its stream entry".into());
    }
    let attr = read_u16(set, 4)?;
    let name_len = usize::from(set[35]);
    let cluster = read_u32(set, 52)?;
    let valid_length = u64::from_le_bytes(set[40..48].try_into().expect("valid length"));
    let mut units = Vec::with_capacity(name_len);
    let mut cursor = 64;
    while units.len() < name_len {
        if cursor + 32 > set.len() || set[cursor] != 0xC1 {
            return Err("exFAT file name is truncated".into());
        }
        for slot in 0..15 {
            if units.len() == name_len {
                break;
            }
            units.push(read_u16(&set[cursor..cursor + 32], 2 + slot * 2)?);
        }
        cursor += 32;
    }
    Ok(ParsedSet {
        name: String::from_utf16_lossy(&units),
        attr,
        cluster,
        valid_length,
    })
}

fn read_bytes(volume: &[u8], boot: &Boot, cluster: u32, len: u64) -> Result<Vec<u8>, String> {
    let chained = read_chain(volume, boot, cluster)?;
    let len = usize::try_from(len).unwrap_or(chained.len());
    if chained.len() < len {
        return Err("exFAT file is shorter than its valid length".into());
    }
    Ok(chained[..len].to_vec())
}

fn read_chain(volume: &[u8], boot: &Boot, start: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut cluster = start;
    let mut guard = 0u32;
    while (2..0xFFFF_FFF8).contains(&cluster) {
        if cluster >= boot.cluster_count + 2 {
            return Err(format!("exFAT cluster {cluster} is out of range"));
        }
        let at = (boot.heap_offset as usize + (cluster as usize - 2)) * SECTOR;
        let bytes = volume
            .get(at..at + SECTOR)
            .ok_or_else(|| format!("exFAT cluster {cluster} is outside the volume"))?;
        out.extend_from_slice(bytes);
        let fat_at = boot.fat_offset as usize * SECTOR + cluster as usize * 4;
        cluster = read_u32(volume, fat_at)?;
        guard += 1;
        if guard > boot.cluster_count {
            return Err("exFAT cluster chain looped".into());
        }
    }
    Ok(out)
}
