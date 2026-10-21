//! Stock-card device and screen detection. Fixtures only: directory file
//! sets and layout descriptors. Nothing here opens a physical disk.
//!
//! Placeholder hashes live in the fixture tree and in test-only profiles.
//! Shipped profiles are loaded only to prove those hashes do not match them.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cfw_zero_touch_lib::detect::{
    self, identify, parse_layout_descriptor, parse_partition_snapshot, read_boot_dtbs, Detection,
    Observation, ObservedDtb, ObservedPartition, ReadOnlyFile, MAX_DTB_BYTES, MAX_DTB_FILES,
};
use cfw_zero_touch_lib::profiles::{self, Profile};

const CITED_HASH: &str = "8c4b6b738bfd5af9638db9c6e9a3ac49884ca4b773c4053a31d1a934b69246b0";
const BOARD: &str = "R36S clone (board G80CA-MB V1.3)";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stock")
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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

fn file_hash(path: &Path) -> String {
    detect::sha256_hex(&fs::read(path).unwrap())
}

fn test_profile(
    id: &str,
    device: &str,
    screen: &str,
    name: &str,
    hash: &str,
    partitions: &str,
) -> Profile {
    let screen_field = if screen.is_empty() {
        String::new()
    } else {
        format!(r#","screen":"{screen}""#)
    };
    let partitions_field = if partitions.is_empty() {
        String::new()
    } else {
        format!(r#","partitions":{partitions}"#)
    };
    let json = format!(
        r#"{{
          "id":"{id}",
          "name":"{id}",
          "cfw":"test",
          "storageModes":["roms_card_only"],
          "romSchema":{{"layout":"arkos_easyroms_root"}},
          "stockFingerprint":{{
            "manualOnly":false,
            "source":"dumped from stock card lab-{id}",
            "variants":[{{
              "device":"{device}"{screen_field},
              "dtbs":[{{"name":"{name}","sha256":"{hash}"}}]
              {partitions_field}
            }}]
          }}
        }}"#
    );
    serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}: {json}"))
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
        other => panic!("expected {profile_id} / {device} / {screen}, got {other:?}"),
    }
}

#[test]
fn placeholder_fixtures_do_not_match_shipped_profiles() {
    let profiles = shipped();
    let root = fixtures();
    for name in [
        "r36s-clone-v12",
        "r36s-clone-type4",
        "r35s-stock",
        "ambiguous",
        "unknown",
    ] {
        let detection = identify(
            &profiles,
            &[observe(
                &root.join(name),
                &fs::read_to_string(root.join(name).join("layout.json")).unwrap(),
            )],
        );
        assert_eq!(
            detection,
            Detection::Manual,
            "{name} is a fixture and must not select a shipped profile"
        );
    }
}

#[test]
fn test_only_feeds_match_placeholder_fixtures() {
    let root = fixtures();
    let v12_hash = file_hash(&root.join("r36s-clone-v12/rk3326-evb-lp3-v12-linux.dtb"));
    let type4_hash = file_hash(&root.join("r36s-clone-type4/rk3326-r36s-type4-linux.dtb"));
    let r35s_hash = file_hash(&root.join("r35s-stock/rk3326-r35s-linux.dtb"));
    let layout = r#"[{"type":"0c","label":"BOOT","sizeBytes":67108864},{"type":"0c","label":"ROMS","sizeBytes":1073741824}]"#;
    let profiles = vec![
        test_profile(
            "lab-v12",
            "Lab clone",
            "V12 panel",
            "rk3326-evb-lp3-v12-linux.dtb",
            &v12_hash,
            "",
        ),
        test_profile(
            "lab-type4",
            "Lab clone",
            "Type 4 panel",
            "rk3326-r36s-type4-linux.dtb",
            &type4_hash,
            "",
        ),
        test_profile(
            "lab-r35s",
            "Lab R35S",
            "Stock panel",
            "rk3326-r35s-linux.dtb",
            &r35s_hash,
            "",
        ),
    ];

    expect_match(
        identify(
            &profiles,
            &[observe(
                &root.join("r36s-clone-v12"),
                &fs::read_to_string(root.join("r36s-clone-v12/layout.json")).unwrap(),
            )],
        ),
        "lab-v12",
        "Lab clone",
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
        "lab-type4",
        "Lab clone",
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
        "lab-r35s",
        "Lab R35S",
        "Stock panel",
    );

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
        "two known DTBs must not be guessed"
    );

    let recorded = test_profile(
        "lab-sized",
        "Lab clone",
        "V12 panel",
        "rk3326-evb-lp3-v12-linux.dtb",
        &v12_hash,
        layout,
    );
    expect_match(
        identify(
            std::slice::from_ref(&recorded),
            &[observe(
                &root.join("r36s-clone-v12"),
                &fs::read_to_string(root.join("r36s-clone-v12/layout.json")).unwrap(),
            )],
        ),
        "lab-sized",
        "Lab clone",
        "V12 panel",
    );
    let wrong_size = identify(
        &[recorded],
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
        "a recorded size that differs is not a match"
    );
}

