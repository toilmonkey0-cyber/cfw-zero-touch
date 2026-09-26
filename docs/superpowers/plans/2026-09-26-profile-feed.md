# Profile feed + update checker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a profile-pack feed the app checks and applies on demand, plus a feed-carried app-update notice, per the approved spec.

**Architecture:** A versioned `profiles-feed.json` at the repo root is fetched over HTTPS (ureq, injectable for tests), schema-validated entry-by-entry, diffed against a writable runtime profile store (`%LOCALAPPDATA%\cfw-card-studio\profiles`, seeded from bundled profiles), and applied only on an explicit UI click. The feed also carries the latest app version, compared against `CARGO_PKG_VERSION` for a "new build available" banner.

**Tech Stack:** Rust (Tauri 2 backend, serde, serde_json, jsonschema 0.57, ureq 2), React + TypeScript frontend, existing Vitest-free test layout (`src-tauri/tests/*.rs` integration tests).

**Spec:** `docs/superpowers/specs/2026-09-26-profile-feed-design.md`

## Global Constraints

- Windows-first app; no new crates beyond those already in `src-tauri/Cargo.toml`.
- Default feed URL: `https://raw.githubusercontent.com/toilmonkey0-cyber/cfw-zero-touch/main/profiles-feed.json`; `CFW_FEED_URL` env overrides it.
- Runtime store: `CFW_STUDIO_DATA` env overrides, else `%LOCALAPPDATA%\cfw-card-studio\profiles`.
- Seed source order: `CFW_STUDIO_ROOT/profiles`, then `repo_root()/profiles` (dev), then Tauri resource dir `profiles` (packaged). Same order for `specs/profile.schema.json`.
- Profile schema gains optional integer `version` (minimum 1, default 1). Unknown `feedVersion` values reject the whole feed.
- Feed entries are schema-validated before any write; apply is all-or-nothing on validation failure.
- The app never downloads games or BIOS; no user data leaves the machine beyond the feed GET.
- Unit/integration tests stay offline (the only network touch is a refused-connection test against `http://127.0.0.1:9/`).
- Tests use the repo's scratch pattern: `std::env::temp_dir().join(format!("cfw-{name}-{}-{}", process::id(), subsec_nanos))`.
- Commit style: imperative one-liners matching `git log` (e.g. "Add the Phase 4 profile feed design spec.").

## Review Focus

1. **Future feed (`feedVersion: 2`) must not partially apply** — an app updated by someone else's newer feed rejects it whole and keeps working. Test in Task 3: `parse_feed` returns Err for `feedVersion: 2`.
2. **One schema-invalid entry must write nothing** — a typo'd profile in the feed cannot leave the store half-updated. Test in Task 3: parse fails; Task 4 test re-asserts store unchanged after a failed parse+apply attempt sequence.
3. **Equal or older remote versions leave files untouched** — a feed replay must be a no-op. Test in Task 4: `apply_feed` skips equal/older and writes nothing.
4. **Seeding must never overwrite an existing store file** — a user-edited profile survives reinstalls and app updates. Test in Task 2: pre-existing file with different content is preserved.
5. **Unreachable feed must not break the wizard** — offline shows "Could not check", profiles still list. Test in Task 5 (`fetch_text` against a closed port) and Task 8 (offline UI state in the running app).

---

### Task 1: Profile `version` field

**Files:**
- Modify: `src-tauri/src/profiles.rs`
- Modify: `specs/profile.schema.json`
- Test: `src-tauri/tests/profiles.rs`

**Interfaces:**
- Consumes: existing `Profile` struct (serde, `PartialEq`).
- Produces: `Profile.version: u32` (serde name `"version"`, default 1 via `fn default_version() -> u32`), available to Tasks 3–5; `pub fn schema_validator(schema_path: &Path) -> Result<jsonschema::Validator, ProfileError>` used by Task 3 and the drift test.

