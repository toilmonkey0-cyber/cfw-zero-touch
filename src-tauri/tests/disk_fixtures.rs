use std::path::PathBuf;
use std::process::Command;

use cfw_zero_touch_lib::block_device::{is_physical_disk_path, FakeBlockDevice, FileBlockDevice};

#[allow(dead_code, unused_imports)]
#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::{
    boot_file, build_card, build_wrap_card, classify_dtb, probe_image, read_easyroms, CardOptions,
    DtbSet, BOOT_PARTITION_TYPE, EASYROMS_PARTITION_TYPE, ROOTFS_PARTITION_TYPE,
};

#[test]
fn fat16_roundtrip_reads_a_nested_file_and_the_boot_label() {
    let volume = fixtures::fat16::format_volume(
        &[fixtures::fat16::dir(
            "roms",
            "ROMS",
            vec![fixtures::fat16::file("note.txt", "NOTE.TXT", b"boot-note")],
        )],
        "BOOT",
        64,
    );
    assert_eq!(fixtures::fat16::volume_label(&volume).unwrap(), "BOOT");
    let files = fixtures::fat16::list_files(&volume).unwrap();
    let note = files
        .iter()
        .find(|(path, _)| path == "roms/note.txt")
        .map(|(_, bytes)| bytes.as_slice());
    assert_eq!(note, Some(b"boot-note".as_slice()));
}

#[test]
fn exfat_roundtrip_reads_easyroms_and_checks_the_boot_region() {
    let volume = fixtures::exfat::format_volume(
        &[fixtures::exfat::dir(
            "gba",
            vec![fixtures::exfat::file("game.gba", b"keep-me".as_slice())],
        )],
        "EASYROMS",
        128,
    );
    fixtures::exfat::boot_region_ok(&volume).unwrap();
    assert_eq!(fixtures::exfat::volume_label(&volume).unwrap(), "EASYROMS");
    let files = fixtures::exfat::list_files(&volume).unwrap();
    let game = files
        .iter()
        .find(|(path, _)| path == "gba/game.gba")
        .map(|(_, bytes)| bytes.as_slice());
    assert_eq!(game, Some(b"keep-me".as_slice()));
}

#[test]
fn generated_cards_have_boot_rootfs_and_easyroms() {
    let samples = [
        (CardOptions::armed_original(), DtbSet::Original, true),
        (CardOptions::armed_clone(), DtbSet::Clone, true),
        (CardOptions::disarmed_original(), DtbSet::Original, false),
        (CardOptions::disarmed_clone(), DtbSet::Clone, false),
    ];
    for (options, set, armed) in samples {
        let card = build_card(options);
        assert_eq!(card.bytes, build_card(options).bytes);
        let probe = probe_image(&card.bytes).unwrap();
        assert_eq!(probe.partitions.len(), 3, "{probe:?}");
        assert_eq!(probe.partitions[0].number, 1);
        assert_eq!(probe.partitions[0].type_code, BOOT_PARTITION_TYPE);
        assert_eq!(probe.partitions[0].filesystem, "fat16");
        assert_eq!(probe.partitions[0].label, "BOOT");
        assert_eq!(probe.partitions[1].number, 2);
        assert_eq!(probe.partitions[1].type_code, ROOTFS_PARTITION_TYPE);
        assert_eq!(probe.partitions[1].filesystem, "unknown");
        assert_eq!(probe.partitions[2].number, 3);
        assert_eq!(probe.partitions[2].type_code, EASYROMS_PARTITION_TYPE);
        assert_eq!(probe.partitions[2].filesystem, "exfat");
        assert_eq!(probe.partitions[2].label, "EASYROMS");
        let names = fixtures::fat16::list_files(partition(&card.bytes, 1))
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert_eq!(classify_dtb(names.iter().map(String::as_str)).unwrap(), set);
        let has_script = names.iter().any(|name| name == "expandtoexfat.sh");
        assert_eq!(has_script, armed);
        assert_eq!(
            read_easyroms(&card.bytes, "gba/game.gba").unwrap(),
            b"keep-me"
        );
        if armed {
            assert!(boot_file(&card.bytes, "doneit")
                .unwrap()
                .starts_with(b"marker"));
        } else {
            assert!(boot_file(&card.bytes, "expandtoexfat.sh").is_err());
        }
    }
}