#[test]
fn cited_board_hash_matches_without_a_recorded_layout() {
    let profiles = shipped();
    let dtb = ObservedDtb {
        name: "rk3326-evb-lp3-v12-linux.dtb".into(),
        sha256: CITED_HASH.into(),
    };
    let stock_shape = vec![
        ObservedPartition {
            type_code: "0c".into(),
            label: "BOOT".into(),
            size_bytes: 64 * 1024 * 1024,
        },
        ObservedPartition {
            type_code: "83".into(),
            label: "rootfs".into(),
            size_bytes: 2 * 1024 * 1024 * 1024,
        },
        ObservedPartition {
            type_code: "0c".into(),
            label: "EASYROMS".into(),
            size_bytes: 32 * 1024 * 1024 * 1024,
        },
    ];
    expect_match(
        identify(
            &profiles,
            &[Observation {
                dtbs: vec![dtb.clone()],
                partitions: stock_shape,
                unreadable: false,
            }],
        ),
        "r36s-clone-card",
        BOARD,
        "",
    );
    expect_match(
        identify(
            &profiles,
            &[Observation {
                dtbs: vec![dtb.clone()],
                partitions: Vec::new(),
                unreadable: false,
            }],
        ),
        "r36s-clone-card",
        BOARD,
        "",
    );
    let view = detect::view(
        &profiles,
        identify(
            &profiles,
            &[Observation {
                dtbs: vec![dtb],
                partitions: Vec::new(),
                unreadable: false,
            }],
        ),
    );
    assert_eq!(view.device, BOARD);
    assert_eq!(view.screen, "");
    assert_eq!(
        view.profile_name,
        "R36S clone — games card (internal firmware)"
    );

    let wrong_hash = identify(
        &profiles,
        &[Observation {
            dtbs: vec![ObservedDtb {
                name: "rk3326-evb-lp3-v12-linux.dtb".into(),
                sha256: "a".repeat(64),
            }],
            partitions: Vec::new(),
            unreadable: false,
        }],
    );
    assert_eq!(wrong_hash, Detection::Manual);
    assert_eq!(identify(&profiles, &[]), Detection::None);
    assert_eq!(
        detect::MANUAL_DEVICE_MESSAGE,
        "We couldn't tell which handheld this is. Please pick it from the list."
    );
}

