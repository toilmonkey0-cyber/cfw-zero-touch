use std::fs;
use std::path::PathBuf;

use cfw_zero_touch_lib::store::{ensure_seeded, profile_store_dir, store_root};

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cfw-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn seeding_copies_missing_profiles_and_ignores_other_files() {
    let seed = scratch("seed");
    let store = scratch("store");
    fs::write(seed.join("a.json"), "{\"id\":\"a\"}").unwrap();
    fs::write(seed.join("b.json"), "{\"id\":\"b\"}").unwrap();
    fs::write(seed.join("readme.txt"), "not a profile").unwrap();

    let copied = ensure_seeded(&seed, &store).unwrap();

    assert_eq!(copied, 2);
    assert_eq!(
        fs::read_to_string(store.join("a.json")).unwrap(),
        "{\"id\":\"a\"}"
    );
    assert!(!store.join("readme.txt").exists());
    let _ = fs::remove_dir_all(&seed);
    let _ = fs::remove_dir_all(&store);
}

#[test]
fn seeding_never_overwrites_an_existing_store_file() {
    let seed = scratch("seed");
    let store = scratch("store");
    fs::write(seed.join("a.json"), "{\"id\":\"seeded\"}").unwrap();
    fs::write(store.join("a.json"), "{\"id\":\"user-edited\"}").unwrap();

    let copied = ensure_seeded(&seed, &store).unwrap();

    assert_eq!(copied, 0);
    assert_eq!(
        fs::read_to_string(store.join("a.json")).unwrap(),
        "{\"id\":\"user-edited\"}"
    );
    let _ = fs::remove_dir_all(&seed);
    let _ = fs::remove_dir_all(&store);
}

#[test]
fn store_root_honors_cfw_studio_data() {
    // Safe to set in-process: this is the only test in this binary that
    // reads CFW_STUDIO_DATA.
    std::env::set_var("CFW_STUDIO_DATA", r"C:\cfw-test-data");
    assert_eq!(store_root(), PathBuf::from(r"C:\cfw-test-data"));
    assert_eq!(
        profile_store_dir(),
        PathBuf::from(r"C:\cfw-test-data\profiles")
    );
    std::env::remove_var("CFW_STUDIO_DATA");
}