#[test]
fn wrap_card_reports_more_than_it_stores_and_still_probes() {
    let card = build_wrap_card();
    assert!(card.reported_len > card.stored_len());
    let scratch = fixtures::Scratch::new("wrap-probe");
    let path = scratch.path().join("wrap-card.img");
    let mut device = FileBlockDevice::create(&path, card.stored_len(), card.reported_len).unwrap();
    device.write_at(0, &card.bytes).unwrap();
    let mut stored = vec![0u8; card.stored_len() as usize];
    device.read_at(0, &mut stored).unwrap();
    let probe = probe_image(&stored).unwrap();
    assert_eq!(probe.partitions[0].label, "BOOT");
    assert_eq!(probe.partitions[2].label, "EASYROMS");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), card.stored_len());
}

#[test]
fn build_script_refuses_a_physical_disk_path() {
    let Some(python) = python_bin() else {
        return;
    };
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/build-images.py");
    for argument in ["/dev/sda", r"\\.\PhysicalDrive0", "/dev/mmcblk0"] {
        let output = Command::new(python)
            .arg(&script)
            .arg(argument)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{argument}: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.to_lowercase().contains("physical"),
            "{argument}: {stderr}"
        );
    }
    assert!(is_physical_disk_path(std::path::Path::new("/dev/sda")));
}

fn partition(image: &[u8], number: u32) -> &[u8] {
    let probe = probe_image(image).unwrap();
    let part = probe
        .partitions
        .iter()
        .find(|part| part.number == number)
        .unwrap();
    let start = part.start_lba as usize * 512;
    let end = start + part.sector_count as usize * 512;
    &image[start..end]
}

fn python_bin() -> Option<&'static str> {
    ["python3", "python"]
        .into_iter()
        .find(|bin| Command::new(bin).arg("--version").output().is_ok())
}

#[test]
#[ignore = "writes images to CFW_FIXTURE_OUT; run by tests/fixtures/build-images.py"]
fn write_fixture_images() {
    let out = PathBuf::from(std::env::var("CFW_FIXTURE_OUT").expect("CFW_FIXTURE_OUT"));
    assert!(
        !is_physical_disk_path(&out),
        "refusing to write fixtures under a physical disk path"
    );
    std::fs::create_dir_all(&out).unwrap();
    let cards = [
        ("boot-armed-original.img", CardOptions::armed_original()),
        ("boot-armed-clone.img", CardOptions::armed_clone()),
        (
            "boot-disarmed-original.img",
            CardOptions::disarmed_original(),
        ),
        ("boot-disarmed-clone.img", CardOptions::disarmed_clone()),
    ];
    for (name, options) in cards {
        write_image(&out.join(name), &build_card(options).bytes, None);
    }
    let wrap = build_wrap_card();
    write_image(
        &out.join("wrap-card.img"),
        &wrap.bytes,
        Some(wrap.reported_len),
    );
    std::fs::write(
        out.join("wrap-card.txt"),
        format!(
            "stored_bytes={}\nreported_bytes={}\nwrites past stored_bytes wrap onto earlier blocks\n",
            wrap.stored_len(),
            wrap.reported_len
        ),
    )
    .unwrap();
}

fn write_image(path: &std::path::Path, bytes: &[u8], reported: Option<u64>) {
    assert!(!is_physical_disk_path(path));
    let stored = bytes.len() as u64;
    let reported = reported.unwrap_or(stored);
    let mut device = FileBlockDevice::create(path, stored, reported).unwrap();
    device.write_at(0, bytes).unwrap();
}
