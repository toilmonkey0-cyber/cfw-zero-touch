use cfw_zero_touch_lib::prepare::{prepare_volume, PrepareOutcome, ScriptCall, ShellLog};
use cfw_zero_touch_lib::volume::VolumeInfo;

fn card(empty: bool, label: &str, fs: &str) -> VolumeInfo {
    VolumeInfo {
        id: "E:".into(),
        letter: "E".into(),
        label: label.into(),
        file_system: fs.into(),
        total_bytes: 8_000_000_000,
        is_removable: true,
        is_empty: empty,
    }
}

#[test]
fn ready_card_does_not_touch_the_disk() {
    let mut shell = ShellLog::default();
    let outcome = prepare_volume(
        &card(true, "EASYROMS", "exFAT"),
        "EASYROMS",
        "",
        8_000_000_000,
        "exFAT",
        &mut shell,
    )
    .unwrap();
    assert_eq!(outcome, PrepareOutcome::AlreadyReady);
    assert!(shell.calls.is_empty());
}

#[test]
fn wrong_label_relabels_without_formatting() {
    let mut shell = ShellLog::default();
    let outcome = prepare_volume(
        &card(true, "UNTITLED", "FAT32"),
        "EASYROMS",
        "",
        8_000_000_000,
        "exFAT",
        &mut shell,
    )
    .unwrap();
    assert_eq!(outcome, PrepareOutcome::Relabeled);
    assert_eq!(shell.calls.len(), 1);
    assert!(shell.calls[0].script.contains("Set-Volume"));
    assert!(!shell.calls[0].script.contains("Format-Volume"));
}

#[test]
fn non_empty_card_is_not_formatted_without_the_typed_word() {
    let mut shell = ShellLog::default();
    let error = prepare_volume(
        &card(false, "GAMES", "exFAT"),
        "EASYROMS",
        "",
        8_000_000_000,
        "exFAT",
        &mut shell,
    )
    .unwrap_err();
    assert!(error.to_lowercase().contains("format"));
    assert!(shell.calls.is_empty());
}

#[test]
fn typed_format_runs_only_the_format_script() {
    let mut shell = ShellLog::default();
    let outcome = prepare_volume(
        &card(false, "GAMES", "NTFS"),
        "EASYROMS",
        "FORMAT",
        8_000_000_000,
        "exFAT",
        &mut shell,
    )
    .unwrap();
    assert_eq!(outcome, PrepareOutcome::Formatted);
    assert_eq!(
        shell.calls,
        vec![ScriptCall {
            script: cfw_zero_touch_lib::format_gate::format_script("E", "exFAT", "EASYROMS")
                .unwrap(),
        }]
    );
}

#[test]
fn fat32_profile_formats_fat32_when_the_volume_is_small() {
    let mut shell = ShellLog::default();
    let outcome = prepare_volume(
        &card(false, "GAMES", "NTFS"),
        "ROMS",
        "FORMAT",
        8_000_000_000,
        "fat32",
        &mut shell,
    )
    .unwrap();
    assert_eq!(outcome, PrepareOutcome::Formatted);
    assert_eq!(
        shell.calls,
        vec![ScriptCall {
            script: cfw_zero_touch_lib::format_gate::format_script("E", "FAT32", "ROMS").unwrap(),
        }]
    );
}

#[test]
fn fat32_is_refused_on_volumes_over_32_gib() {
    let mut shell = ShellLog::default();
    let mut big = card(false, "GAMES", "exFAT");
    big.total_bytes = 33 * 1024 * 1024 * 1024;
    let error = prepare_volume(
        &big,
        "ROMS",
        "FORMAT",
        33 * 1024 * 1024 * 1024,
        "fat32",
        &mut shell,
    )
    .unwrap_err();
    assert!(error.contains("FAT32"), "{error}");
    assert!(shell.calls.is_empty());
}
