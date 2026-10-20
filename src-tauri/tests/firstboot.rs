use std::fs;

use cfw_zero_touch_lib::firstboot::{
    boot_letter, card_safety, disarm_easyroms_firstboot, user_roms_are_safe, CardSafety,
};

#[allow(dead_code, unused_imports)]
#[path = "fixtures/mod.rs"]
mod fixtures;

fn fixture() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "cfw-firstboot-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let boot = root.join("boot");
    fs::create_dir_all(boot.join("roms").join("gba")).unwrap();
    fs::write(boot.join("expandtoexfat.sh"), "mkfs.exfat\n").unwrap();
    fs::write(boot.join("firstboot.sh"), "expand\n").unwrap();
    fs::write(boot.join("doneit"), "marker\n").unwrap();
    fs::write(boot.join("roms").join("gba").join("game.gba"), b"keep-me").unwrap();
    boot
}

#[test]
fn doneit_does_not_make_user_roms_safe() {
    let boot = fixture();
    let error = user_roms_are_safe(&boot).unwrap_err();
    assert!(error.to_lowercase().contains("format") || error.to_lowercase().contains("firstboot"));
    let _ = fs::remove_dir_all(boot.parent().unwrap());
}

#[test]
fn disarm_removes_the_wipe_scripts_and_leaves_roms() {
    let boot = fixture();
    let report = disarm_easyroms_firstboot(&boot).unwrap();
    assert!(report.removed.iter().any(|name| name == "expandtoexfat.sh"));
    assert!(report.removed.iter().any(|name| name == "firstboot.sh"));
    assert!(!boot.join("expandtoexfat.sh").exists());
    assert!(!boot.join("firstboot.sh").exists());
    assert!(boot.join("doneit").exists());
    assert_eq!(
        fs::read(boot.join("roms").join("gba").join("game.gba")).unwrap(),
        b"keep-me"
    );
    user_roms_are_safe(&boot).unwrap();
    let _ = fs::remove_dir_all(boot.parent().unwrap());
}

#[test]
fn disarm_refuses_a_boot_folder_without_the_wipe_script() {
    let boot = fixture();
    fs::remove_file(boot.join("expandtoexfat.sh")).unwrap();
    fs::remove_file(boot.join("firstboot.sh")).unwrap();
    let error = disarm_easyroms_firstboot(&boot).unwrap_err();
    assert!(error.to_lowercase().contains("expandtoexfat"));
    assert_eq!(
        fs::read(boot.join("roms").join("gba").join("game.gba")).unwrap(),
        b"keep-me"
    );
    let _ = fs::remove_dir_all(boot.parent().unwrap());
}

fn expanded_card_volumes() -> Vec<(char, String)> {
    vec![('D', "BOOT".into()), ('E', "EASYROMS".into())]
}

#[test]
fn boot_letter_finds_the_boot_volume_case_insensitively() {
    let volumes = vec![('E', "EASYROMS".into()), ('D', "boot".into())];
    assert_eq!(boot_letter(&volumes), Some('D'));
    assert_eq!(boot_letter(&expanded_card_volumes()), Some('D'));
    assert_eq!(
        boot_letter(&vec![('E', "EASYROMS".into())]),
        None,
        "EASYROMS alone is not a BOOT volume"
    );
}

#[test]
fn card_safety_refuses_an_armed_card_even_though_easyroms_is_labeled() {
    let boot = fixture();
    let volumes = expanded_card_volumes();
    let boot_root = boot.clone();
    match card_safety(&volumes, Some(&boot_root)) {
        CardSafety::Armed(reason) => {
            assert!(reason.contains("expandtoexfat.sh"), "{reason}");
        }
        other => panic!("expected Armed, got {other:?}"),
    }
    let _ = fs::remove_dir_all(boot.parent().unwrap());
}

#[test]
fn card_safety_allows_a_card_whose_boot_ran_firstboot() {
    let boot = fixture();
    disarm_easyroms_firstboot(&boot).unwrap();
    let volumes = expanded_card_volumes();
    assert_eq!(card_safety(&volumes, Some(&boot)), CardSafety::Safe);
    let _ = fs::remove_dir_all(boot.parent().unwrap());
}

#[test]
fn armed_fixture_boot_is_not_safe_for_user_roms() {
    let card = fixtures::build_card(fixtures::CardOptions::armed_original());
    let scratch = fixtures::Scratch::new("armed-boot");
    let boot = scratch.path().join("boot");
    fixtures::extract_boot(&card.bytes, &boot).unwrap();
    let error = user_roms_are_safe(&boot).unwrap_err();
    assert!(error.contains("expandtoexfat.sh"), "{error}");
    assert!(boot.join("doneit").is_file());
    let probe = fixtures::probe_image(&card.bytes).unwrap();
    match card_safety(&fixtures::lettered_volumes(&probe), Some(&boot)) {
        CardSafety::Armed(reason) => assert!(reason.contains("expandtoexfat.sh"), "{reason}"),
        other => panic!("expected Armed, got {other:?}"),
    }
    assert_eq!(
        fixtures::read_easyroms(&card.bytes, "gba/game.gba").unwrap(),
        b"keep-me"
    );
}

#[test]
fn disarm_on_an_extracted_fixture_boot_removes_scripts_and_leaves_dtbs() {
    let card = fixtures::build_card(fixtures::CardOptions::armed_clone());
    let scratch = fixtures::Scratch::new("disarm-fixture");
    let boot = scratch.path().join("boot");
    fixtures::extract_boot(&card.bytes, &boot).unwrap();
    let report = disarm_easyroms_firstboot(&boot).unwrap();
    assert!(report.removed.iter().any(|name| name == "expandtoexfat.sh"));
    assert!(report.removed.iter().any(|name| name == "firstboot.sh"));
    user_roms_are_safe(&boot).unwrap();
    assert!(boot.join("rk3326-r36s-type4-linux.dtb").is_file());
    assert!(boot.join("rk3326-r36s-linux.dtb").is_file());
    assert!(!fixtures::boot_file(&card.bytes, "expandtoexfat.sh")
        .unwrap()
        .is_empty());
    assert_eq!(
        fixtures::read_easyroms(&card.bytes, "gba/game.gba").unwrap(),
        b"keep-me"
    );
}

#[test]
fn disarmed_fixture_boot_is_safe_and_refuses_another_disarm() {
    let card = fixtures::build_card(fixtures::CardOptions::disarmed_original());
    let scratch = fixtures::Scratch::new("disarmed-boot");
    let boot = scratch.path().join("boot");
    fixtures::extract_boot(&card.bytes, &boot).unwrap();
    user_roms_are_safe(&boot).unwrap();
    let error = disarm_easyroms_firstboot(&boot).unwrap_err();
    assert!(error.to_lowercase().contains("expandtoexfat"), "{error}");
    let probe = fixtures::probe_image(&card.bytes).unwrap();
    assert_eq!(
        card_safety(&fixtures::lettered_volumes(&probe), Some(&boot)),
        CardSafety::Safe
    );
    assert!(boot.join("gameconsole-r36s.dtb").is_file());
    assert!(boot.join("rk3326-rg351mp-linux.dtb").is_file());
}

#[test]
fn card_safety_is_unknown_without_a_boot_volume_next_to_easyroms() {
    let volumes = vec![('E', "EASYROMS".into())];
    match card_safety(&volumes, None) {
        CardSafety::Unknown(reason) => {
            assert!(reason.to_lowercase().contains("boot"), "{reason}");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}
