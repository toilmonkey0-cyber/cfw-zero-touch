use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashDisk {
    pub number: u32,
    pub bus: String,
    pub size_bytes: u64,
    pub system_disk: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashPlan {
    pub confirmation: String,
    pub disk_number: u32,
    pub displayed_bytes: u64,
    pub expected_sha256: String,
    pub actual_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashBlock {
    Confirmation,
    NotRemovable,
    Size,
    Identity,
    Checksum,
}

pub fn image_file_name(url: &str) -> Result<String, String> {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    if without_query.split(['/', '\\']).any(|part| part == "..") {
        return Err(format!("image URL {url} does not name a .img.gz file"));
    }
    let name = without_query
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();
    let safe = !name.is_empty()
        && is_image_file_name(name)
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        && !name.contains("..");
    if safe {
        Ok(name.to_string())
    } else {
        Err(format!("image URL {url} does not name an OS image"))
    }
}

fn is_image_file_name(name: &str) -> bool {
    name.ends_with(".img.gz")
        || name.ends_with(".img")
        || name.ends_with(".7z")
        || name.rsplit_once(".7z.").is_some_and(|(_, index)| {
            !index.is_empty() && index.chars().all(|ch| ch.is_ascii_digit())
        })
}

pub fn extracted_image_name(part_name: &str) -> Result<String, String> {
    let without_index = part_name.strip_suffix(".001").unwrap_or(part_name);
    let image = without_index.strip_suffix(".7z").unwrap_or(without_index);
    if image.ends_with(".img") && image != part_name {
        Ok(image.to_string())
    } else {
        Err(format!("{part_name} is not a split .img.7z archive"))
    }
}

pub fn ensure_image(
    directories: &[PathBuf],
    url: &str,
    expected_sha256: &str,
    mut fetch: impl FnMut(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let file_name = image_file_name(url)?;
    let destination_dir = directories
        .first()
        .ok_or("no image cache directory is configured")?;
    for directory in directories {
        let cached = directory.join(&file_name);
        if cached.is_file() && file_matches(&cached, expected_sha256)? {
            return Ok(cached);
        }
    }
    fs::create_dir_all(destination_dir)
        .map_err(|error| format!("could not create the image cache: {error}"))?;
    let partial = destination_dir.join(format!("{file_name}.partial"));
    fetch(&partial)?;
    if !file_matches(&partial, expected_sha256)? {
        let _ = fs::remove_file(&partial);
        return Err("downloaded image did not match the published checksum".into());
    }
    let final_path = destination_dir.join(&file_name);
    if final_path.exists() {
        let _ = fs::remove_file(&final_path);
    }
    fs::rename(&partial, &final_path)
        .map_err(|error| format!("could not save the image: {error}"))?;
    Ok(final_path)
}

fn file_matches(path: &Path, expected_sha256: &str) -> Result<bool, String> {
    let file =
        File::open(path).map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let actual = sha256_reader(file)?;
    Ok(actual.eq_ignore_ascii_case(expected_sha256))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex_hash(&Sha256::digest(bytes))
}

pub fn sha256_reader(mut reader: impl Read) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("could not read while hashing: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_hash(&hasher.finalize()))
}

pub fn sha256_prefix(mut reader: impl Read, length: u64) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut remaining = length;
    let mut buffer = vec![0u8; 1024 * 1024];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len() as u64) as usize;
        let read = reader
            .read(&mut buffer[..chunk])
            .map_err(|error| format!("could not read the written image: {error}"))?;
        if read == 0 {
            return Err("short read while verifying the written image".into());
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(hex_hash(&hasher.finalize()))
}

pub fn write_gzip(compressed: impl Read, mut output: impl Write) -> Result<u64, String> {
    let mut decoder = GzDecoder::new(compressed);
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut written = 0u64;
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|error| format!("could not decompress the image: {error}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("could not write the image: {error}"))?;
        written += read as u64;
    }
    output
        .flush()
        .map_err(|error| format!("could not flush the image: {error}"))?;
    Ok(written)
}

pub struct SectorWriter<W> {
    inner: W,
    pending: Vec<u8>,
    sector: usize,
}

impl<W: Write> SectorWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            sector: 512,
        }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for SectorWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buf);
        let ready = self.pending.len() - (self.pending.len() % self.sector);
        if ready > 0 {
            self.inner.write_all(&self.pending[..ready])?;
            self.pending.drain(..ready);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.pending.is_empty() {
            let pad = (self.sector - (self.pending.len() % self.sector)) % self.sector;
            self.pending.extend(std::iter::repeat_n(0, pad));
            self.inner.write_all(&self.pending)?;
            self.pending.clear();
        }
        self.inner.flush()
    }
}

pub fn authorize_flash(disk: &FlashDisk, plan: &FlashPlan) -> Result<(), FlashBlock> {
    if plan.confirmation != "FLASH" {
        return Err(FlashBlock::Confirmation);
    }
    if disk.system_disk || disk.number == 0 || !disk.bus.eq_ignore_ascii_case("USB") {
        return Err(FlashBlock::NotRemovable);
    }
    if disk.number != plan.disk_number {
        return Err(FlashBlock::Identity);
    }
    if disk.size_bytes != plan.displayed_bytes {
        return Err(FlashBlock::Size);
    }
    if !plan
        .actual_sha256
        .eq_ignore_ascii_case(&plan.expected_sha256)
    {
        return Err(FlashBlock::Checksum);
    }
    Ok(())
}

fn hex_hash(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}
