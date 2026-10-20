//! Read-only backup of a whole removable card to `.img.gz` plus a SHA-256 sidecar.
//!
//! Gzip is the codec because `flate2` is already in this crate and the flash
//! path already reads `.img.gz`. A new zstd dependency would widen the
//! supply-chain check for a read of the card. The sidecar is the SHA-256 of
//! the uncompressed stored bytes, so the codec does not affect verification.
//!
//! The file is written as `name.img.gz.partial` and renamed to `name.img.gz`
//! only after the gzip trailer is flushed. Cancel leaves the partial name and
//! does not write the sidecar.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};

use crate::block_device::{is_physical_disk_path, FakeBlockDevice};
use crate::format_gate::{displayed_size_matches, drive_is_usable, FormatBlock};
use crate::volume::{decide, DriveKind, VolumeDecision, VolumeInfo};

const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupDisk {
    pub number: u32,
    pub bus: String,
    pub size_bytes: u64,
    pub system_disk: bool,
    pub displayed_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupBlock {
    NotRemovable,
    Size,
}

impl BackupBlock {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotRemovable => "only a removable USB disk can be backed up",
            Self::Size => "refusing a disk whose size is 0 or no longer matches the size on screen",
        }
    }
}

#[derive(Debug)]
pub enum BackupError {
    InsufficientSpace { need: u64, free: u64 },
    Destination(String),
    Read(String),
    Write(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientSpace { need, free } => write!(
                formatter,
                "not enough free space for the backup: need {need} bytes, {free} free"
            ),
            Self::Destination(message) | Self::Read(message) | Self::Write(message) => {
                formatter.write_str(message)
            }
        }
    }
}

#[derive(Debug)]
pub enum BackupOutcome {
    Complete {
        image: PathBuf,
        sidecar: PathBuf,
        sha256: String,
        bytes: u64,
    },
    Cancelled {
        partial: PathBuf,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
    pub bytes_read: u64,
    pub bytes_total: u64,
}

/// Classify a whole disk with the same Win32 removable/fixed split `volume`
/// uses, then hand that volume to the existing gates.
pub fn source_volume(disk: &BackupDisk) -> VolumeInfo {
    let win32_drive_type =
        if disk.system_disk || disk.number == 0 || !disk.bus.eq_ignore_ascii_case("USB") {
            3
        } else {
            2
        };
    VolumeInfo {
        id: format!("disk{}", disk.number),
        letter: String::new(),
        label: String::new(),
        file_system: String::new(),
        total_bytes: disk.size_bytes,
        is_removable: DriveKind::from_win32(win32_drive_type).is_removable(),
        is_empty: true,
    }
}

pub fn authorize_backup(disk: &BackupDisk) -> Result<(), BackupBlock> {
    let volume = source_volume(disk);
    let decision = decide(&volume, "");
    let usable = drive_is_usable(&volume);
    if matches!(decision, VolumeDecision::Rejected { .. })
        || matches!(usable, Err(FormatBlock::NotRemovable))
    {
        return Err(BackupBlock::NotRemovable);
    }
    if matches!(usable, Err(FormatBlock::Size)) {
        return Err(BackupBlock::Size);
    }
    if usable.is_err() {
        return Err(BackupBlock::NotRemovable);
    }
    displayed_size_matches(&volume, disk.displayed_bytes).map_err(|_| BackupBlock::Size)
}

/// Bytes of free space required before a gzip of `source_len` may start.
///
/// A gzip stream can grow by 5 bytes per 32 KiB block plus a 10-byte header
/// and an 8-byte trailer. The extra 64 KiB covers the sidecar and a short write.
pub fn bytes_required(source_len: u64) -> u64 {
    let blocks = source_len.div_ceil(32 * 1024);
    source_len
        .saturating_add(blocks.saturating_mul(5))
        .saturating_add(18)
        .saturating_add(64 * 1024)
}

pub fn partial_path(destination: &Path) -> PathBuf {
    suffixed(destination, ".partial")
}

pub fn sidecar_path(destination: &Path) -> PathBuf {
    suffixed(destination, ".sha256")
}

fn suffixed(destination: &Path, suffix: &str) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    destination.with_file_name(name)
}