- [ ] **Step 1: Write the failing test** — append to `src-tauri/tests/profiles.rs` (reuse the file's existing fixture helpers; if it has none, add the scratch helper shown):

```rust
fn scratch(name: &str) -> std::path::PathBuf {
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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let schema = root.join("specs").join("profile.schema.json");
    let profiles = cfw_zero_touch_lib::profiles::load_profiles(&dir, &schema).unwrap();
    let a = profiles.iter().find(|p| p.id == "a").unwrap();
    let b = profiles.iter().find(|p| p.id == "b").unwrap();
    assert_eq!(a.version, 1);
    assert_eq!(b.version, 7);
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test profiles version_defaults` (from `src-tauri`)
Expected: FAIL — `no field 'version'` (compile error).

- [ ] **Step 3: Implement** — in `src-tauri/src/profiles.rs`, add to `Profile` after the `id` field:

```rust
#[serde(rename = "version", default = "default_version")]
pub version: u32,
```

and at module scope:

```rust
fn default_version() -> u32 {
    1
}
```

In `specs/profile.schema.json`, add inside `"properties"` next to `"id"`:

```json
"version": { "type": "integer", "minimum": 1 },
```

Also refactor `load_profiles`'s inline validator construction into a public helper (same body, moved):

```rust
pub fn schema_validator(schema_path: &Path) -> Result<jsonschema::Validator, ProfileError> {
    let schema_text = fs::read_to_string(schema_path).map_err(|error| {
        ProfileError::new(format!(
            "could not read schema {}: {error}",
            schema_path.display()
        ))
    })?;
    let schema: Value = serde_json::from_str(&schema_text)
        .map_err(|error| ProfileError::new(format!("schema is not JSON: {error}")))?;
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .map_err(|error| ProfileError::new(format!("schema failed to compile: {error}")))
}
```

and call it from `load_profiles`.

- [ ] **Step 4: Run all profile tests**

Run: `cargo test --test profiles`
Expected: PASS (existing tests plus the new one).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/profiles.rs specs/profile.schema.json src-tauri/tests/profiles.rs
git commit -m "Add a version field to profiles."
```

---

### Task 2: Runtime profile store

**Files:**
- Create: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/lib.rs` (one line: `pub mod store;`)
- Test: `src-tauri/tests/store.rs`

**Interfaces:**
- Produces (used by Task 5): `pub fn store_root() -> PathBuf`, `pub fn profile_store_dir() -> PathBuf`, `pub fn ensure_seeded(seed_dir: &Path, store_dir: &Path) -> Result<usize, String>` (returns number of files copied).

- [ ] **Step 1: Write the failing tests** — create `src-tauri/tests/store.rs`:

```rust
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
    assert_eq!(fs::read_to_string(store.join("a.json")).unwrap(), "{\"id\":\"a\"}");
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
    assert_eq!(fs::read_to_string(store.join("a.json")).unwrap(), "{\"id\":\"user-edited\"}");
    let _ = fs::remove_dir_all(&seed);
    let _ = fs::remove_dir_all(&store);
}

#[test]
fn store_root_honors_cfw_studio_data() {
    // Safe to set in-process: Rust 2021 std::env::set_var is not yet unsafe,
    // and no other test in this binary reads CFW_STUDIO_DATA.
    std::env::set_var("CFW_STUDIO_DATA", r"C:\cfw-test-data");
    assert_eq!(store_root(), PathBuf::from(r"C:\cfw-test-data"));
    assert_eq!(profile_store_dir(), PathBuf::from(r"C:\cfw-test-data\profiles"));
    std::env::remove_var("CFW_STUDIO_DATA");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test store`
Expected: FAIL — unresolved import `cfw_zero_touch_lib::store`.

- [ ] **Step 3: Implement** — create `src-tauri/src/store.rs`:

```rust
use std::fs;
use std::path::{Path, PathBuf};

/// Root of Card Studio's writable data. `CFW_STUDIO_DATA` overrides it for
/// tests; installed builds use %LOCALAPPDATA%\cfw-card-studio.
pub fn store_root() -> PathBuf {
    if let Ok(root) = std::env::var("CFW_STUDIO_DATA") {
        return PathBuf::from(root);
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local).join("cfw-card-studio");
    }
    std::env::temp_dir().join("cfw-card-studio")
}

pub fn profile_store_dir() -> PathBuf {
    store_root().join("profiles")
}

/// Copy seed profiles the store is missing. Existing files are never
/// overwritten, so user-edited profiles survive seeding.
pub fn ensure_seeded(seed_dir: &Path, store_dir: &Path) -> Result<usize, String> {
    fs::create_dir_all(store_dir)
        .map_err(|error| format!("could not create {}: {error}", store_dir.display()))?;
    let entries = fs::read_dir(seed_dir)
        .map_err(|error| format!("could not read {}: {error}", seed_dir.display()))?;
    let mut seeded = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let target = store_dir.join(entry.file_name());
        if target.exists() {
            continue;
        }
        fs::copy(&path, &target)
            .map_err(|error| format!("could not seed {}: {error}", target.display()))?;
        seeded += 1;
    }
    Ok(seeded)
}
```

Add `pub mod store;` to `src-tauri/src/lib.rs` beside the other module declarations.

- [ ] **Step 4: Run the tests**

Run: `cargo test --test store`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/store.rs src-tauri/src/lib.rs src-tauri/tests/store.rs
git commit -m "Add the runtime profile store with seeding."
```

---

### Task 3: Feed parsing, diffing, version compare

**Files:**
- Create: `src-tauri/src/feed.rs`
- Modify: `src-tauri/src/lib.rs` (one line: `pub mod feed;`)
- Test: `src-tauri/tests/feed.rs`

**Interfaces:**
- Consumes: `Profile.version` (Task 1), `profiles::schema_validator` (Task 1).
- Produces (used by Tasks 4–7): `FEED_VERSION`, `DEFAULT_FEED_URL`, `pub fn feed_url() -> String`, `FeedApp { version: String, release_url: String }`, `ProfileFeed { feed_version: u32, app: FeedApp, profiles: Vec<Profile> }`, `ProfileChange { id, name, local_version, remote_version }`, `FeedDiff { updates: Vec<ProfileChange>, additions: Vec<Profile>, app_update: Option<FeedApp> }`, `pub fn parse_feed(text: &str, validator: &jsonschema::Validator) -> Result<ProfileFeed, String>`, `pub fn diff_feed(local: &[Profile], feed: &ProfileFeed, running_version: &str) -> FeedDiff`, `pub fn app_version_newer(remote: &str, local: &str) -> bool`.

- [ ] **Step 1: Write the failing tests** — create `src-tauri/tests/feed.rs` with a real-schema validator and three fixtures (valid feed with one update + one addition + newer app; a `feedVersion: 2` feed; a feed whose second entry breaks the schema). Use this shape:

```rust
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
    let error = parse_feed(&feed_text(1, "0.1.0", &[profile_json("a", 1), broken]), &validator()).unwrap_err();
    assert!(error.to_lowercase().contains("schema"), "{error}");

    let error = parse_feed(&feed_text(1, "0.1.0", &[profile_json("a", 1), profile_json("a", 2)]), &validator()).unwrap_err();
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
```

Note: `jsonschema` and `serde_json` are dev-reachable in integration tests through the lib crate only if re-exported; if the test cannot name `jsonschema::Validator`, add to `src-tauri/Cargo.toml` a `[dev-dependencies]` section with `jsonschema = { version = "0.57.0", default-features = false }` and `serde_json = "1"` (both already main deps; dev-deps make them nameable from tests).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test feed`
Expected: FAIL — unresolved import `cfw_zero_touch_lib::feed`.

- [ ] **Step 3: Implement** — create `src-tauri/src/feed.rs`:

```rust
use std::collections::HashSet;
use std::path::PathBuf;

use crate::profiles::Profile;

pub const FEED_VERSION: u32 = 1;
pub const DEFAULT_FEED_URL: &str =
    "https://raw.githubusercontent.com/toilmonkey0-cyber/cfw-zero-touch/main/profiles-feed.json";

pub fn feed_url() -> String {
    std::env::var("CFW_FEED_URL").unwrap_or_else(|_| DEFAULT_FEED_URL.into())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedApp {
    pub version: String,
    pub release_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileFeed {
    pub feed_version: u32,
    pub app: FeedApp,
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileChange {
    pub id: String,
    pub name: String,
    pub local_version: u32,
    pub remote_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FeedDiff {
    pub updates: Vec<ProfileChange>,
    pub additions: Vec<Profile>,
    pub app_update: Option<FeedApp>,
}

/// Validate a feed body against the profile schema. Any invalid entry,
/// duplicate id, or unknown feedVersion rejects the whole feed so apply can
/// never be partial.
pub fn parse_feed(text: &str, validator: &jsonschema::Validator) -> Result<ProfileFeed, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("feed is not JSON: {error}"))?;
    let feed_version = value.get("feedVersion").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if feed_version != FEED_VERSION {
        return Err(format!(
            "unsupported feed version {feed_version}; this app understands version {FEED_VERSION}"
        ));
    }
    let entries = value
        .get("profiles")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut profiles = Vec::new();
    let mut seen = HashSet::new();
    for entry in &entries {
        let errors = validator.iter_errors(entry).into_errors();
        if !errors.is_empty() {
            return Err(format!("a feed profile does not match the profile schema: {errors}"));
        }
        let profile: Profile = serde_json::from_value(entry.clone())
            .map_err(|error| format!("a feed profile could not be read: {error}"))?;
        if !seen.insert(profile.id.clone()) {
            return Err(format!("feed lists profile {} twice", profile.id));
        }
        profiles.push(profile);
    }
    let app: FeedApp = serde_json::from_value(value.get("app").cloned().unwrap_or(serde_json::Value::Null))
        .map_err(|error| format!("feed app section is invalid: {error}"))?;
    Ok(ProfileFeed { feed_version, app, profiles })
}

pub fn diff_feed(local: &[Profile], feed: &ProfileFeed, running_version: &str) -> FeedDiff {
    let mut updates = Vec::new();
    let mut additions = Vec::new();
    for remote in &feed.profiles {
        match local.iter().find(|current| current.id == remote.id) {
            Some(current) if remote.version > current.version => updates.push(ProfileChange {
                id: remote.id.clone(),
                name: remote.name.clone(),
                local_version: current.version,
                remote_version: remote.version,
            }),
            Some(_) => {}
            None => additions.push(remote.clone()),
        }
    }
    let app_update = if app_version_newer(&feed.app.version, running_version) {
        Some(feed.app.clone())
    } else {
        None
    };
    FeedDiff { updates, additions, app_update }
}

fn version_parts(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Semver-aware on major.minor.patch with a plain string fallback.
pub fn app_version_newer(remote: &str, local: &str) -> bool {
    match (version_parts(remote), version_parts(local)) {
        (Some(remote), Some(local)) => remote > local,
        _ => remote != local && remote > local,
    }
}
```

Add `pub mod feed;` to `lib.rs`. If the `PathBuf` import is unused after Task 3 (apply arrives in Task 4), keep the module compiling by omitting it until Task 4 adds it.

- [ ] **Step 4: Run the tests**

Run: `cargo test --test feed`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/feed.rs src-tauri/src/lib.rs src-tauri/tests/feed.rs src-tauri/Cargo.toml
git commit -m "Parse and diff the profile feed."
```

---

### Task 4: Feed apply

**Files:**
- Modify: `src-tauri/src/feed.rs`
- Test: `src-tauri/tests/feed.rs`

**Interfaces:**
- Consumes: `ProfileFeed` (Task 3).
- Produces (used by Task 5): `pub struct ApplyReport { pub applied: usize, pub skipped: usize }`, `pub fn apply_feed(store_dir: &Path, feed: &ProfileFeed, local: &[Profile]) -> Result<ApplyReport, String>`.

- [ ] **Step 1: Write the failing tests** — append to `src-tauri/tests/feed.rs`:

```rust
use cfw_zero_touch_lib::feed::apply_feed;
use std::fs;

#[test]
fn an_invalid_feed_writes_nothing() {
    let store = scratch_dir();
    fs::write(store.join("a.json"), serde_json::to_string(&local("a", 1)).unwrap()).unwrap();
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
        serde_json::from_str(&fs::read_to_string(store.join("a-card.example.json")).unwrap()).unwrap();
    assert_eq!(updated.version, 2);
    let added: Profile =
        serde_json::from_str(&fs::read_to_string(store.join("new.json")).unwrap()).unwrap();
    assert_eq!(added.id, "new");
    let _ = fs::remove_dir_all(&store);
}

#[test]
fn apply_skips_equal_and_older_entries_without_writing() {
    let store = scratch_dir();
    fs::write(store.join("a.json"), serde_json::to_string(&local("a", 5)).unwrap()).unwrap();

    let feed = parse_feed(&feed_text(1, "0.1.0", &[profile_json("a", 5)]), &validator()).unwrap();
    let report = apply_feed(&store, &feed, &[local("a", 5)]).unwrap();
    assert_eq!((report.applied, report.skipped), (0, 1));

    let feed = parse_feed(&feed_text(1, "0.1.0", &[profile_json("a", 4)]), &validator()).unwrap();
    let report = apply_feed(&store, &feed, &[local("a", 5)]).unwrap();
    assert_eq!((report.applied, report.skipped), (0, 1));

    let stored: Profile = serde_json::from_str(&fs::read_to_string(store.join("a.json")).unwrap()).unwrap();
    assert_eq!(stored.version, 5);
    let _ = fs::remove_dir_all(&store);
}

fn scratch_dir() -> std::path::PathBuf {
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test feed apply_`
Expected: FAIL — unresolved `apply_feed`.

- [ ] **Step 3: Implement** — add to `src-tauri/src/feed.rs` (add `use std::fs;` and `use std::path::Path;`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub applied: usize,
    pub skipped: usize,
}

/// Write feed entries that are newer or new into the store. An entry whose
/// id already lives in a file keeps that filename (e.g. an `.example.json`
/// seed), so updates land where the profile is listed from.
pub fn apply_feed(
    store_dir: &Path,
    feed: &ProfileFeed,
    local: &[Profile],
) -> Result<ApplyReport, String> {
    let mut report = ApplyReport { applied: 0, skipped: 0 };
    for remote in &feed.profiles {
        if let Some(current) = local.iter().find(|current| current.id == remote.id) {
            if remote.version <= current.version {
                report.skipped += 1;
                continue;
            }
        }
        let target = match profile_file_for_id(store_dir, &remote.id)? {
            Some(path) => path,
            None => store_dir.join(format!("{}.json", remote.id)),
        };
        let text = serde_json::to_string_pretty(remote)
            .map_err(|error| format!("could not encode profile {}: {error}", remote.id))?;
        fs::write(&target, text)
            .map_err(|error| format!("could not write {}: {error}", target.display()))?;
        report.applied += 1;
    }
    Ok(report)
}

fn profile_file_for_id(store_dir: &Path, id: &str) -> Result<Option<std::path::PathBuf>, String> {
    let entries = fs::read_dir(store_dir)
        .map_err(|error| format!("could not read {}: {error}", store_dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(profile) = serde_json::from_str::<Profile>(&text) {
            if profile.id == id {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --test feed`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/feed.rs src-tauri/tests/feed.rs
git commit -m "Apply feed updates into the profile store."
```

---

### Task 5: Backend wiring — store-backed loading, fetch, commands

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/tauri.conf.json` (bundle resources)
- Modify: `src-tauri/capabilities/default.json`
- Test: `src-tauri/tests/feed.rs` (fetch failure path)

**Interfaces:**
- Consumes: `store::{profile_store_dir, ensure_seeded}` (Task 2), `feed::{feed_url, fetch_text, parse_feed, diff_feed, apply_feed, FeedDiff, ApplyReport, ProfileChange, FeedApp}` (Tasks 3–4), `profiles::schema_validator` (Task 1).
- Produces: Tauri commands `check_profile_feed(app) -> FeedCheckView`, `apply_profile_feed(app) -> ApplyReport`, `app_version() -> String`; `feed::fetch_text(url) -> Result<String, String>`; `load_all(app)` reading the seeded store. Frontend payload shapes (camelCase): `{ status: "up_to_date" | "updates" | "error", reason: string, updates: ProfileChange[], additions: string[], appUpdate: FeedApp | null }` and `{ applied: number, skipped: number }`.

- [ ] **Step 1: Write the failing test** — append to `src-tauri/tests/feed.rs`:

```rust
#[test]
fn fetch_reports_an_unreachable_feed_as_an_error() {
    // Port 9 (discard) is closed on Windows: connection refused, fast.
    let error = cfw_zero_touch_lib::feed::fetch_text("http://127.0.0.1:9/feed.json").unwrap_err();
    assert!(!error.is_empty());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test feed fetch_reports`
Expected: FAIL — unresolved `fetch_text`.

- [ ] **Step 3: Implement `fetch_text`** — add to `src-tauri/src/feed.rs`:

```rust
/// Fetch the feed body over HTTPS with a hard timeout. The only network
/// touch in this module; commands treat failure as a soft state.
pub fn fetch_text(url: &str) -> Result<String, String> {
    let response = ureq::get(url)
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|error| format!("could not reach the profile feed: {error}"))?;
    let mut body = String::new();
    response
        .into_reader()
        .take(4 * 1024 * 1024)
        .read_to_string(&mut body)
        .map_err(|error| format!("could not read the profile feed: {error}"))?;
    Ok(body)
}
```

with `use std::io::Read;` at the top of `feed.rs`.

- [ ] **Step 4: Wire the commands** — in `src-tauri/src/lib.rs`:

1. Add `use tauri::Manager;` (for `app.path()`).
2. Replace `load_all()` with a version taking `&tauri::AppHandle`:

```rust
fn seed_candidates(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(root) = std::env::var("CFW_STUDIO_ROOT") {
        candidates.push(PathBuf::from(root).join("profiles"));
    }
    candidates.push(repo_root().join("profiles"));
    if let Ok(resource) = app.path().resource_dir() {
        candidates.push(resource.join("profiles"));
    }
    candidates
}

fn schema_candidates(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(root) = std::env::var("CFW_STUDIO_ROOT") {
        candidates.push(PathBuf::from(root).join("specs").join("profile.schema.json"));
    }
    candidates.push(repo_root().join("specs").join("profile.schema.json"));
    if let Ok(resource) = app.path().resource_dir() {
        candidates.push(resource.join("specs").join("profile.schema.json"));
    }
    candidates
}

fn load_all(app: &tauri::AppHandle) -> Result<Vec<Profile>, String> {
    let store = store::profile_store_dir();
    if let Some(seed) = seed_candidates(app).into_iter().find(|dir| dir.is_dir()) {
        store::ensure_seeded(&seed, &store)?;
    }
    let schema = schema_candidates(app)
        .into_iter()
        .find(|path| path.is_file())
        .ok_or("profile schema is missing")?;
    profiles::load_profiles(&store, &schema).map_err(|error| error.to_string())
}
```

3. Thread `app: tauri::AppHandle` as the first parameter into every command that calls `load_all`/`profile_by_id` today: `list_profiles`, `list_volumes`, `prepare_card`, `seed_card`, `plan_roms`, `copy_roms`, `flash_os`, `firstboot_state`. Tauri injects it; the frontend calls stay unchanged.
4. Add the new commands:

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedCheckView {
    status: String,
    reason: String,
    updates: Vec<feed::ProfileChange>,
    additions: Vec<String>,
    app_update: Option<feed::FeedApp>,
}

fn feed_diff(app: &tauri::AppHandle) -> Result<feed::FeedDiff, String> {
    let local = load_all(app)?;
    let url = feed::feed_url();
    let text = feed::fetch_text(&url)?;
    let schema = schema_candidates(app)
        .into_iter()
        .find(|path| path.is_file())
        .ok_or("profile schema is missing")?;
    let validator = profiles::schema_validator(&schema).map_err(|error| error.to_string())?;
    let parsed = feed::parse_feed(&text, &validator)?;
    Ok(feed::diff_feed(&local, &parsed, env!("CARGO_PKG_VERSION")))
}

#[tauri::command]
fn check_profile_feed(app: tauri::AppHandle) -> Result<FeedCheckView, String> {
    load_all(&app)?; // the store must still load before reporting any status
    match feed_diff(&app) {
        Ok(diff) => {
            let status = if diff.updates.is_empty() && diff.additions.is_empty() {
                "up_to_date"
            } else {
                "updates"
            };
            Ok(FeedCheckView {
                status: status.into(),
                reason: String::new(),
                updates: diff.updates,
                additions: diff.additions.iter().map(|p| p.id.clone()).collect(),
                app_update: diff.app_update,
            })
        }
        Err(reason) => Ok(FeedCheckView {
            status: "error".into(),
            reason,
            updates: Vec::new(),
            additions: Vec::new(),
            app_update: None,
        }),
    }
}

#[tauri::command]
fn apply_profile_feed(app: tauri::AppHandle) -> Result<feed::ApplyReport, String> {
    let local = load_all(&app)?;
    let url = feed::feed_url();
    let text = feed::fetch_text(&url)?;
    let schema = schema_candidates(&app)
        .into_iter()
        .find(|path| path.is_file())
        .ok_or("profile schema is missing")?;
    let validator = profiles::schema_validator(&schema).map_err(|error| error.to_string())?;
    let parsed = feed::parse_feed(&text, &validator)?;
    feed::apply_feed(&store::profile_store_dir(), &parsed, &local)
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}
```

(Drop the unused `local` binding in `check_profile_feed` if the compiler flags it; `load_all` there verifies the store still loads before reporting status.)

5. Register `check_profile_feed`, `apply_profile_feed`, `app_version` in `tauri::generate_handler![...]`.
6. `src-tauri/tauri.conf.json`: inside `bundle`, add `"resources": ["profiles/*", "specs/*"]` (keep existing keys).
7. `src-tauri/capabilities/default.json`: add `"opener:allow-open-url"` to `permissions`.

- [ ] **Step 5: Run everything**

Run: `cargo test` (from `src-tauri`)
Expected: PASS — all suites including the new fetch test.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/src/feed.rs src-tauri/tauri.conf.json src-tauri/capabilities/default.json src-tauri/tests/feed.rs
git commit -m "Wire the feed check and apply commands onto the profile store."
```

---

### Task 6: Frontend — feed status, update button, version banner

**Files:**
- Modify: `src/App.tsx`
- Modify: `package.json` only if `@tauri-apps/plugin-opener` is missing from dependencies (then `npm install @tauri-apps/plugin-opener`).

**Interfaces:**
- Consumes: commands `check_profile_feed`, `apply_profile_feed`, `app_version`, `list_profiles` (Task 5).
- Produces: the UI states named in the spec.

- [ ] **Step 1: Add types and state** — in `src/App.tsx`, below `type CopyReport`:

```ts
type ProfileChange = { id: string; name: string; localVersion: number; remoteVersion: number };

type FeedCheck = {
  status: "up_to_date" | "updates" | "error";
  reason: string;
  updates: ProfileChange[];
  additions: string[];
  appUpdate: { version: string; releaseUrl: string } | null;
};

type ApplyReport = { applied: number; skipped: number };
```

Extend the `Profile` type with `version?: number;`. Add state next to the existing hooks:

```ts
const [feedCheck, setFeedCheck] = useState<FeedCheck | null>(null);
const [appVersion, setAppVersion] = useState("");
const [applyingFeed, setApplyingFeed] = useState(false);
const [feedMessage, setFeedMessage] = useState("");
```

- [ ] **Step 2: Add the check/apply functions** — reuse the existing error/busy conventions:

```ts
async function checkFeed(silent = false) {
  if (!silent) setBusy(true);
  try {
    setFeedCheck(await invoke<FeedCheck>("check_profile_feed"));
    setFeedMessage("");
  } catch (cause) {
    setFeedCheck({
      status: "error",
      reason: String(cause),
      updates: [],
      additions: [],
      appUpdate: null,
    });
  } finally {
    if (!silent) setBusy(false);
  }
}

async function applyFeed() {
  setApplyingFeed(true);
  setError("");
  try {
    const report = await invoke<ApplyReport>("apply_profile_feed");
    setFeedMessage(`Updated ${report.applied} profile${report.applied === 1 ? "" : "s"}.`);
    setProfiles(await invoke<Profile[]>("list_profiles"));
    await checkFeed(true);
  } catch (cause) {
    setError(String(cause));
  } finally {
    setApplyingFeed(false);
  }
}
```

Startup effects (one silent check, one version fetch):

```ts
useEffect(() => {
  void checkFeed(true);
}, []);

useEffect(() => {
  invoke<string>("app_version")
    .then(setAppVersion)
    .catch(() => setAppVersion(""));
}, []);
```

- [ ] **Step 3: Render the block** — inside the `step === "profile"` section, before `<div className="cards">`:

```tsx
{feedCheck ? (
  <div className="feed">
    {feedCheck.status === "up_to_date" ? <p>Profiles are up to date.</p> : null}
    {feedCheck.status === "updates" ? (
      <p>
        {feedCheck.updates.length + feedCheck.additions.length} profile update
        {feedCheck.updates.length + feedCheck.additions.length === 1 ? "" : "s"} available.
      </p>
    ) : null}
    {feedCheck.status === "error" ? (
      <p className="error">Could not check for updates: {feedCheck.reason}</p>
    ) : null}
    {feedMessage ? <p>{feedMessage}</p> : null}
    <div className="row">
      <button disabled={busy || applyingFeed} onClick={() => void checkFeed()}>
        Check again
      </button>
      {feedCheck.status === "updates" ? (
        <button disabled={busy || applyingFeed} onClick={() => void applyFeed()}>
          Update profiles
        </button>
      ) : null}
    </div>
  </div>
) : null}
```

In the header, after the legal paragraph:

```tsx
{appVersion ? <p className="path">Card Studio {appVersion}</p> : null}
{feedCheck?.appUpdate ? (
  <p>
    Card Studio {feedCheck.appUpdate.version} is available.
    <button onClick={() => void openUrl(feedCheck.appUpdate?.releaseUrl ?? "")}>
      Open the releases page
    </button>
  </p>
) : null}
```

with `import { openUrl } from "@tauri-apps/plugin-opener";` at the top (install the package first if missing).

- [ ] **Step 4: Check the build**

Run: `npm run test` then `npm run build`
Expected: tsc + ESLint pass, Vite builds.

- [ ] **Step 5: Commit**

```bash
git add src/App.tsx package.json package-lock.json
git commit -m "Show feed status, update button, and app version in the wizard."
```

---

### Task 7: Initial feed, anti-drift test, plan checkbox

**Files:**
- Create: `profiles-feed.json` (repo root)
- Test: `src-tauri/tests/feed.rs`
- Modify: `docs/BUILD_PLAN.md`

**Interfaces:**
- Consumes: `parse_feed`, `schema_validator`, `load_profiles` (Tasks 1, 3).
- Produces: the checked-in feed served from `main` once merged; the drift guard.

- [ ] **Step 1: Generate the feed** — from the repo root:

```bash
node -e "const fs=require('fs'),path=require('path');const dir='profiles';const profiles=fs.readdirSync(dir).filter(f=>f.endsWith('.json')).sort().map(f=>JSON.parse(fs.readFileSync(path.join(dir,f),'utf8')));const feed={feedVersion:1,app:{version:'0.1.0',releaseUrl:'https://github.com/toilmonkey0-cyber/cfw-zero-touch/releases'},profiles:profiles.map(p=>({...p,version:p.version??1}))};fs.writeFileSync('profiles-feed.json',JSON.stringify(feed,null,2)+'\n');console.log('wrote',profiles.length,'profiles');"
```

Expected output: `wrote 6 profiles`.

- [ ] **Step 2: Write the drift test** — append to `src-tauri/tests/feed.rs`:

```rust
#[test]
fn checked_in_feed_matches_shipped_profiles() {
    let root = repo();
    let schema = root.join("specs").join("profile.schema.json");
    let validator = schema_validator(&schema).unwrap();
    let text = fs::read_to_string(root.join("profiles-feed.json"))
        .expect("profiles-feed.json must stay checked in beside profiles/");
    let feed = parse_feed(&text, &validator).unwrap();
    let shipped = cfw_zero_touch_lib::profiles::load_profiles(&root.join("profiles"), &schema).unwrap();
    assert_eq!(feed.profiles.len(), shipped.len(), "feed and profiles/ disagree on count");
    for profile in &shipped {
        let remote = feed
            .profiles
            .iter()
            .find(|candidate| candidate.id == profile.id)
            .unwrap_or_else(|| panic!("{} is missing from profiles-feed.json", profile.id));
        assert_eq!(remote, profile, "{} differs between feed and profiles/", profile.id);
        assert!(remote.version >= 1);
    }
    assert!(!feed.app.version.is_empty());
}
```

- [ ] **Step 3: Run everything**

Run: `cargo test` and `npm run test`
Expected: PASS, including the drift test.

- [ ] **Step 4: Update the plan document** — in `docs/BUILD_PLAN.md` Phase 4, mark the profile pack + update checker line done with a one-line note naming the feed file, the runtime store, and the deferred items (igir, DTB dropped, diagnostics log).

- [ ] **Step 5: Commit**

```bash
git add profiles-feed.json src-tauri/tests/feed.rs docs/BUILD_PLAN.md
git commit -m "Ship the initial profile feed with an anti-drift test."
```

---

### Task 8: End-to-end verification in the dev app

**Files:**
- No product files. Scratch fixtures under `%TEMP%`.

**Interfaces:**
- Consumes: the full running app (Tasks 5–7).

- [ ] **Step 1: Serve a fixture feed** — create `%TEMP%\cfw-feed-qa\profiles-feed.json` = a copy of the checked-in feed with one profile's `version` bumped to 2 and `app.version` set to `"0.2.0"`. Serve it:

```bash
node -e "const http=require('http'),fs=require('fs');http.createServer((req,res)=>{res.setHeader('content-type','application/json');res.end(fs.readFileSync(process.argv[1]))}).listen(8787)" "$env:TEMP\cfw-feed-qa\profiles-feed.json"
```

(background task; stop it after verification)

- [ ] **Step 2: Launch the app pointed at the fixture** — with a scratch store so nothing touches the real `%LOCALAPPDATA%`:

```bash
$env:CFW_FEED_URL='http://127.0.0.1:8787/feed.json'; $env:CFW_STUDIO_DATA="$env:TEMP\cfw-feed-qa\store"; npm run tauri dev
```

(background task)

- [ ] **Step 3: Drive with Orca computer-use** — verify, on the profile screen:
  1. "1 profile update available." renders with an Update button; the "Card Studio 0.2.0 is available" line renders.
  2. Click Update profiles → "Updated 1 profile." renders, the profile list re-renders from the store, and a second check reports "Profiles are up to date."
  3. `%TEMP%\cfw-feed-qa\store\profiles` contains the bumped file (read it back).
  4. Stop the app, relaunch with `CFW_FEED_URL='http://127.0.0.1:9/feed.json'` → "Could not check for updates:" renders and the six profiles still list.
  5. Regression sweep: enter the dArkOS profile → flash step renders; "The card already booted" path with the real card still reaches the library step (firstboot gate intact).
- [ ] **Step 4: Clean up** — kill the fixture server and dev app, delete `%TEMP%\cfw-feed-qa` and any scratch state files. Leave no background tasks.
- [ ] **Step 5: Commit** any fixes that verification forced, as their own commits.

---

## Post-build (user actions, not code)

- Flip `toilmonkey0-cyber/cfw-zero-touch` to public in GitHub settings; then verify `https://raw.githubusercontent.com/toilmonkey0-cyber/cfw-zero-touch/main/profiles-feed.json` returns 200. Until the repo is public (or `phase-0-1` is merged to `main`), the default URL will 404 and the app shows the soft error state by design.
- Decide when to merge `phase-0-1` → `main` (the feed URL targets `main`).
