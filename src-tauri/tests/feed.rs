use std::path::PathBuf;

use cfw_zero_touch_lib::feed::{app_version_newer, diff_feed, parse_feed};
use cfw_zero_touch_lib::profiles::{schema_validator, Profile};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn validator() -> jsonschema::Validator {
    schema_validator(&repo().join("specs").join("profile.schema.json")).unwrap()
}

fn profile_json(id: &str, version: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id, "version": version, "name": format!("{id} card"), "cfw": "x",
        "storageModes": ["roms_card_only"],
        "romSchema": { "layout": "arkos_easyroms_root", "systems": [] }
    })
}

fn feed_text(feed_version: u64, app_version: &str, profiles: &[serde_json::Value]) -> String {
    serde_json::json!({
        "feedVersion": feed_version,
        "app": { "version": app_version,
                 "releaseUrl": "https://example.com/releases" },
        "profiles": profiles
    })
    .to_string()
}

fn local(id: &str, version: u32) -> Profile {
    let mut value = profile_json(id, version as u64);
    // Local profiles normally come from disk without an explicit version.
    value.as_object_mut().unwrap().remove("version");
    serde_json::from_value(value).unwrap()
}

#[test]
fn parse_accepts_a_valid_feed_and_diff_reports_update_addition_and_app() {
    let text = feed_text(1, "0.2.0", &[profile_json("a", 2), profile_json("new", 1)]);
    let feed = parse_feed(&text, &validator()).unwrap();
    assert_eq!(feed.feed_version, 1);
    assert_eq!(feed.profiles.len(), 2);

    let diff = diff_feed(&[local("a", 1)], &feed, "0.1.0");
    assert_eq!(diff.updates.len(), 1);
    assert_eq!(diff.updates[0].id, "a");
    assert_eq!(diff.updates[0].local_version, 1);
    assert_eq!(diff.updates[0].remote_version, 2);
    assert_eq!(diff.additions.len(), 1);
    assert_eq!(diff.additions[0].id, "new");
    assert_eq!(diff.app_update.as_ref().unwrap().version, "0.2.0");
}

#[test]
fn parse_rejects_an_unknown_feed_version_whole() {
    let text = feed_text(2, "0.2.0", &[profile_json("a", 2)]);
    let error = parse_feed(&text, &validator()).unwrap_err();
    assert!(error.contains("feed version 2"), "{error}");
}

#[test]
fn parse_rejects_a_schema_invalid_entry_and_duplicate_ids() {
    let mut broken = profile_json("broken", 1);
    broken
        .as_object_mut()
        .unwrap()
        .insert("storageModes".into(), serde_json::json!("not-an-array"));
    let error = parse_feed(
        &feed_text(1, "0.1.0", &[profile_json("a", 1), broken]),
        &validator(),
    )
    .unwrap_err();
    assert!(error.to_lowercase().contains("schema"), "{error}");

    let error = parse_feed(
        &feed_text(1, "0.1.0", &[profile_json("a", 1), profile_json("a", 2)]),
        &validator(),
    )
    .unwrap_err();
    assert!(error.contains("twice"), "{error}");
}

#[test]
fn equal_or_older_versions_and_older_app_versions_do_not_diff() {
    let text = feed_text(1, "0.1.0", &[profile_json("a", 1), profile_json("b", 1)]);
    let feed = parse_feed(&text, &validator()).unwrap();
    let diff = diff_feed(&[local("a", 1), local("b", 3)], &feed, "0.2.0");
    assert!(diff.updates.is_empty());
    assert!(diff.additions.is_empty());
    assert!(diff.app_update.is_none());
}

#[test]
fn app_version_compare_is_numeric_then_string() {
    assert!(app_version_newer("0.10.0", "0.9.0"));
    assert!(!app_version_newer("0.9.0", "0.10.0"));
    assert!(app_version_newer("1.0.0", "0.9.9"));
    assert!(!app_version_newer("0.1.0", "0.1.0"));
    assert!(app_version_newer("0.1.1", "0.1.0"));
    assert!(app_version_newer("nightly-2", "nightly-1"));
    assert!(!app_version_newer("0.1.0", "dev"));
}

use cfw_zero_touch_lib::feed::apply_feed;
use std::fs;

fn scratch_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cfw-feed-apply-{}-{}",
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
fn an_invalid_feed_writes_nothing() {
    let store = scratch_dir();
    fs::write(
        store.join("a.json"),
        serde_json::to_string(&local("a", 1)).unwrap(),
    )
    .unwrap();
    let mut broken = profile_json("a", 2);
    broken
        .as_object_mut()
        .unwrap()
        .insert("storageModes".into(), serde_json::json!("not-an-array"));
    let error = parse_feed(&feed_text(1, "0.1.0", &[broken]), &validator()).unwrap_err();
    assert!(error.to_lowercase().contains("schema"), "{error}");
    let stored: Profile =
        serde_json::from_str(&fs::read_to_string(store.join("a.json")).unwrap()).unwrap();
    assert_eq!(stored.version, 1);
    let _ = fs::remove_dir_all(&store);
}

