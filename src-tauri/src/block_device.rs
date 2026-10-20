//! File-backed block devices for probe, backup, and flash tests.
//!
//! `FileBlockDevice` stores bytes in a regular file. When `reported_len` is
//! larger than `stored_len`, offsets wrap modulo the stored length. That is
//! the fake-capacity SD card: the controller advertises a bigger disk than
//! the NAND it actually has, and writes past the real size land on earlier
//! blocks.
//!
//! This module does not change flashing. `flash` still authorizes a plan and
//! hashes a reader; tests adapt a `FakeBlockDevice` with [`DeviceCursor`] so
//! those functions can target an image file. Nothing here opens a physical
//! disk. [`FileBlockDevice::create`] and [`FileBlockDevice::open`] reject
//! `/dev/...` and `\\.\PhysicalDrive...` paths before any open on that path.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

/// Byte-addressed device tests can probe, back up, and flash.
///
/// `reported_len` is the capacity the card claims. `stored_len` is the number
/// of bytes the backing file actually holds.
pub trait FakeBlockDevice {
    fn sector_size(&self) -> u64 {
        512
    }

    fn reported_len(&self) -> u64;

    fn stored_len(&self) -> u64;

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()>;
}

/// True for a path that names a physical disk or another `/dev` node.
///
/// The check is lexical. It does not stat, open, read, or write the path.
pub fn is_physical_disk_path(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    let raw_lower = raw.to_ascii_lowercase();
    if raw_lower.contains("physicaldrive")
        || raw_lower.starts_with(r"\\.\")
        || raw_lower.starts_with(r"\\?\")
        || raw_lower.starts_with("//./")
        || raw_lower.starts_with("//?/")
    {
        return true;
    }
    // Check the path as written. On Windows `/dev/sda` is not absolute, so
    // joining it with the current directory would hide the prefix.
    let slash = raw_lower.replace('\\', "/");
    if slash == "/dev" || slash.starts_with("/dev/") {
        return true;
    }
    dev_root(&lexical_absolute(path))
}

fn dev_root(path: &Path) -> bool {
    let comps: Vec<Component<'_>> = path.components().collect();
    let mut index = 0;
    if matches!(comps.first(), Some(Component::Prefix(_))) {
        index = 1;
    }
    matches!(comps.get(index), Some(Component::RootDir))
        && matches!(
            comps.get(index + 1),
            Some(Component::Normal(name)) if name.eq_ignore_ascii_case("dev")
        )
}

fn lexical_absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn refuse_physical() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "refusing a physical disk path",
    )
}

fn refuse_special() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "refusing a path that is not a regular file",
    )
}

/// Open `path` only after the physical-disk checks.
///
/// A symlink is refused so a fixture path cannot redirect onto another inode.
/// A block or character device that the lexical check missed is refused from
/// `symlink_metadata` before `OpenOptions::open`.
fn open_regular(path: &Path, create: bool) -> io::Result<File> {
    open_regular_access(path, create, true)
}