#[test]
fn shipped_audit_rejects_fixture_hashes() {
    let profiles = shipped();
    detect::audit_stock_fingerprints(&profiles, &fixture_root()).expect("shipped fingerprints");

    let r36s = profiles
        .iter()
        .find(|profile| profile.id == "r36s-clone-card")
        .unwrap();
    let fingerprint = r36s.stock_fingerprint.as_ref().unwrap();
    assert!(!fingerprint.manual_only);
    assert_eq!(
        fingerprint.source.as_deref(),
        Some("https://dtbvault.com/en/file/rk3326-evb-lp3-v12-linux-IYZchb-P")
    );
    assert_eq!(fingerprint.variants.len(), 1);
    assert_eq!(fingerprint.variants[0].device, BOARD);
    assert!(fingerprint.variants[0].screen.is_empty());
    assert!(fingerprint.variants[0].partitions.is_empty());
    assert_eq!(fingerprint.variants[0].dtbs[0].sha256, CITED_HASH);

    let r35s = profiles
        .iter()
        .find(|profile| profile.id == "r35s-stock-card")
        .unwrap();
    assert!(r35s.stock_fingerprint.as_ref().unwrap().manual_only);
    assert!(r35s.stock_fingerprint.as_ref().unwrap().variants.is_empty());

    let fixture_hash = file_hash(&fixtures().join("r36s-clone-v12/rk3326-evb-lp3-v12-linux.dtb"));
    let mut poisoned = profiles.clone();
    poisoned
        .iter_mut()
        .find(|profile| profile.id == "r36s-clone-card")
        .unwrap()
        .stock_fingerprint
        .as_mut()
        .unwrap()
        .variants[0]
        .dtbs[0]
        .sha256 = fixture_hash;
    let error = detect::audit_stock_fingerprints(&poisoned, &fixture_root()).unwrap_err();
    assert!(
        error.contains("sha256 of a file"),
        "fixture hash must fail the audit: {error}"
    );

    let mut uncited = profiles.clone();
    uncited
        .iter_mut()
        .find(|profile| profile.id == "r36s-clone-card")
        .unwrap()
        .stock_fingerprint
        .as_mut()
        .unwrap()
        .source = Some("synthetic placeholder".into());
    let error = detect::audit_stock_fingerprints(&uncited, &fixture_root()).unwrap_err();
    assert!(error.contains("source"), "{error}");
}

#[test]
fn overlapping_profiles_do_not_guess() {
    let root = fixtures();
    let hash = file_hash(&root.join("r36s-clone-v12/rk3326-evb-lp3-v12-linux.dtb"));
    let left = test_profile(
        "left-card",
        "Left",
        "V12 panel",
        "rk3326-evb-lp3-v12-linux.dtb",
        &hash,
        "",
    );
    let right = test_profile(
        "right-card",
        "Right",
        "V12 panel",
        "rk3326-evb-lp3-v12-linux.dtb",
        &hash,
        "",
    );
    let detection = identify(
        &[left, right],
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
    assert_eq!(
        identify(&shipped(), &observations),
        Detection::Manual,
        "a placeholder boot file must not select a shipped profile"
    );
    let hash = observations[0].dtbs[0].sha256.clone();
    let name = observations[0].dtbs[0].name.clone();
    let lab = test_profile("lab-v12", "Lab clone", "V12 panel", &name, &hash, "");
    expect_match(
        identify(&[lab], &observations),
        "lab-v12",
        "Lab clone",
        "V12 panel",
    );
}

#[test]
fn read_boot_dtbs_skips_files_over_one_mib_and_caps_the_count() {
    let dir = std::env::temp_dir().join(format!("cfw-detect-cap-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for index in 0..=MAX_DTB_FILES {
        let mut file = fs::File::create(dir.join(format!("{index:02}.dtb"))).unwrap();
        file.write_all(&[index as u8]).unwrap();
    }
    let huge = dir.join("huge.dtb");
    let mut file = fs::File::create(&huge).unwrap();
    file.write_all(&vec![0u8; (MAX_DTB_BYTES as usize) + 1])
        .unwrap();
    drop(file);

    let dtbs = read_boot_dtbs(&dir).unwrap();
    assert_eq!(dtbs.len(), MAX_DTB_FILES);
    assert!(dtbs.iter().all(|dtb| dtb.name != "huge.dtb"));
    assert!(dtbs
        .iter()
        .all(|dtb| dtb.name != format!("{MAX_DTB_FILES:02}.dtb")));
    assert_eq!(fs::metadata(&huge).unwrap().len(), MAX_DTB_BYTES + 1);

    let only_huge = std::env::temp_dir().join(format!("cfw-detect-huge-{}", std::process::id()));
    let _ = fs::remove_dir_all(&only_huge);
    fs::create_dir_all(&only_huge).unwrap();
    fs::copy(&huge, only_huge.join("huge.dtb")).unwrap();
    assert!(read_boot_dtbs(&only_huge).unwrap().is_empty());

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&only_huge);
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
