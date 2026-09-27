use std::io::{Cursor, Write};

use cfw_zero_touch_lib::flash::{
    authorize_flash, ensure_image, extracted_image_name, image_file_name, sha256_bytes, write_gzip,
    FlashBlock, FlashDisk, FlashPlan,
};
use flate2::write::GzEncoder;
use flate2::Compression;

fn disk(number: u32, usb: bool, bytes: u64) -> FlashDisk {
    FlashDisk {
        number,
        bus: if usb { "USB".into() } else { "NVMe".into() },
        size_bytes: bytes,
        system_disk: number == 0,
    }
}

fn plan(confirm: &str, disk: u32, bytes: u64, sha: &str) -> FlashPlan {
    FlashPlan {
        confirmation: confirm.into(),
        disk_number: disk,
        displayed_bytes: bytes,
        expected_sha256: sha.into(),
        actual_sha256: sha.into(),
    }
}

#[test]
fn sha256_of_abc_matches_the_known_vector() {
    let hash = sha256_bytes(b"abc");
    assert_eq!(
        hash,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn gzip_roundtrip_writes_the_original_bytes() {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, b"rgb10x-image").unwrap();
    let compressed = encoder.finish().unwrap();
    let mut output = Vec::new();
    let written = write_gzip(Cursor::new(compressed), &mut output).unwrap();
    assert_eq!(written, output.len() as u64);
    assert_eq!(output, b"rgb10x-image");
}

#[test]
fn flash_requires_the_word_flash() {
    let error = authorize_flash(&disk(1, true, 100), &plan("flash", 1, 100, "abc")).unwrap_err();
    assert_eq!(error, FlashBlock::Confirmation);
}

#[test]
fn flash_refuses_the_system_disk() {
    let error = authorize_flash(&disk(0, false, 100), &plan("FLASH", 0, 100, "abc")).unwrap_err();
    assert_eq!(error, FlashBlock::NotRemovable);
}

#[test]
fn flash_refuses_when_the_shown_size_does_not_match() {
    let error = authorize_flash(&disk(1, true, 100), &plan("FLASH", 1, 50, "abc")).unwrap_err();
    assert_eq!(error, FlashBlock::Size);
}

#[test]
fn flash_refuses_a_checksum_mismatch() {
    let mut request = plan("FLASH", 1, 100, "expected");
    request.actual_sha256 = "different".into();
    let error = authorize_flash(&disk(1, true, 100), &request).unwrap_err();
    assert_eq!(error, FlashBlock::Checksum);
}

#[test]
fn sector_writer_only_emits_full_sectors() {
    struct Guard(Vec<u8>);
    impl std::io::Write for Guard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if !buf.len().is_multiple_of(512) {
                return Err(std::io::Error::other("unaligned"));
            }
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = cfw_zero_touch_lib::flash::SectorWriter::new(Guard(Vec::new()));
    std::io::Write::write_all(&mut writer, &vec![7u8; 600]).unwrap();
    std::io::Write::write_all(&mut writer, &vec![8u8; 424]).unwrap();
    writer.flush().unwrap();
    assert_eq!(writer.into_inner().0.len(), 1024);
}

#[test]
fn image_file_name_uses_the_gz_name() {
    let name = image_file_name(
        "https://github.com/ROCKNIX/distribution/releases/download/20260901/ROCKNIX-RK3326.aarch64-20260901-b.img.gz",
    )
    .unwrap();
    assert_eq!(name, "ROCKNIX-RK3326.aarch64-20260901-b.img.gz");
}

#[test]
fn split_7z_name_and_extracted_image_name() {
    let name = image_file_name(
        "https://github.com/christianhaitian/dArkOS/releases/download/v08272026/dArkOS_RG351MP_trixie_09022026.img.7z.001",
    )
    .unwrap();
    assert_eq!(name, "dArkOS_RG351MP_trixie_09022026.img.7z.001");
    assert_eq!(
        extracted_image_name(&name).unwrap(),
        "dArkOS_RG351MP_trixie_09022026.img"
    );
}

#[test]
fn image_file_name_rejects_a_path_escape() {
    assert!(image_file_name("https://example.com/../secret.img.gz").is_err());
}

#[test]
fn matching_cache_is_reused_without_downloading() {
    let dir = std::env::temp_dir().join(format!("cfw-cache-hit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let name = "ROCKNIX-test.img.gz";
    std::fs::write(dir.join(name), b"abc").unwrap();
    let hash = sha256_bytes(b"abc");
    let mut fetched = false;
    let path = ensure_image(
        std::slice::from_ref(&dir),
        &format!("https://example.com/{name}"),
        &hash,
        |_| {
            fetched = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(!fetched);
    assert_eq!(path, dir.join(name));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_cache_downloads_and_checks_the_hash() {
    let dir = std::env::temp_dir().join(format!("cfw-cache-miss-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let name = "ROCKNIX-test.img.gz";
    let hash = sha256_bytes(b"abc");
    let path = ensure_image(
        std::slice::from_ref(&dir),
        &format!("https://example.com/{name}"),
        &hash,
        |dest| {
            std::fs::write(dest, b"abc").unwrap();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"abc");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrong_cached_hash_is_replaced() {
    let dir = std::env::temp_dir().join(format!("cfw-cache-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let name = "ROCKNIX-test.img.gz";
    std::fs::write(dir.join(name), b"stale").unwrap();
    let hash = sha256_bytes(b"fresh");
    let path = ensure_image(
        std::slice::from_ref(&dir),
        &format!("https://example.com/{name}"),
        &hash,
        |dest| {
            std::fs::write(dest, b"fresh").unwrap();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn authorized_usb_disk_is_allowed() {
    authorize_flash(&disk(1, true, 100), &plan("FLASH", 1, 100, "abc")).unwrap();
}