/// `write` is false for a backup source. Create and the flash-test opener
/// still pass true.
fn open_regular_access(path: &Path, create: bool, write: bool) -> io::Result<File> {
    if is_physical_disk_path(path) {
        return Err(refuse_physical());
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            let kind = meta.file_type();
            if kind.is_symlink() || !kind.is_file() {
                return Err(refuse_special());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {}
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    if write {
        options.write(true);
    }
    if create {
        options.create(true).truncate(true);
    }
    options.open(path)
}

/// Regular file pretending to be a card.
///
/// `reported_len` may be larger than the file. Reads and writes in the
/// reported range wrap onto `[0, stored_len)`.
#[derive(Debug)]
pub struct FileBlockDevice {
    path: PathBuf,
    file: File,
    stored_len: u64,
    reported_len: u64,
}

impl FileBlockDevice {
    /// Create or truncate a regular file of `stored_len` bytes.
    ///
    /// The file is not grown to `reported_len`. A wrap-around card keeps its
    /// real size on disk and only claims the larger capacity.
    pub fn create(path: &Path, stored_len: u64, reported_len: u64) -> io::Result<Self> {
        validate_lengths(stored_len, reported_len)?;
        let file = open_regular(path, true)?;
        file.set_len(stored_len)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            stored_len,
            reported_len,
        })
    }

    /// Open an existing regular file. Reported capacity equals the file length.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = open_regular(path, false)?;
        let stored_len = file.metadata()?.len();
        Self::from_file(path, file, stored_len, stored_len)
    }

    /// Open an existing regular file for reading only.
    ///
    /// Backup uses this. `write_at` fails because the handle was not opened
    /// for write, and a physical-disk path is still refused before the open.
    pub fn open_read_only(path: &Path) -> io::Result<Self> {
        let file = open_regular_access(path, false, false)?;
        let stored_len = file.metadata()?.len();
        Self::from_file(path, file, stored_len, stored_len)
    }

    /// Open an existing file and claim `reported_len`, which may exceed the file.
    pub fn open_with_reported_len(path: &Path, reported_len: u64) -> io::Result<Self> {
        let file = open_regular(path, false)?;
        let stored_len = file.metadata()?.len();
        Self::from_file(path, file, stored_len, reported_len)
    }

    fn from_file(path: &Path, file: File, stored_len: u64, reported_len: u64) -> io::Result<Self> {
        validate_lengths(stored_len, reported_len)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            stored_len,
            reported_len,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn check_range(&self, offset: u64, len: u64) -> io::Result<()> {
        if len == 0 {
            return Ok(());
        }
        let end = offset
            .checked_add(len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "block offset overflow"))?;
        if offset >= self.reported_len || end > self.reported_len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "access outside the reported capacity",
            ));
        }
        Ok(())
    }
}

fn validate_lengths(stored_len: u64, reported_len: u64) -> io::Result<()> {
    if stored_len == 0 || reported_len == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a fixture disk cannot report size 0",
        ));
    }
    if reported_len < stored_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "reported capacity is smaller than the stored image",
        ));
    }
    Ok(())
}

impl FakeBlockDevice for FileBlockDevice {
    fn reported_len(&self) -> u64 {
        self.reported_len
    }

    fn stored_len(&self) -> u64 {
        self.stored_len
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.check_range(offset, buf.len() as u64)?;
        let mut done = 0;
        while done < buf.len() {
            let pos = offset + done as u64;
            let physical = pos % self.stored_len;
            let room = (self.stored_len - physical) as usize;
            let n = room.min(buf.len() - done);
            self.file.seek(SeekFrom::Start(physical))?;
            self.file.read_exact(&mut buf[done..done + n])?;
            done += n;
        }
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        self.check_range(offset, data.len() as u64)?;
        let mut done = 0;
        while done < data.len() {
            let pos = offset + done as u64;
            let physical = pos % self.stored_len;
            let room = (self.stored_len - physical) as usize;
            let n = room.min(data.len() - done);
            self.file.seek(SeekFrom::Start(physical))?;
            self.file.write_all(&data[done..done + n])?;
            done += n;
        }
        self.file.flush()?;
        Ok(())
    }
}

/// Sequential view of a [`FakeBlockDevice`].
///
/// `flash::SectorWriter` and `flash::sha256_prefix` take `Write` and `Read`.
/// This cursor is the adapter those functions use in tests.
pub struct DeviceCursor<D> {
    device: D,
    pos: u64,
}

impl<D> DeviceCursor<D> {
    pub fn new(device: D) -> Self {
        Self { device, pos: 0 }
    }

    pub fn rewind(&mut self) {
        self.pos = 0;
    }

    pub fn into_device(self) -> D {
        self.device
    }
}