pub fn check_destination(destination: &Path) -> Result<(), BackupError> {
    if is_physical_disk_path(destination) {
        return Err(BackupError::Destination(
            "refusing to write a backup onto a physical disk path".into(),
        ));
    }
    let name = destination.file_name().and_then(|name| name.to_str());
    let Some(name) = name else {
        return Err(BackupError::Destination(
            "backup file name is not valid text".into(),
        ));
    };
    if !name.ends_with(".img.gz") || name.ends_with(".img.gz.partial") {
        return Err(BackupError::Destination(
            "backup file must end in .img.gz".into(),
        ));
    }
    let parent = destination_dir(destination);
    if !parent.is_dir() {
        return Err(BackupError::Destination(format!(
            "destination folder {} does not exist",
            parent.display()
        )));
    }
    let canonical = fs::canonicalize(parent).map_err(|error| {
        BackupError::Destination(format!("could not resolve {}: {error}", parent.display()))
    })?;
    if is_physical_disk_path(&canonical) || is_physical_disk_path(&canonical.join(name)) {
        return Err(BackupError::Destination(
            "refusing to write a backup onto a physical disk path".into(),
        ));
    }
    match fs::symlink_metadata(destination) {
        Ok(meta) if meta.file_type().is_symlink() => Err(BackupError::Destination(
            "refusing to follow a symlink at the backup path".into(),
        )),
        Ok(_) => Err(BackupError::Destination(format!(
            "a file already exists at {}",
            destination.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BackupError::Destination(format!(
            "could not inspect {}: {error}",
            destination.display()
        ))),
    }
}

fn destination_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Free bytes available to this user on the filesystem that holds `directory`.
///
/// This stats a directory. It does not open a disk device.
pub fn directory_free_bytes(directory: &Path) -> io::Result<u64> {
    directory_free_bytes_at(directory)
}

#[cfg(unix)]
fn directory_free_bytes_at(directory: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut bytes = directory.as_os_str().as_bytes().to_vec();
    if bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains a nul",
        ));
    }
    bytes.push(0);
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    let rc = unsafe { libc::statvfs(bytes.as_ptr().cast(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    let unit = if stat.f_frsize == 0 {
        stat.f_bsize
    } else {
        stat.f_frsize
    };
    Ok(stat.f_bavail.saturating_mul(unit))
}

#[cfg(windows)]
fn directory_free_bytes_at(directory: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = directory
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free = 0u64;
    let status = unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) };
    if let Err(error) = status {
        return Err(io::Error::other(error.to_string()));
    }
    Ok(free)
}

#[cfg(not(any(unix, windows)))]
fn directory_free_bytes_at(directory: &Path) -> io::Result<u64> {
    let _ = directory;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "free space is not implemented for this OS",
    ))
}

struct ReadOnlyView<'a>(&'a mut dyn FakeBlockDevice);

impl FakeBlockDevice for ReadOnlyView<'_> {
    fn reported_len(&self) -> u64 {
        self.0.reported_len()
    }

    fn stored_len(&self) -> u64 {
        self.0.stored_len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.0.read_at(offset, buf)
    }

    fn write_at(&mut self, _offset: u64, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "backup source is read-only",
        ))
    }
}

