use cfw_zero_touch_lib::volume::{allow_copy, decide, DriveKind, VolumeDecision, VolumeInfo};

fn volume(removable: bool, file_system: &str, label: &str, empty: bool) -> VolumeInfo {
    VolumeInfo {
        id: "E:".into(),
        letter: "E".into(),
        label: label.into(),
        file_system: file_system.into(),
        total_bytes: 32 * 1024 * 1024 * 1024,
        is_removable: removable,
        is_empty: empty,
    }
}

#[test]
fn fixed_disk_is_rejected_even_when_empty_and_labeled() {
    let decision = decide(&volume(false, "exFAT", "EASYROMS", true), "EASYROMS");
    assert!(matches!(decision, VolumeDecision::Rejected { .. }));
}

#[test]
fn empty_matching_exfat_card_is_ready_without_relabel() {
    let decision = decide(&volume(true, "exFAT", "EASYROMS", true), "EASYROMS");
    assert_eq!(decision, VolumeDecision::Ready { relabel: false });
}

#[test]
fn empty_fat_card_with_the_wrong_label_is_ready_to_relabel() {
    let decision = decide(&volume(true, "FAT32", "UNTITLED", true), "easyroms");
    assert_eq!(decision, VolumeDecision::Ready { relabel: true });
}

#[test]
fn empty_ntfs_removable_card_needs_a_format() {
    let decision = decide(&volume(true, "NTFS", "", true), "EASYROMS");
    match decision {
        VolumeDecision::NeedsFormat { reason } => assert!(reason.to_lowercase().contains("ntfs")),
        other => panic!("expected format, got {other:?}"),
    }
}

#[test]
fn non_empty_card_with_the_expected_label_is_ready_to_receive_roms() {
    let decision = decide(&volume(true, "exFAT", "EASYROMS", false), "EASYROMS");
    assert_eq!(decision, VolumeDecision::Ready { relabel: false });
}

#[test]
fn non_empty_card_with_a_different_label_needs_an_explicit_format() {
    let decision = decide(&volume(true, "exFAT", "UNTITLED", false), "EASYROMS");
    match decision {
        VolumeDecision::NeedsFormat { reason } => {
            assert!(reason.to_lowercase().contains("not empty"))
        }
        other => panic!("expected format, got {other:?}"),
    }
}

#[test]
fn seeded_exfat_card_can_receive_roms_without_another_format() {
    let seeded = volume(true, "exFAT", "EASYROMS", false);
    allow_copy(&seeded, "EASYROMS").expect("folders from prepare must not block copy");
}

#[test]
fn copy_still_refuses_a_fixed_disk() {
    let error = allow_copy(&volume(false, "exFAT", "EASYROMS", false), "EASYROMS").unwrap_err();
    assert!(error.to_lowercase().contains("removable"));
}

#[test]
fn windows_listing_returns_only_removable_drives() {
    let volumes = cfw_zero_touch_lib::volume::list_volumes().expect("drive list");
    assert!(volumes.iter().all(|volume| {
        volume.is_removable && volume.letter.chars().count() == 1 && volume.id.ends_with(':')
    }));
}

#[test]
fn only_removable_drive_kind_is_accepted() {
    assert!(DriveKind::from_win32(2).is_removable());
    assert!(!DriveKind::from_win32(3).is_removable());
    assert!(!DriveKind::from_win32(0).is_removable());
}