impl<D: FakeBlockDevice> Write for DeviceCursor<D> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.device.write_at(self.pos, buf)?;
        self.pos += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<D: FakeBlockDevice> Read for DeviceCursor<D> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let available = self.device.reported_len().saturating_sub(self.pos);
        if available == 0 {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(available) as usize;
        self.device.read_at(self.pos, &mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

/// Copy the stored bytes, not the advertised capacity, into `out`.
pub fn backup_stored(device: &mut dyn FakeBlockDevice, mut out: impl Write) -> io::Result<u64> {
    let total = device.stored_len();
    let mut offset = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    while offset < total {
        let n = ((total - offset) as usize).min(buf.len());
        device.read_at(offset, &mut buf[..n])?;
        out.write_all(&buf[..n])?;
        offset += n as u64;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::flash::{sha256_bytes, sha256_prefix, write_gzip, SectorWriter};

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cfw-block-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn physical_disk_paths_are_rejected_before_any_open() {
        assert!(is_physical_disk_path(Path::new(r"\\.\PhysicalDrive0")));
        assert!(is_physical_disk_path(Path::new(r"\\?\PhysicalDrive1")));
        assert!(is_physical_disk_path(Path::new("/dev/sda")));
        assert!(is_physical_disk_path(Path::new("/dev/nvme0n1")));
        assert!(is_physical_disk_path(Path::new("/dev/mmcblk0")));
        assert!(is_physical_disk_path(Path::new("/dev/loop0")));
        assert!(is_physical_disk_path(Path::new("/dev")));
        let error = FileBlockDevice::create(Path::new("/dev/sda"), 512, 512).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("physical disk"));
        let missing =
            FileBlockDevice::create(Path::new("/dev/cfw-not-a-real-node"), 512, 512).unwrap_err();
        assert!(missing.to_string().contains("physical disk"));
    }

    #[test]
    fn a_temp_image_path_is_not_a_physical_disk() {
        let dir = scratch("path");
        let path = dir.join("card.img");
        assert!(!is_physical_disk_path(&path));
        let mut device = FileBlockDevice::create(&path, 1024, 1024).unwrap();
        device.write_at(0, b"abcd").unwrap();
        let mut got = [0u8; 4];
        device.read_at(0, &mut got).unwrap();
        assert_eq!(&got, b"abcd");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 1024);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_or_shrinking_capacity_is_rejected() {
        let dir = scratch("len");
        let path = dir.join("card.img");
        assert!(FileBlockDevice::create(&path, 0, 512).is_err());
        assert!(FileBlockDevice::create(&path, 512, 0).is_err());
        assert!(FileBlockDevice::create(&path, 1024, 512).is_err());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wrap_writes_past_the_stored_length_land_on_earlier_bytes() {
        let dir = scratch("wrap");
        let path = dir.join("wrap.img");
        let mut device = FileBlockDevice::create(&path, 4096, 8192).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 4096);
        device.write_at(0, &[0x11; 4096]).unwrap();
        device.write_at(4050, &[0x22; 100]).unwrap();
        let mut got = [0u8; 4096];
        device.read_at(0, &mut got).unwrap();
        assert_eq!(&got[..54], &[0x22; 54]);
        assert_eq!(&got[54..4050], &[0x11; 3996]);
        assert_eq!(&got[4050..], &[0x22; 46]);
        let mut at_reported = [0u8; 16];
        device.read_at(4096, &mut at_reported).unwrap();
        assert_eq!(at_reported, [0x22; 16]);
        let error = device.write_at(8192, &[1]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backup_copies_stored_bytes_only() {
        let dir = scratch("backup");
        let path = dir.join("wrap.img");
        let mut device = FileBlockDevice::create(&path, 512, 2048).unwrap();
        let mut payload = vec![0u8; 512];
        payload[..4].copy_from_slice(b"card");
        device.write_at(0, &payload).unwrap();
        let mut out = Vec::new();
        let copied = backup_stored(&mut device, &mut out).unwrap();
        assert_eq!(copied, 512);
        assert_eq!(out, payload);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sector_writer_and_gzip_hash_a_file_backed_device() {
        let dir = scratch("flash");
        let path = dir.join("card.img");
        let payload = b"rgb10x-image-padded";
        let stored = 4096u64;
        let device = FileBlockDevice::create(&path, stored, stored).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(payload).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut cursor = DeviceCursor::new(device);
        let mut writer = SectorWriter::new(&mut cursor);
        let written = write_gzip(Cursor::new(compressed), &mut writer).unwrap();
        writer.flush().unwrap();
        drop(writer);
        assert_eq!(written, payload.len() as u64);
        cursor.rewind();
        let hash = sha256_prefix(&mut cursor, written).unwrap();
        assert_eq!(hash, sha256_bytes(payload));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn directories_and_symlinks_are_refused() {
        let dir = scratch("special");
        let error = FileBlockDevice::open(&dir).unwrap_err();
        assert!(error.to_string().contains("regular file"));
        #[cfg(unix)]
        {
            let file = dir.join("real.img");
            std::fs::write(&file, vec![0u8; 512]).unwrap();
            let link = dir.join("link.img");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            let error = FileBlockDevice::open(&link).unwrap_err();
            assert!(error.to_string().contains("regular file"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
