use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn example_arkos_profile_validates_and_loads() {
    let profiles = cfw_zero_touch_lib::profiles::load_profiles(
        &repo_root().join("profiles"),
        &repo_root().join("specs").join("profile.schema.json"),
    )
    .expect("profiles directory should load");

    let arkos = profiles
        .iter()
        .find(|profile| profile.id == "arkos-roms-card")
        .expect("example ArkOS profile");

    assert_eq!(arkos.rom_schema.layout, "arkos_easyroms_root");
    assert_eq!(arkos.rom_schema.volume_label.as_deref(), Some("EASYROMS"));
    assert!(arkos
        .storage_modes
        .iter()
        .any(|mode| mode == "roms_card_only"));
    assert!(arkos
        .rom_schema
        .systems
        .iter()
        .any(|system| system.folder == "gba"));

    let rocknix = profiles
        .iter()
        .find(|profile| profile.id == "rocknix-roms-card")
        .expect("ROCKNIX ROMs card profile");
    assert_eq!(rocknix.rom_schema.layout, "rocknix_roms_nested");
    assert!(rocknix
        .storage_modes
        .iter()
        .any(|mode| mode == "roms_card_only"));

    let os = profiles
        .iter()
        .find(|profile| profile.id == "rocknix-rgb10x-os")
        .expect("RGB10X OS profile");
    assert_eq!(
        os.image.as_ref().map(|image| image.sha256.as_str()),
        Some("f2b35e9feeef1a2ba2a40298614c1e9d67349ffd98e8498449e8815107ab82cc")
    );

    let darkos = profiles
        .iter()
        .find(|profile| profile.id == "darkos-rgb10x-os")
        .expect("dArkOS RGB10X profile");
    assert_eq!(darkos.image.as_ref().map(|image| image.parts.len()), Some(2));
    assert_eq!(
        darkos.image.as_ref().and_then(|image| image.compressed.as_deref()),
        Some("7z")
    );

    let stock = profiles
        .iter()
        .find(|profile| profile.id == "r35s-stock-card")
        .expect("R35S stock games-card profile");
    assert_eq!(stock.rom_schema.layout, "stock_r36s_roms");
    assert_eq!(stock.rom_schema.format_fs.as_deref(), Some("fat32"));
    assert_eq!(stock.rom_schema.volume_label.as_deref(), Some("ROMS"));
    assert!(stock.image.is_none());

    let clone = profiles
        .iter()
        .find(|profile| profile.id == "r36s-clone-card")
        .expect("R36S clone games-card profile");
    assert_eq!(clone.rom_schema.layout, "arkos_easyroms_root");
    assert_eq!(clone.rom_schema.format_fs.as_deref(), Some("fat32"));
    assert_eq!(clone.rom_schema.volume_label.as_deref(), Some("ROMS"));
    assert!(clone.image.is_none());
}

#[test]
fn malformed_profile_is_rejected() {
    let dir = std::env::temp_dir().join(format!("cfw-bad-profile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.json"), r#"{"id":"NOT VALID"}"#).unwrap();

    let error = cfw_zero_touch_lib::profiles::load_profiles(
        &dir,
        &repo_root().join("specs").join("profile.schema.json"),
    )
    .expect_err("schema violations must fail the load");

    let message = error.to_string();
    assert!(
        message.contains("broken.json"),
        "error should name the file: {message}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cfw-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn version_defaults_to_one_and_parses() {
    let dir = scratch("profiles-version");
    std::fs::write(
        dir.join("a.json"),
        r#"{"id":"a","name":"A","cfw":"x","storageModes":["roms_card_only"],
            "romSchema":{"layout":"arkos_easyroms_root","systems":[]}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("b.json"),
        r#"{"id":"b","version":7,"name":"B","cfw":"x","storageModes":["roms_card_only"],
            "romSchema":{"layout":"arkos_easyroms_root","systems":[]}}"#,
    )
    .unwrap();
    let schema = repo_root().join("specs").join("profile.schema.json");
    let profiles = cfw_zero_touch_lib::profiles::load_profiles(&dir, &schema).unwrap();
    let a = profiles.iter().find(|p| p.id == "a").unwrap();
    let b = profiles.iter().find(|p| p.id == "b").unwrap();
    assert_eq!(a.version, 1);
    assert_eq!(b.version, 7);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dat_name_pattern_parses_and_roundtrips() {
    let dir = scratch("profiles-datpattern");
    std::fs::write(
        dir.join("a.json"),
        r#"{"id":"a","name":"A","cfw":"x","storageModes":["roms_card_only"],
            "romSchema":{"layout":"arkos_easyroms_root","systems":[
                {"id":"gb","folder":"gb","extensions":[".gb"],"datNamePattern":"^Nintendo - Game Boy(?! Advance| Color)"},
                {"id":"arcade","folder":"arcade","extensions":[".zip"]}
            ]}}"#,
    )
    .unwrap();
    let schema = repo_root().join("specs").join("profile.schema.json");
    let profiles = cfw_zero_touch_lib::profiles::load_profiles(&dir, &schema).unwrap();
    let systems = &profiles[0].rom_schema.systems;
    assert_eq!(
        systems[0].dat_name_pattern.as_deref(),
        Some("^Nintendo - Game Boy(?! Advance| Color)")
    );
    assert_eq!(systems[1].dat_name_pattern, None);

    // Serialization keeps the field and still omits the absent one.
    let text = serde_json::to_string(&profiles[0]).unwrap();
    assert!(text.contains("datNamePattern"));
    let reparsed: cfw_zero_touch_lib::profiles::Profile = serde_json::from_str(&text).unwrap();
    assert_eq!(reparsed, profiles[0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsafe_folders_and_patterns_are_rejected_at_load() {
    let dir = scratch("profiles-unsafe");
    std::fs::write(
        dir.join("a.json"),
        r#"{"id":"a","name":"A","cfw":"x","storageModes":["roms_card_only"],
            "romSchema":{"layout":"arkos_easyroms_root","systems":[
                {"id":"evil","folder":"../evil","extensions":[".zip"]}]}}"#,
    )
    .unwrap();
    let schema = repo_root().join("specs").join("profile.schema.json");
    let error = cfw_zero_touch_lib::profiles::load_profiles(&dir, &schema).unwrap_err();
    assert!(error.to_string().contains("folder"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);

    let dir = scratch("profiles-unsafe2");
    std::fs::write(
        dir.join("a.json"),
        r#"{"id":"a","name":"A","cfw":"x","storageModes":["roms_card_only"],
            "romSchema":{"layout":"arkos_easyroms_root","systems":[
                {"id":"gb","folder":"gb","datNamePattern":"^x\" --evil"}]}}"#,
    )
    .unwrap();
    let error = cfw_zero_touch_lib::profiles::load_profiles(&dir, &schema).unwrap_err();
    assert!(error.to_string().contains("datNamePattern"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}
