//! Stock-card device and screen detection. Fixtures only: directory file
//! sets and layout descriptors. Nothing here opens a physical disk.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cfw_zero_touch_lib::detect::{
    self, identify, parse_layout_descriptor, parse_partition_snapshot, read_boot_dtbs, Detection,
    Observation, ObservedDtb, ObservedPartition, ReadOnlyFile,
};
use cfw_zero_touch_lib::profiles::{self, Profile};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stock")
}

fn shipped() -> Vec<Profile> {
    profiles::load_profiles(
        &repo().join("profiles"),
        &repo().join("specs").join("profile.schema.json"),
    )
    .expect("shipped profiles")
}

fn observe(dir: &Path, layout: &str) -> Observation {
    Observation {
        dtbs: read_boot_dtbs(dir).expect("boot files"),
        partitions: parse_layout_descriptor(layout).expect("layout"),
        unreadable: false,
    }
}

fn expect_match(detection: Detection, profile_id: &str, device: &str, screen: &str) {
    match detection {
        Detection::Match {
            profile_id: found,
            device: found_device,
            screen: found_screen,
        } => {
            assert_eq!(found, profile_id);
            assert_eq!(found_device, device);
            assert_eq!(found_screen, screen);
        }
        other => panic!("expected {profile_id} / {screen}, got {other:?}"),
    }
}

#[test]
fn fixture_boot_sets_map_to_the_shipped_device_and_screen() {
    let profiles = shipped();
    let root = fixtures();

    expect_match(
        identify(
            &profiles,
            &[observe(
                &root.join("r36s-clone-v12"),
                &fs::read_to_string(root.join("r36s-clone-v12/layout.json")).unwrap(),
            )],
        ),
        "r36s-clone-card",
        "R36S clone",
        "V12 panel",
    );
    expect_match(
        identify(
            &profiles,
            &[observe(
                &root.join("r36s-clone-type4"),
                &fs::read_to_string(root.join("r36s-clone-type4/layout.json")).unwrap(),
            )],
        ),
        "r36s-clone-card",
        "R36S clone",
        "Type 4 panel",
    );
    expect_match(
        identify(
            &profiles,
            &[observe(
                &root.join("r35s-stock"),
                &fs::read_to_string(root.join("r35s-stock/layout.json")).unwrap(),
            )],
        ),
        "r35s-stock-card",
        "R35S",
        "Stock panel",
    );
}

#[test]
fn published_clone_hash_matches_any_positive_boot_size() {
    let profiles = shipped();
    let detection = identify(
        &profiles,
        &[Observation {
            dtbs: vec![ObservedDtb {
                name: "rk3326-evb-lp3-v12-linux.dtb".into(),
                sha256: "8c4b6b738bfd5af9638db9c6e9a3ac49884ca4b773c4053a31d1a934b69246b0".into(),
            }],
            partitions: vec![
                ObservedPartition {
                    type_code: "0c".into(),
                    label: "BOOT".into(),
                    size_bytes: 268_435_456,
                },
                ObservedPartition {
                    type_code: "0C".into(),
                    label: "roms".into(),
                    size_bytes: 31_000_000_000,
                },
            ],
            unreadable: false,
        }],
    );
    expect_match(detection, "r36s-clone-card", "R36S clone", "V12 panel");
}

#[test]
fn unknown_ambiguous_and_wrong_size_fall_back_to_the_manual_picker() {
    let profiles = shipped();
    let root = fixtures();
    let message = "We couldn't tell which handheld this is. Please pick it from the list.";
    assert_eq!(detect::MANUAL_DEVICE_MESSAGE, message);

    let unknown = identify(
        &profiles,
        &[observe(
            &root.join("unknown"),
            &fs::read_to_string(root.join("unknown/layout.json")).unwrap(),
        )],
    );
    assert_eq!(unknown, Detection::Manual);

    let ambiguous = identify(
        &profiles,
        &[observe(
            &root.join("ambiguous"),
            &fs::read_to_string(root.join("ambiguous/layout.json")).unwrap(),
        )],
    );
    assert_eq!(
        ambiguous,
        Detection::Manual,
        "two screens on one card must not be guessed"
    );

    let wrong_size = identify(
        &profiles,
        &[Observation {
            dtbs: read_boot_dtbs(&root.join("r36s-clone-v12")).unwrap(),
            partitions: vec![
                ObservedPartition {
                    type_code: "0c".into(),
                    label: "BOOT".into(),
                    size_bytes: 32 * 1024 * 1024,
                },
                ObservedPartition {
                    type_code: "0c".into(),
                    label: "ROMS".into(),
                    size_bytes: 1024 * 1024 * 1024,
                },
            ],
            unreadable: false,
        }],
    );
    assert_eq!(
        wrong_size,
        Detection::Manual,
        "a fixed boot size that differs is not a match"
    );

    assert_eq!(identify(&profiles, &[]), Detection::None);
}

#[test]
fn overlapping_profiles_do_not_guess() {
    let mut clone = shipped()
        .into_iter()
        .find(|profile| profile.id == "r36s-clone-card")
        .unwrap();
    let mut other = clone.clone();
    other.id = "other-card".into();
    clone
        .stock_fingerprint
        .as_mut()
        .unwrap()
        .variants
        .truncate(1);
    other
        .stock_fingerprint
        .as_mut()
        .unwrap()
        .variants
        .truncate(1);
    let root = fixtures();
    let detection = identify(
        &[clone, other],
        &[observe(
            &root.join("r36s-clone-v12"),
            &fs::read_to_string(root.join("r36s-clone-v12/layout.json")).unwrap(),
        )],
    );
    assert_eq!(detection, Detection::Manual);
}