#[test]
fn apply_writes_updates_to_the_existing_filename_and_additions_as_id_json() {
    let store = scratch_dir();
    fs::write(
        store.join("a-card.example.json"),
        serde_json::to_string(&local("a", 1)).unwrap(),
    )
    .unwrap();

    let feed = parse_feed(
        &feed_text(1, "0.1.0", &[profile_json("a", 2), profile_json("new", 1)]),
        &validator(),
    )
    .unwrap();
    let report = apply_feed(&store, &feed, &[local("a", 1)]).unwrap();

    assert_eq!(report.applied, 2);
    let updated: Profile =
        serde_json::from_str(&fs::read_to_string(store.join("a-card.example.json")).unwrap())
            .unwrap();
    assert_eq!(updated.version, 2);
    let added: Profile =
        serde_json::from_str(&fs::read_to_string(store.join("new.json")).unwrap()).unwrap();
    assert_eq!(added.id, "new");
    let _ = fs::remove_dir_all(&store);
}

#[test]
fn apply_skips_equal_and_older_entries_without_writing() {
    let store = scratch_dir();
    let versioned: Profile = serde_json::from_value(profile_json("a", 5)).unwrap();
    fs::write(
        store.join("a.json"),
        serde_json::to_string(&versioned).unwrap(),
    )
    .unwrap();

    let feed = parse_feed(
        &feed_text(1, "0.1.0", &[profile_json("a", 5)]),
        &validator(),
    )
    .unwrap();
    let report = apply_feed(&store, &feed, std::slice::from_ref(&versioned)).unwrap();
    assert_eq!((report.applied, report.skipped), (0, 1));

    let feed = parse_feed(
        &feed_text(1, "0.1.0", &[profile_json("a", 4)]),
        &validator(),
    )
    .unwrap();
    let report = apply_feed(&store, &feed, &[versioned]).unwrap();
    assert_eq!((report.applied, report.skipped), (0, 1));

    let stored: Profile =
        serde_json::from_str(&fs::read_to_string(store.join("a.json")).unwrap()).unwrap();
    assert_eq!(stored.version, 5);
    let _ = fs::remove_dir_all(&store);
}

#[test]
fn fetch_reports_an_unreachable_feed_as_an_error() {
    // Port 9 (discard) is closed on Windows: connection refused, fast.
    let error = cfw_zero_touch_lib::feed::fetch_text("http://127.0.0.1:9/feed.json").unwrap_err();
    assert!(!error.is_empty());
}

#[test]
fn checked_in_feed_matches_shipped_profiles() {
    let schema = repo().join("specs").join("profile.schema.json");
    let validator = schema_validator(&schema).unwrap();
    let text = fs::read_to_string(repo().join("profiles-feed.json"))
        .expect("profiles-feed.json must stay checked in beside profiles/");
    let feed = parse_feed(&text, &validator).unwrap();
    let shipped =
        cfw_zero_touch_lib::profiles::load_profiles(&repo().join("profiles"), &schema).unwrap();
    assert_eq!(
        feed.profiles.len(),
        shipped.len(),
        "feed and profiles/ disagree on count"
    );
    for profile in &shipped {
        let remote = feed
            .profiles
            .iter()
            .find(|candidate| candidate.id == profile.id)
            .unwrap_or_else(|| panic!("{} is missing from profiles-feed.json", profile.id));
        assert_eq!(
            remote, profile,
            "{} differs between feed and profiles/",
            profile.id
        );
        assert!(remote.version >= 1);
    }
    assert!(!feed.app.version.is_empty());
}

#[test]
fn applied_profiles_reload_through_the_schema() {
    let store = scratch_dir();
    let feed = parse_feed(
        &feed_text(1, "0.1.0", &[profile_json("a", 2)]),
        &validator(),
    )
    .unwrap();
    apply_feed(&store, &feed, &[local("a", 1)]).unwrap();
    let schema = repo().join("specs").join("profile.schema.json");
    let reloaded = cfw_zero_touch_lib::profiles::load_profiles(&store, &schema)
        .expect("applied profiles must reload through the schema");
    assert!(reloaded.iter().any(|p| p.id == "a" && p.version == 2));
    let _ = fs::remove_dir_all(&store);
}

#[test]
fn release_urls_must_be_https() {
    let text = feed_text(1, "0.1.0", &[]);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut evil = value.clone();
    evil["app"]["releaseUrl"] = serde_json::json!("http://example.com/releases");
    let error = parse_feed(&evil.to_string(), &validator()).unwrap_err();
    assert!(error.contains("https"), "{error}");
}
