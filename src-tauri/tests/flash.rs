use std::io::{Cursor, Write};

use cfw_zero_touch_lib::block_device::{
    backup_stored, DeviceCursor, FakeBlockDevice, FileBlockDevice,
};
use cfw_zero_touch_lib::flash::{
    authorize_flash, ensure_image, extracted_image_name, image_file_name, sha256_bytes,
    sha256_prefix, write_gzip, FlashBlock, FlashDisk, FlashPlan, SectorWriter,
};

#[allow(dead_code, unused_imports)]
#[path = "fixtures/mod.rs"]
mod fixtures;
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

#[test]
fn flash_size_gate_uses_the_reported_size_of_a_fixture_card() {
    let card = fixtures::build_card(fixtures::CardOptions::armed_original());
    let stored = card.stored_len();
    let reported = fixtures::wrap_reported_len(stored);
    authorize_flash(&disk(1, true, reported), &plan("FLASH", 1, reported, "abc")).unwrap();
    let error =
        authorize_flash(&disk(1, true, reported), &plan("FLASH", 1, stored, "abc")).unwrap_err();
    assert_eq!(error, FlashBlock::Size);
}

#[test]
fn gzip_flash_helpers_roundtrip_a_fixture_image_on_a_fake_device() {
    let card = fixtures::build_card(fixtures::CardOptions::disarmed_clone());
    assert_eq!(card.bytes.len() % 512, 0);
    let scratch = fixtures::Scratch::new("flash-fixture");
    let path = scratch.path().join("card.img");
    let stored = card.stored_len();
    let device = FileBlockDevice::create(&path, stored, stored).unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&card.bytes).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut cursor = DeviceCursor::new(device);
    let mut writer = SectorWriter::new(&mut cursor);
    let written = write_gzip(Cursor::new(compressed), &mut writer).unwrap();
    writer.flush().unwrap();
    drop(writer);
    assert_eq!(written, stored);
    cursor.rewind();
    let card_hash = sha256_prefix(&mut cursor, written).unwrap();
    let image_hash = sha256_bytes(&card.bytes);
    assert_eq!(card_hash, image_hash);
    authorize_flash(
        &disk(1, true, stored),
        &FlashPlan {
            confirmation: "FLASH".into(),
            disk_number: 1,
            displayed_bytes: stored,
            expected_sha256: image_hash,
            actual_sha256: card_hash,
        },
    )
    .unwrap();

    let mut flashed = cursor.into_device();
    let probe_buf = {
        let mut buf = vec![0u8; stored as usize];
        flashed.read_at(0, &mut buf).unwrap();
        buf
    };
    let probe = fixtures::probe_image(&probe_buf).unwrap();
    assert!(probe
        .partitions
        .iter()
        .any(|part| part.label.eq_ignore_ascii_case("BOOT") && part.filesystem == "fat16"));
    assert!(probe
        .partitions
        .iter()
        .any(|part| part.label.eq_ignore_ascii_case("EASYROMS") && part.filesystem == "exfat"));
}

#[test]
fn wraparound_fixture_fails_flash_readback() {
    let card = fixtures::build_wrap_card();
    let stored = card.stored_len();
    let reported = card.reported_len;
    assert!(reported > stored);
    let scratch = fixtures::Scratch::new("wrap-flash");
    let path = scratch.path().join("wrap-card.img");
    let mut device = FileBlockDevice::create(&path, stored, reported).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), stored);
    device.write_at(0, &card.bytes).unwrap();
    let mut backup = Vec::new();
    assert_eq!(backup_stored(&mut device, &mut backup).unwrap(), stored);
    assert_eq!(backup, card.bytes);

    let mut payload = card.bytes.clone();
    payload.extend(std::iter::repeat_n(0xA5, 512));
    assert!((payload.len() as u64) <= reported);
    device.write_at(0, &payload).unwrap();
    let mut readback = vec![0u8; payload.len()];
    device.read_at(0, &mut readback).unwrap();
    assert_ne!(readback, payload);
    assert_eq!(
        &readback[..512],
        &payload[stored as usize..stored as usize + 512]
    );

    let image_hash = sha256_bytes(&payload);
    let mut cursor = DeviceCursor::new(device);
    let card_hash = sha256_prefix(&mut cursor, payload.len() as u64).unwrap();
    assert_ne!(card_hash, image_hash);
    // authorize_flash compares the reported size with the plan. A wrap card
    // passes that gate; the read-back hash above is what catches it.
    authorize_flash(
        &disk(1, true, reported),
        &plan("FLASH", 1, reported, &image_hash),
    )
    .unwrap();
}