#[test]
fn snapshot_refuses_system_boot_disk0_and_non_removable_and_reads_boot() {
    let script = detect::partition_query_script();
    let lower = script.to_ascii_lowercase();
    assert!(script.contains("IsSystem"));
    assert!(script.contains("IsBoot"));
    assert!(script.contains("USB"));
    assert!(script.contains("SD"));
    assert!(script.contains("Number -ne 0"));
    assert!(script.contains("Size -gt 0"));
    for forbidden in [
        "set-disk",
        "clear-disk",
        "format-volume",
        "new-partition",
        "cfw-flash",
        "runas",
    ] {
        assert!(!lower.contains(forbidden), "{forbidden} in {script}");
    }

    let root = fixtures();
    let json = r#"[
          {"DiskNumber":0,"BusType":"USB","IsSystem":false,"IsBoot":false,"DiskSize":100,"PartitionNumber":1,"MbrType":12,"Size":67108864,"DriveLetter":"C","Label":"BOOT"},
          {"DiskNumber":1,"BusType":"SATA","IsSystem":false,"IsBoot":false,"DiskSize":100,"PartitionNumber":1,"MbrType":12,"Size":67108864,"DriveLetter":"D","Label":"BOOT"},
          {"DiskNumber":2,"BusType":"USB","IsSystem":true,"IsBoot":false,"DiskSize":100,"PartitionNumber":1,"MbrType":12,"Size":67108864,"DriveLetter":"E","Label":"BOOT"},
          {"DiskNumber":3,"BusType":"SD","IsSystem":false,"IsBoot":true,"DiskSize":100,"PartitionNumber":1,"MbrType":12,"Size":67108864,"DriveLetter":"F","Label":"BOOT"},
          {"DiskNumber":4,"BusType":"USB","IsSystem":false,"IsBoot":false,"DiskSize":0,"PartitionNumber":1,"MbrType":12,"Size":67108864,"DriveLetter":"G","Label":"BOOT"},
          {"DiskNumber":5,"BusType":"SD","IsSystem":false,"IsBoot":false,"DiskSize":64000000000,"PartitionNumber":1,"MbrType":12,"Size":67108864,"DriveLetter":"H","Label":"BOOT"},
          {"DiskNumber":5,"BusType":"SD","IsSystem":false,"IsBoot":false,"DiskSize":64000000000,"PartitionNumber":2,"MbrType":12,"Size":1073741824,"DriveLetter":"I","Label":"ROMS"}
        ]"#;
    let observations = parse_partition_snapshot(json, |letter| {
        assert_eq!(
            letter, 'H',
            "only the USB/SD data disk's BOOT letter is read"
        );
        read_boot_dtbs(&root.join("r36s-clone-v12"))
    })
    .unwrap();
    assert_eq!(observations.len(), 1);
    expect_match(
        identify(&shipped(), &observations),
        "r36s-clone-card",
        "R36S clone",
        "V12 panel",
    );
}

#[test]
fn detection_reader_has_no_write_capability() {
    let root = fixtures().join("r36s-clone-v12");
    let dtb = root.join("rk3326-evb-lp3-v12-linux.dtb");
    let before = fs::read(&dtb).unwrap();
    let mut opened = ReadOnlyFile::open(&dtb).unwrap();
    assert_eq!(opened.read_to_end().unwrap(), before);
    let error = opened.try_write(b"overwrite").unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        error.to_string().contains("read-only"),
        "detection write error should say read-only: {error}"
    );
    assert_eq!(
        fs::read(&dtb).unwrap(),
        before,
        "detection must not modify the boot file"
    );

    for path in [
        r"\\.\PhysicalDrive1",
        r"\\.\PhysicalDrive0",
        "/dev/sda",
        "/dev/mmcblk0",
    ] {
        let error = ReadOnlyFile::open(Path::new(path)).unwrap_err();
        assert_eq!(
            error.kind(),
            io::ErrorKind::PermissionDenied,
            "{path}: {error}"
        );
    }

    #[cfg(unix)]
    {
        let link_dir = std::env::temp_dir().join(format!("cfw-detect-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&link_dir);
        fs::create_dir_all(&link_dir).unwrap();
        let real = link_dir.join("real.dtb");
        fs::write(&real, b"synthetic").unwrap();
        let link = link_dir.join("link.dtb");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(
            ReadOnlyFile::open(&link).is_err(),
            "symlinks are not followed"
        );
        let _ = fs::remove_dir_all(&link_dir);
    }

    let src = include_str!("../src/detect.rs");
    let lower = src.to_ascii_lowercase();
    assert!(
        !src.contains("cfw-flash"),
        "detection must not launch the elevated writer"
    );
    assert!(!src.contains(".write(true)"));
    assert!(!lower.contains("runas"));
    assert!(!src.contains("PhysicalDrive"));
    for forbidden in ["set-disk", "clear-disk", "format-volume", "prepare_volume"] {
        assert!(!lower.contains(forbidden), "{forbidden}");
    }

    let lib = include_str!("../src/lib.rs");
    let start = lib.find("fn detect_stock_device").expect("detect command");
    let rest = &lib[start..];
    let end = rest[1..]
        .find("\nfn ")
        .or_else(|| rest[1..].find("\npub fn "))
        .map(|index| index + 1)
        .unwrap_or(rest.len());
    let body = &rest[..end];
    let body_lower = body.to_ascii_lowercase();
    assert!(!body.contains("flash_tool"));
    assert!(!body.contains("cfw-flash"));
    assert!(!body.contains(".write(true)"));
    assert!(!body.contains("prepare_volume"));
    assert!(!body.contains("open_stock_disk"));
    assert!(!body_lower.contains("runas"));
}
