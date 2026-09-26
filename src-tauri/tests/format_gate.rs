use cfw_zero_touch_lib::format_gate::{
    authorize_format, format_script, FormatBlock, FormatRequest,
};
use cfw_zero_touch_lib::volume::VolumeInfo;

fn card() -> VolumeInfo {
    VolumeInfo {
        id: "E:".into(),
        letter: "E".into(),
        label: "UNTITLED".into(),
        file_system: "NTFS".into(),
        total_bytes: 32_000_000_000,
        is_removable: true,
        is_empty: false,
    }
}

fn request(confirmation: &str, bytes: u64) -> FormatRequest {
    FormatRequest {
        volume_id: "E:".into(),
        letter: "E".into(),
        displayed_bytes: bytes,
        confirmation: confirmation.into(),
        file_system: "exFAT".into(),
        label: "EASYROMS".into(),
    }
}

#[test]
fn format_requires_the_exact_word() {
    let error = authorize_format(&card(), &request("format", 32_000_000_000)).unwrap_err();
    assert_eq!(error, FormatBlock::Confirmation);
}

#[test]
fn format_refuses_a_fixed_disk() {
    let mut fixed = card();
    fixed.is_removable = false;
    let error = authorize_format(&fixed, &request("FORMAT", 32_000_000_000)).unwrap_err();
    assert_eq!(error, FormatBlock::NotRemovable);
}

#[test]
fn format_refuses_when_the_shown_size_no_longer_matches() {
    let error = authorize_format(&card(), &request("FORMAT", 1)).unwrap_err();
    assert_eq!(error, FormatBlock::Size);
}

#[test]
fn format_refuses_when_the_drive_identity_changed() {
    let mut moved = request("FORMAT", 32_000_000_000);
    moved.volume_id = "F:".into();
    let error = authorize_format(&card(), &moved).unwrap_err();
    assert_eq!(error, FormatBlock::Identity);
}

#[test]
fn authorized_format_script_targets_the_letter_and_rejects_label_injection() {
    authorize_format(&card(), &request("FORMAT", 32_000_000_000)).unwrap();
    let script = format_script("E", "exFAT", "EASYROMS").unwrap();
    assert!(script.contains("-DriveLetter E"));
    assert!(script.contains("EASYROMS"));
    assert!(script.contains("-Confirm:$false"));

    let injected = format_script("E", "exFAT", "EASY'; Remove-Item C:\\*");
    assert!(injected.is_err());
}