/// Read `device` and write a gzip image. The device is only read.
///
/// `write_at` on the view used here returns an error and does not forward
/// the bytes, so a later edit cannot persist a write through this function.
pub fn backup_image(
    device: &mut dyn FakeBlockDevice,
    destination: &Path,
    free_bytes: u64,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(BackupProgress),
) -> Result<BackupOutcome, BackupError> {
    check_destination(destination)?;
    let total = device.stored_len();
    if total == 0 {
        return Err(BackupError::Destination(
            "refusing to back up a source that reports size 0".into(),
        ));
    }
    let need = bytes_required(total);
    if free_bytes < need {
        return Err(BackupError::InsufficientSpace {
            need,
            free: free_bytes,
        });
    }

    let partial = partial_path(destination);
    let sidecar = sidecar_path(destination);
    if is_physical_disk_path(&partial) || is_physical_disk_path(&sidecar) {
        return Err(BackupError::Destination(
            "refusing to write a backup onto a physical disk path".into(),
        ));
    }
    if let Ok(meta) = fs::symlink_metadata(&partial) {
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            return Err(BackupError::Destination(
                "refusing to replace a non-file at the partial backup path".into(),
            ));
        }
    }
    match fs::symlink_metadata(&sidecar) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_file() => {
            return Err(BackupError::Destination(
                "refusing to replace a non-file at the checksum path".into(),
            ));
        }
        Ok(_) => fs::remove_file(&sidecar).map_err(|error| {
            BackupError::Write(format!(
                "could not remove a stale checksum {}: {error}",
                sidecar.display()
            ))
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(BackupError::Destination(format!(
                "could not inspect {}: {error}",
                sidecar.display()
            )));
        }
    }

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(&partial).map_err(|error| {
        BackupError::Write(format!("could not create {}: {error}", partial.display()))
    })?;
    let mut encoder = GzEncoder::new(file, Compression::fast());
    let mut view = ReadOnlyView(device);
    let mut hasher = Sha256::new();
    let mut offset = 0u64;
    let mut buf = vec![0u8; CHUNK];
    while offset < total {
        if cancel.load(Ordering::Acquire) {
            drop(encoder);
            return Ok(BackupOutcome::Cancelled { partial });
        }
        let n = ((total - offset) as usize).min(buf.len());
        view.read_at(offset, &mut buf[..n]).map_err(|error| {
            BackupError::Read(format!(
                "{error}; partial backup left at {}",
                partial.display()
            ))
        })?;
        hasher.update(&buf[..n]);
        encoder.write_all(&buf[..n]).map_err(|error| {
            BackupError::Write(format!(
                "{error}; partial backup left at {}",
                partial.display()
            ))
        })?;
        offset += n as u64;
        on_progress(BackupProgress {
            bytes_read: offset,
            bytes_total: total,
        });
    }
    if cancel.load(Ordering::Acquire) {
        drop(encoder);
        return Ok(BackupOutcome::Cancelled { partial });
    }

    let finished = encoder.finish().map_err(|error| {
        BackupError::Write(format!(
            "could not finish the compressed image: {error}; partial backup left at {}",
            partial.display()
        ))
    })?;
    finished.sync_all().map_err(|error| {
        BackupError::Write(format!("could not flush {}: {error}", partial.display()))
    })?;
    drop(finished);
    fs::rename(&partial, destination).map_err(|error| {
        BackupError::Write(format!(
            "could not publish {}: {error}; partial backup left at {}",
            destination.display(),
            partial.display()
        ))
    })?;
    let sha256 = hex_hash(&hasher.finalize());
    if let Err(error) = fs::write(&sidecar, format!("{sha256}\n")) {
        let _ = fs::rename(destination, &partial);
        return Err(BackupError::Write(format!(
            "could not write the checksum {}: {error}",
            sidecar.display()
        )));
    }
    Ok(BackupOutcome::Complete {
        image: destination.to_path_buf(),
        sidecar,
        sha256,
        bytes: total,
    })
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

/// A file handle that refuses writes even if the operating system opened it
/// writable. The stock-card command opens the handle read-only and then wraps
/// it in this type.
pub struct ReadOnlyFile {
    file: File,
    len: u64,
}

impl ReadOnlyFile {
    pub fn new(file: File, len: u64) -> Self {
        Self { file, len }
    }
}

impl FakeBlockDevice for ReadOnlyFile {
    fn reported_len(&self) -> u64 {
        self.len
    }

    fn stored_len(&self) -> u64 {
        self.len
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "block offset overflow"))?;
        if end > self.len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "read outside the disk size",
            ));
        }
        self.file.seek(io::SeekFrom::Start(offset))?;
        self.file.read_exact(buf)
    }

    fn write_at(&mut self, _offset: u64, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "backup source is read-only",
        ))
    }
}
