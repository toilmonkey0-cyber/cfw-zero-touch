use cfw_zero_touch_lib::profiles::{Profile, RomSchema, SystemFolder};

fn profile(layout: &str, bios: Option<&str>, systems: &[(&str, &str)]) -> Profile {
    Profile {
        id: "fixture".into(),
        version: 1,
        name: "Fixture".into(),
        cfw: "Fixture".into(),
        device_family: None,
        storage_modes: vec!["roms_card_only".into()],
        rom_schema: RomSchema {
            layout: layout.into(),
            format_fs: None,
            volume_label: None,
            bios_folder: bios.map(str::to_string),
            systems: systems
                .iter()
                .map(|(id, folder)| SystemFolder {
                    id: (*id).into(),
                    folder: (*folder).into(),
                    extensions: vec![],
                    dat_name_pattern: None,
                })
                .collect(),
        },
        image: None,
    }
}

#[test]
fn arkos_layout_puts_system_and_bios_folders_at_the_card_root() {
    let folders = cfw_zero_touch_lib::folder_map::folders_for(&profile(
        "arkos_easyroms_root",
        Some("bios"),
        &[("gba", "gba"), ("snes", "snes")],
    ))
    .expect("known layout");

    assert_eq!(folders, vec!["gba", "snes", "bios"]);
}

#[test]
fn rocknix_layout_nests_systems_under_roms() {
    let folders = cfw_zero_touch_lib::folder_map::folders_for(&profile(
        "rocknix_roms_nested",
        Some("bios"),
        &[("gba", "gba"), ("snes", "snes")],
    ))
    .expect("known layout");

    assert_eq!(folders, vec!["roms/gba", "roms/snes", "roms/bios"]);
}

#[test]
fn stock_r36s_layout_maps_games_under_roms_and_bios_at_the_root() {
    let folders = cfw_zero_touch_lib::folder_map::folders_for(&profile(
        "stock_r36s_roms",
        Some("bios"),
        &[
            ("gba", "gba"),
            ("snes", "snes"),
            ("nes", "nes"),
            ("psx", "psx"),
        ],
    ))
    .expect("known layout");

    assert_eq!(
        folders,
        vec!["Roms/GBA", "Roms/SFC", "Roms/FC", "Roms/PS", "BIOS"]
    );
}

#[test]
fn unknown_layout_is_rejected() {
    let error = cfw_zero_touch_lib::folder_map::folders_for(&profile("custom", None, &[]))
        .expect_err("custom has no built-in map");
    assert!(error.contains("custom"), "{error}");
}

#[test]
fn bios_folder_is_not_duplicated_when_a_system_already_uses_it() {
    let folders = cfw_zero_touch_lib::folder_map::folders_for(&profile(
        "arkos_easyroms_root",
        Some("bios"),
        &[("bios", "bios")],
    ))
    .expect("known layout");

    assert_eq!(folders, vec!["bios"]);
}
