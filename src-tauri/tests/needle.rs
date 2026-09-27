use std::fs;
use std::path::{Path, PathBuf};

use cfw_zero_touch_lib::flash;
use cfw_zero_touch_lib::needle::acquire;
use cfw_zero_touch_lib::needle::embed_client::{self, EmbedHelper};
use cfw_zero_touch_lib::needle::manifest::{self, ArtifactRole};

fn scratch(name: &str) -> PathBuf {
    // Unique even when two parallel test threads scratch the same name
    // within one nanosecond (they share the pid).
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cfw-needle-{name}-{}-{}-{seq}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn sha256_of(bytes: &[u8]) -> String {
    flash::sha256_bytes(bytes)
}

/// Fixture fetch that writes the given bytes to the destination .partial.
fn fetch_writer(bytes: &'static [u8]) -> impl FnMut(&Path) -> Result<(), String> {
    move |dest| {
        let parent = dest.parent().unwrap();
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        fs::write(dest, bytes).map_err(|e| e.to_string())
    }
}

#[test]
fn manifest_pins_are_complete_wellformed_and_pinned_to_a_revision() {
    assert_eq!(manifest::NEEDLE_ARTIFACTS.len(), 5);

    let runtime: Vec<_> = manifest::NEEDLE_ARTIFACTS
        .iter()
        .filter(|a| a.role == ArtifactRole::RuntimeDownload)
        .collect();
    assert_eq!(
        runtime.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec!["weights", "serve-engine", "embed-helper"],
        "exactly the two runtime downloads, in a stable order"
    );
    let build: Vec<_> = manifest::NEEDLE_ARTIFACTS
        .iter()
        .filter(|a| a.role == ArtifactRole::BuildInput)
        .collect();
    assert_eq!(
        build.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec!["embed-lib", "embed-header"]
    );

    for artifact in &manifest::NEEDLE_ARTIFACTS {
        assert!(
            artifact.url.starts_with("https://"),
            "{} must use https, got {}",
            artifact.id,
            artifact.url
        );
        let sha = artifact.sha256;
        assert_eq!(sha.len(), 64, "{} sha256 must be 64 hex chars", artifact.id);
        assert!(
            sha.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{} sha256 must be lowercase hex",
            artifact.id
        );
        assert!(artifact.size > 0, "{} must record a size", artifact.id);
        let from_url = artifact.url.rsplit('/').next().unwrap();
        assert_eq!(
            from_url, artifact.file_name,
            "{} file name must match the URL's last segment",
            artifact.id
        );
    }

    // Sizes were verified against the pinned revision when the manifest was
    // written; needle3.cact is 35,335,380 bytes.
    let weights = manifest::find("weights").unwrap();
    assert_eq!(weights.size, 35_335_380);
    // The revision the pins were taken from is recorded for traceability.
    assert_eq!(manifest::NEEDLE_PIN_REVISION.len(), 40);
}

#[test]
fn runtime_artifact_ids_are_validated() {
    assert_eq!(
        acquire::runtime_artifact("weights").unwrap().role,
        ArtifactRole::RuntimeDownload
    );
    assert_eq!(
        acquire::runtime_artifact("serve-engine").unwrap().file_name,
        "needle.exe"
    );
    assert_eq!(
        acquire::runtime_artifact("embed-helper").unwrap().file_name,
        "cfw-embed.exe",
        "our CI-built helper is a pinned runtime download"
    );

    let build = acquire::runtime_artifact("embed-lib").unwrap_err();
    assert!(build.contains("build-time"), "got: {build}");
    let unknown = acquire::runtime_artifact("bogus").unwrap_err();
    assert!(unknown.contains("unknown"), "got: {unknown}");
    // The id reaches us from IPC; traversal junk must die as unknown ids.
    assert!(acquire::runtime_artifact("../evil").is_err());
    assert!(acquire::runtime_artifact("").is_err());
}

#[test]
fn ensure_in_downloads_verifies_and_renames() {
    let dirs = vec![scratch("acquire-ok")];
    let bytes: &'static [u8] = b"needle-weights-fixture";
    let sha = sha256_of(bytes);

    let path = acquire::ensure_in(&dirs, &sha, "needle3.cact", fetch_writer(bytes))
        .expect("verified download should succeed");

    assert_eq!(path, dirs[0].join("needle3.cact"));
    assert!(path.is_file());
    assert!(
        !dirs[0].join("needle3.cact.partial").exists(),
        "no partial left behind"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn ensure_in_discards_mismatched_download() {
    let dirs = vec![scratch("acquire-bad")];
    let expected = sha256_of(b"the pinned bytes");
    let served: &'static [u8] = b"hostile or corrupted bytes";

    let err = acquire::ensure_in(&dirs, &expected, "needle3.cact", fetch_writer(served))
        .expect_err("checksum mismatch must fail");

    assert!(err.contains("checksum"), "got: {err}");
    assert!(
        !dirs[0].join("needle3.cact.partial").exists(),
        "partial discarded"
    );
    assert!(!dirs[0].join("needle3.cact").exists(), "no final artifact");
}

#[test]
fn ensure_in_uses_cached_verified_file_without_fetching() {
    let dirs = vec![
        scratch("acquire-cache-primary"),
        scratch("acquire-cache-secondary"),
    ];
    let bytes: &'static [u8] = b"cached weights";
    let sha = sha256_of(bytes);
    // Verified copy sits in the SECOND cache directory (store fallback).
    fs::write(dirs[1].join("needle3.cact"), bytes).unwrap();

    let mut fetched = false;
    let path = acquire::ensure_in(&dirs, &sha, "needle3.cact", |_| {
        fetched = true;
        Err("must not fetch when a verified copy is cached".into())
    })
    .expect("cache hit should satisfy the acquire");

    assert_eq!(path, dirs[1].join("needle3.cact"));
    assert!(!fetched);
}

#[test]
fn ensure_in_redownloads_when_cached_copy_is_corrupt() {
    let dirs = vec![scratch("acquire-corrupt-cache")];
    let bytes: &'static [u8] = b"good bytes";
    let sha = sha256_of(bytes);
    // A file with the right name but wrong bytes occupies the cache.
    fs::write(dirs[0].join("needle3.cact"), b"corrupted").unwrap();

    let path = acquire::ensure_in(&dirs, &sha, "needle3.cact", fetch_writer(bytes))
        .expect("corrupt cache should be replaced by a verified download");

    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn status_for_reports_presence_and_verification() {
    let dirs = vec![scratch("status")];
    // Empty cache: nothing present, including build inputs.
    let empty = acquire::status_for(&dirs, &manifest::NEEDLE_ARTIFACTS);
    assert_eq!(empty.len(), 5);
    for row in &empty {
        assert!(
            !row.present,
            "empty cache reports nothing present: {:?}",
            row.id
        );
        assert!(!row.verified);
        assert!(row.path.is_none());
    }

    // Fixture pins so real bytes can be "verified" offline.
    let weights_bytes: &'static [u8] = b"real weights bytes";
    let engine_bytes: &'static [u8] = b"real engine bytes";
    let fixtures = [
        manifest::Artifact {
            id: "weights",
            role: manifest::ArtifactRole::RuntimeDownload,
            url: "https://example.invalid/needle3.cact",
            sha256: Box::leak(sha256_of(weights_bytes).into_boxed_str()),
            size: weights_bytes.len() as u64,
            file_name: "needle3.cact",
            purpose: "fixture",
        },
        manifest::Artifact {
            id: "serve-engine",
            role: manifest::ArtifactRole::RuntimeDownload,
            url: "https://example.invalid/needle.exe",
            sha256: Box::leak(sha256_of(engine_bytes).into_boxed_str()),
            size: engine_bytes.len() as u64,
            file_name: "needle.exe",
            purpose: "fixture",
        },
    ];
    fs::write(dirs[0].join("needle3.cact"), weights_bytes).unwrap();
    fs::write(dirs[0].join("needle.exe"), b"not the pinned engine").unwrap();

    let status = acquire::status_for(&dirs, &fixtures);
    assert_eq!(status.len(), 2);
    let by_id = |id: &str| status.iter().find(|r| r.id == id).unwrap();
    let w = by_id("weights");
    assert!(w.present && w.verified, "matching file is verified");
    assert!(w.path.as_deref().unwrap().ends_with("needle3.cact"));
    let e = by_id("serve-engine");
    assert!(e.present, "file with the right name is present");
    assert!(!e.verified, "wrong bytes are not verified");
}

#[test]
fn stems_are_validated_before_reaching_the_helper() {
    assert!(embed_client::validate_stem("Final Fantasy VII (USA)").is_ok());
    assert!(embed_client::validate_stem("  spaces  ").is_ok());
    assert!(embed_client::validate_stem("").is_err());
    assert!(embed_client::validate_stem("line\nbreak").is_err());
    assert!(embed_client::validate_stem("tab\there").is_err());
    assert!(embed_client::validate_stem("nul\0byte").is_err());
    assert!(embed_client::validate_stem(&"x".repeat(embed_client::MAX_STEM_BYTES + 1)).is_err());
    assert!(embed_client::validate_stem(&"x".repeat(embed_client::MAX_STEM_BYTES)).is_ok());
}

#[test]
fn float_lines_parse_and_bad_input_is_rejected() {
    let parsed = embed_client::parse_floats("0.5 -0.25 1.00000004e-3").unwrap();
    assert_eq!(parsed, vec![0.5, -0.25, 1e-3]);
    assert!(embed_client::parse_floats("").unwrap().is_empty());
    assert!(embed_client::parse_floats("0.5 nan-inf").is_err());
    assert!(embed_client::parse_floats("not-a-float").is_err());
}

/// Real-helper integration test, run only when CFW_NEEDLE_TEST=1 and the
/// helper + weights are provided:
///   CFW_NEEDLE_TEST=1
///   CFW_NEEDLE_EMBED_EXE=...\cfw-embed.exe
///   CFW_NEEDLE_WEIGHTS=...\needle3.cact
/// Mirrors the CFW_IGIR_TEST pattern: offline CI skips this entirely.
#[test]
fn real_helper_embeds_deterministically_within_budget() {
    if std::env::var("CFW_NEEDLE_TEST").ok().as_deref() != Some("1") {
        eprintln!("skipping: CFW_NEEDLE_TEST not set");
        return;
    }
    let exe = std::env::var("CFW_NEEDLE_EMBED_EXE").expect("CFW_NEEDLE_EMBED_EXE required");
    let weights = std::env::var("CFW_NEEDLE_WEIGHTS").expect("CFW_NEEDLE_WEIGHTS required");
    let exe = PathBuf::from(exe);
    let weights = PathBuf::from(weights);
    assert!(exe.is_file(), "helper exe missing: {}", exe.display());
    assert!(weights.is_file(), "weights missing: {}", weights.display());

    let mut helper = EmbedHelper::spawn(&exe, &weights).expect("helper should spawn");
    let dim = helper.ping().expect("ping");
    assert_eq!(dim, 3072, "weights report the expected dimension");

    let a1 = helper.embed("Final Fantasy VII (USA)").expect("embed 1");
    let a2 = helper.embed("Final Fantasy VII (USA)").expect("embed 2");
    assert_eq!(a1.len(), dim);
    assert_eq!(
        a1, a2,
        "embedding must be deterministic (copy-time guard depends on it)"
    );

    let b = helper.embed("Panzer Dragoon Saga (USA)").expect("embed 3");
    assert_ne!(a1, b, "different titles must embed differently");

    let many = helper
        .embed_many(&["Chrono Cross", "Nights into Dreams"])
        .expect("embed_many");
    assert_eq!(many.len(), 2);
    assert!(many.iter().all(|v| v.len() == dim));

    let start = std::time::Instant::now();
    for i in 0..20 {
        helper
            .embed(&format!("latency probe {i}"))
            .expect("latency embed");
    }
    let per = start.elapsed() / 20;
    assert!(
        per < std::time::Duration::from_millis(250),
        "embed too slow: {per:?}"
    );

    helper.kill();
}

// ------------------------------------------------------------------
// PR 2b: library index (offline, synthetic vectors through EmbedFn)
// ------------------------------------------------------------------
use cfw_zero_touch_lib::needle::index::{self, DeterministicRouter, LibraryIndex};
use cfw_zero_touch_lib::needle::tags;
use cfw_zero_touch_lib::profiles::SystemFolder;

fn system(id: &str, folder: &str, extensions: &[&str]) -> SystemFolder {
    SystemFolder {
        id: id.into(),
        folder: folder.into(),
        extensions: extensions.iter().map(|e| e.to_string()).collect(),
        dat_name_pattern: None,
    }
}

fn library_fixture(name: &str) -> PathBuf {
    let root = scratch(name);
    // Organized: routed by folder.
    fs::create_dir_all(root.join("gba")).unwrap();
    fs::write(root.join("gba/Advance Wars (U).gba"), b"gba-rom").unwrap();
    fs::write(root.join("gba/Golden Sun (USA).gba"), b"gba-rom").unwrap();
    fs::create_dir_all(root.join("nes")).unwrap();
    fs::write(root.join("nes/Zelda.nes"), b"nes-rom").unwrap();
    // Ambiguous: .zip claimed by gba and nes; routes nowhere.
    fs::write(root.join("Mystery Game (Europe).zip"), b"zip").unwrap();
    root
}

/// Deterministic synthetic embedding: a 4-dim vector keyed by the stem's
/// first token, so identical stems embed identically and distinct stems
/// differ. Enough for index semantics; real vectors come from cfw-embed.
fn synthetic_embed(stem: &str) -> Result<Vec<f32>, String> {
    let mut v = vec![0.0f32; 4];
    let key = stem.chars().next().unwrap_or('?') as usize;
    v[key % 4] = 1.0;
    if stem.starts_with("advance") {
        v[1] = 0.5;
    }
    Ok(v)
}

#[test]
fn router_matches_folders_case_insensitively_and_ids() {
    let systems = vec![
        system("gba", "GBA", &[".gba"]),
        system("nes", "Nintendo", &[".nes"]),
    ];
    let router = DeterministicRouter::from_systems(&systems);
    assert_eq!(router.route(Path::new("gba/Game.gba")), Some("gba"));
    assert_eq!(router.route(Path::new("GBA/Game.gba")), Some("gba"));
    assert_eq!(
        router.route(Path::new("Nintendo/Zelda.nes")),
        Some("nes"),
        "system.id also names a folder"
    );
}

#[test]
fn router_routes_extensions_only_with_a_unique_claimant() {
    let systems = vec![
        system("gba", "gba", &[".gba", ".zip"]),
        system("nes", "nes", &[".nes", ".zip"]),
        system("snes", "snes", &[".sfc"]),
    ];
    let router = DeterministicRouter::from_systems(&systems);
    assert_eq!(
        router.route(Path::new("loose/Game.sfc")),
        Some("snes"),
        "unique extension routes"
    );
    assert_eq!(
        router.route(Path::new("loose/Game.zip")),
        None,
        "ambiguous extension does not"
    );
    assert_eq!(
        router.route(Path::new("loose/Game.unk")),
        None,
        "unknown extension does not"
    );
}

#[test]
fn bin_extension_differs_across_shipped_profile_shapes() {
    // The design's motivating case: .bin is psx-unique in r35s-stock-card
    // but claimed by psx AND megadrive in r36s-clone-card.
    let r35s = vec![
        system("psx", "psx", &[".bin", ".cue", ".chd"]),
        system("snes", "snes", &[".sfc", ".smc"]),
    ];
    let r36s = vec![
        system("psx", "psx", &[".bin", ".cue", ".chd"]),
        system("megadrive", "megadrive", &[".bin", ".md", ".gen"]),
    ];
    let r35s_router = DeterministicRouter::from_systems(&r35s);
    let r36s_router = DeterministicRouter::from_systems(&r36s);
    assert_eq!(r35s_router.route(Path::new("x/Game.bin")), Some("psx"));
    assert_eq!(r36s_router.route(Path::new("x/Game.bin")), None);
    assert_ne!(r35s_router.fingerprint(), r36s_router.fingerprint());
}

#[test]
fn index_build_includes_deterministic_files_and_excludes_queries() {
    let root = library_fixture("build");
    let systems = vec![
        system("gba", "gba", &[".gba"]),
        system("nes", "nes", &[".nes"]),
    ];
    // Point the store at scratch so nothing touches the real store.
    let store = scratch("build-store");
    // SAFETY of env: tests within one binary run in parallel; use a
    // unique store per test (env set once below via unsafe is the
    // established pattern in this suite? No � instead give ensure_index
    // an explicit path by monkey-patching? The store root is env-driven,
    // so serialize env-touching tests with a lock.
    let _guard = STORE_ENV_LOCK.lock();
    std::env::set_var("CFW_STUDIO_DATA", &store);
    let (index, rebuilt) = index::ensure_index(
        "test-profile",
        &root,
        &systems,
        None,
        "weights-v1",
        &mut synthetic_embed,
    )
    .unwrap();
    assert!(rebuilt);
    let stems: Vec<&str> = index.entries.iter().map(|e| e.stem.as_str()).collect();
    assert!(
        stems.contains(&"advance wars"),
        "folder-routed gba file indexed: {stems:?}"
    );
    assert!(stems.contains(&"zelda"));
    assert!(
        !stems.iter().any(|s| s.contains("mystery")),
        "ambiguous zip excluded"
    );
    assert!(index.entries.iter().all(|e| e.vector.len() == 4));
    // The persisted file exists with the tmp sibling gone.
    let path = index::index_path("test-profile").unwrap();
    assert!(path.is_file());
    assert!(!path.with_extension("tmp").exists());
    std::env::remove_var("CFW_STUDIO_DATA");
}

static STORE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn reuse_skips_embedding_when_fingerprints_match() {
    let root = library_fixture("reuse");
    let systems = vec![
        system("gba", "gba", &[".gba"]),
        system("nes", "nes", &[".nes"]),
    ];
    let store = scratch("reuse-store");
    let _guard = STORE_ENV_LOCK.lock();
    std::env::set_var("CFW_STUDIO_DATA", &store);

    let calls = std::cell::Cell::new(0usize);
    let mut counting = |stem: &str| -> Result<Vec<f32>, String> {
        calls.set(calls.get() + 1);
        synthetic_embed(stem)
    };
    let (_, first) = index::ensure_index("p", &root, &systems, None, "w1", &mut counting).unwrap();
    assert!(first);
    let before = calls.get();
    let (_, second) = index::ensure_index("p", &root, &systems, None, "w1", &mut counting).unwrap();
    assert!(!second, "all fingerprints match: no rebuild");
    assert_eq!(calls.get(), before, "reuse must not embed again");

    // Library change (new file) forces a rebuild.
    fs::write(root.join("gba/New Game.gba"), b"gba").unwrap();
    let (_, third) = index::ensure_index("p", &root, &systems, None, "w1", &mut counting).unwrap();
    assert!(third, "library fingerprint change rebuilds");

    // Weights change forces a rebuild.
    let (_, fourth) = index::ensure_index("p", &root, &systems, None, "w2", &mut counting).unwrap();
    assert!(fourth, "weights fingerprint change rebuilds");
    std::env::remove_var("CFW_STUDIO_DATA");
}

#[test]
fn routing_change_rebuilds_and_profiles_coexist() {
    let root = library_fixture("routing");
    let r35s = vec![
        system("psx", "psx", &[".bin"]),
        system("gba", "gba", &[".gba"]),
    ];
    let r36s = vec![
        system("psx", "psx", &[".bin"]),
        system("megadrive", "megadrive", &[".bin"]),
    ];
    let store = scratch("routing-store");
    let _guard = STORE_ENV_LOCK.lock();
    std::env::set_var("CFW_STUDIO_DATA", &store);

    let (_, a) = index::ensure_index(
        "r35s-stock-card",
        &root,
        &r35s,
        None,
        "w1",
        &mut synthetic_embed,
    )
    .unwrap();
    assert!(a);
    // Same library, same weights, DIFFERENT routing: rebuild (this is the
    // two-profiles-one-library rule; the second profile also lives at its
    // own path).
    let (_, b) = index::ensure_index(
        "r36s-clone-card",
        &root,
        &r36s,
        None,
        "w1",
        &mut synthetic_embed,
    )
    .unwrap();
    assert!(b, "routing fingerprint differs across profiles");
    let (_, a2) = index::ensure_index(
        "r35s-stock-card",
        &root,
        &r35s,
        None,
        "w1",
        &mut synthetic_embed,
    )
    .unwrap();
    assert!(!a2, "first profile's index is still reusable");
    // Both files coexist under needle/index/.
    assert!(index::index_path("r35s-stock-card").unwrap().is_file());
    assert!(index::index_path("r36s-clone-card").unwrap().is_file());
    std::env::remove_var("CFW_STUDIO_DATA");
}

#[test]
fn persist_roundtrip_preserves_everything() {
    let entries = vec![
        index::IndexEntry {
            system_id: "gba".into(),
            stem: "advance wars".into(),
            vector: vec![1.0, 0.0, 0.5, -0.25],
        },
        index::IndexEntry {
            system_id: "nes".into(),
            stem: "zelda".into(),
            vector: vec![0.0, 1.0, 0.0, 0.0],
        },
    ];
    let original = LibraryIndex {
        dim: 4,
        entries,
        library: [1; 32],
        routing: [2; 32],
        weights: [3; 32],
    };
    let dir = scratch("roundtrip");
    let path = dir.join("idx.bin");
    index::save(&original, &path).unwrap();
    let loaded = index::load(&path).unwrap();
    assert_eq!(loaded, original);
}

#[test]
fn corrupt_index_files_are_errors_never_panics() {
    let dir = scratch("corrupt");
    let good = LibraryIndex {
        dim: 2,
        entries: vec![index::IndexEntry {
            system_id: "gba".into(),
            stem: "x".into(),
            vector: vec![1.0, 0.0],
        }],
        library: [0; 32],
        routing: [0; 32],
        weights: [0; 32],
    };
    let path = dir.join("good.bin");
    index::save(&good, &path).unwrap();
    let bytes = fs::read(&path).unwrap();

    // Truncated at every prefix length: must error, never panic.
    for cut in 0..bytes.len() {
        let truncated = dir.join(format!("t{cut}.bin"));
        fs::write(&truncated, &bytes[..cut]).unwrap();
        assert!(
            index::load(&truncated).is_err(),
            "truncated at {cut} must error"
        );
    }
    // Bad magic.
    let mut magic = bytes.clone();
    magic[0] = b'X';
    let bad = dir.join("magic.bin");
    fs::write(&bad, magic).unwrap();
    assert!(index::load(&bad).is_err());
    // Trailing bytes.
    let mut trailing = bytes.clone();
    trailing.push(0);
    let bad = dir.join("trailing.bin");
    fs::write(&bad, trailing).unwrap();
    assert!(index::load(&bad).is_err());
    // Hostile header: huge count claims.
    let mut hostile = bytes.clone();
    let count_offset = 8 + 4 + 4;
    hostile[count_offset..count_offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    let bad = dir.join("hostile.bin");
    fs::write(&bad, hostile).unwrap();
    assert!(
        index::load(&bad).is_err(),
        "huge count must be rejected before allocation"
    );
}

#[test]
fn profile_ids_are_validated_before_becoming_paths() {
    assert!(index::index_path("r35s-stock-card").is_ok());
    assert!(index::index_path("p1").is_ok());
    assert!(index::index_path("").is_err());
    assert!(index::index_path("../evil").is_err());
    assert!(index::index_path("UPPER").is_err());
    assert!(index::index_path("a/b").is_err());
    assert!(index::index_path(&"x".repeat(65)).is_err());
}

#[test]
fn query_returns_nearest_by_cosine() {
    let index = LibraryIndex {
        dim: 4,
        entries: vec![
            index::IndexEntry {
                system_id: "gba".into(),
                stem: "advance wars".into(),
                vector: vec![1.0, 0.0, 0.0, 0.0],
            },
            index::IndexEntry {
                system_id: "nes".into(),
                stem: "zelda".into(),
                vector: vec![0.0, 1.0, 0.0, 0.0],
            },
        ],
        library: [0; 32],
        routing: [0; 32],
        weights: [0; 32],
    };
    let (i, sim) = index.query(&[0.9, 0.1, 0.0, 0.0]).unwrap();
    assert_eq!(index.entries[i].stem, "advance wars");
    assert!(sim > 0.99, "near-identical vector: {sim}");
    let (i, sim) = index.query(&[0.0, 1.0, 0.0, 0.0]).unwrap();
    assert_eq!(index.entries[i].stem, "zelda");
    assert!((sim - 1.0).abs() < 1e-9);
    // Wrong-dimension queries refuse rather than misrank.
    assert!(index.query(&[1.0, 0.0]).is_none());
}

#[test]
fn clean_stem_matches_spike_fixtures() {
    assert_eq!(
        tags::clean_stem("Dragon Spirit_[Euro]_disc3.img"),
        "dragon spirit"
    );
    assert_eq!(tags::clean_stem("sega-saturn/Wipeout (PAL).gdi"), "wipeout");
}

// ------------------------------------------------------------------
// PR 4: smart-sort planner (offline, injected embeddings)
// ------------------------------------------------------------------
use cfw_zero_touch_lib::needle::sort;

/// Controlled embedding: stems map to hand-picked vectors so similarity
/// is exact � "clone wars" sits at ~0.999 vs "advance wars" (auto-route),
/// "mystery game" at 0.8 (review with a suggestion), everything else
/// defaults to a diagonal-ish vector far from all indexed stems.
fn controlled_embed(stem: &str) -> Result<Vec<f32>, String> {
    match stem {
        "advance wars" => Ok(vec![1.0, 0.0, 0.0, 0.0]),
        "golden sun" => Ok(vec![0.0, 1.0, 0.0, 0.0]),
        "zelda" => Ok(vec![0.0, 0.0, 1.0, 0.0]),
        "mystery game" => Ok(vec![0.8, 0.0, 0.6, 0.0]),
        "clone wars" => Ok(vec![0.999, 0.0447, 0.0, 0.0]),
        "astrob" => Ok(vec![0.999, 0.0447, 0.0, 0.0]),
        _ => Ok(vec![0.25, 0.25, 0.25, 0.25]),
    }
}

fn smart_library_fixture(name: &str) -> PathBuf {
    let root = scratch(name);
    for (sub, file) in [
        ("gba", "Advance Wars (U).gba"),
        ("gba", "Advance Wars (Europe).gba"),
        ("gba", "Golden Sun (USA).gba"),
        ("gba", "Final Fantasy (USA) (Disc 1).gba"),
        ("gba", "Final Fantasy (USA) (Disc 2).gba"),
        ("gba", "Final Fantasy (Japan) (Disc 1).gba"),
        ("nes", "Zelda.nes"),
    ] {
        fs::create_dir_all(root.join(sub)).unwrap();
        fs::write(root.join(sub).join(file), b"rom-bytes").unwrap();
    }
    fs::create_dir_all(root.join("loose")).unwrap();
    fs::write(root.join("loose/Mystery Game (Japan).zip"), b"zip").unwrap();
    fs::write(root.join("loose/Clone Wars.zip"), b"zip").unwrap();
    fs::write(root.join("loose/notes.txt"), b"not a rom").unwrap();
    root
}

fn smart_systems() -> Vec<SystemFolder> {
    vec![
        system("gba", "gba", &[".gba", ".zip"]),
        system("nes", "nes", &[".nes", ".zip"]),
    ]
}

fn classify_fixture(name: &str) -> sort::Classification {
    let root = smart_library_fixture(name);
    let store = scratch(&format!("{name}-store"));
    let _guard = STORE_ENV_LOCK.lock();
    std::env::set_var("CFW_STUDIO_DATA", &store);
    let systems = smart_systems();
    let mut embed = controlled_embed;
    let (index, _rebuilt) =
        index::ensure_index("test", &root, &systems, None, "w1", &mut embed).unwrap();
    let mut progress = |_, _| {};
    let classification = sort::classify(
        &root,
        &systems,
        &[],
        None,
        &index,
        false,
        &[tags::Region::Usa],
        &mut embed,
        &mut progress,
    )
    .unwrap();
    std::env::remove_var("CFW_STUDIO_DATA");
    classification
}

#[test]
fn bios_folder_payload_never_routes_as_games() {
    // The library bios tree is card bios payload, never game content:
    // sample packs are NAMED after games (astrob.zip scores ~0.999 vs
    // "advance wars") and .sms bios blobs own a unique extension, so
    // without the exclusion tier 2/3 happily scatter them into system
    // folders as fake games (found live on the R36S card QA).
    let root = scratch("bios-routing");
    for (sub, file) in [
        ("gba", "Advance Wars (U).gba"),
        ("nes", "Zelda.nes"),
        ("mastersystem", "Alex Kidd (USA).sms"),
        ("bios/mame2003-plus/samples", "astrob.zip"),
        ("bios", "bios_E.sms"),
    ] {
        fs::create_dir_all(root.join(sub)).unwrap();
        fs::write(root.join(sub).join(file), b"bytes").unwrap();
    }
    let systems = vec![
        system("gba", "gba", &[".gba", ".zip"]),
        system("nes", "nes", &[".nes", ".zip"]),
        system("mastersystem", "mastersystem", &[".sms", ".zip"]),
    ];
    let store = scratch("bios-routing-store");
    let _guard = STORE_ENV_LOCK.lock();
    std::env::set_var("CFW_STUDIO_DATA", &store);
    let mut embed = controlled_embed;
    let (index, _rebuilt) =
        index::ensure_index("bios", &root, &systems, None, "w1", &mut embed).unwrap();
    let mut progress = |_, _| {};
    let classification = sort::classify(
        &root,
        &systems,
        &[],
        Some("bios"),
        &index,
        false,
        &[tags::Region::Usa],
        &mut embed,
        &mut progress,
    )
    .unwrap();
    std::env::remove_var("CFW_STUDIO_DATA");

    let touched = |rows: &[String]| {
        rows.iter()
            .filter(|relative| relative.starts_with("bios/"))
            .count()
    };
    let routed: Vec<String> = classification
        .routes
        .iter()
        .map(|row| row.relative.clone())
        .collect();
    let reviewed: Vec<String> = classification
        .needs_review
        .iter()
        .map(|row| row.relative.clone())
        .collect();
    assert_eq!(touched(&routed), 0, "no bios file may route: {routed:?}");
    assert_eq!(
        touched(&reviewed),
        0,
        "no bios file may review: {reviewed:?}"
    );
    assert!(routed.contains(&"mastersystem/Alex Kidd (USA).sms".to_string()));

    // The planner still ships the bios tree to the card's bios folder.
    let applied = std::collections::HashMap::new();
    let dest_root = scratch("bios-routing-dest");
    let plan = sort::plan_smart(
        &root,
        &dest_root,
        "arkos_easyroms_root",
        &systems,
        &classification,
        &applied,
        Some("bios"),
    )
    .unwrap();
    let dests: Vec<&str> = plan
        .items
        .iter()
        .map(|item| item.relative_dest.as_str())
        .collect();
    assert!(
        dests.contains(&"bios/bios_E.sms"),
        "bios payload still planned: {dests:?}"
    );
    assert!(
        dests.contains(&"bios/mame2003-plus/samples/astrob.zip"),
        "sample packs still planned: {dests:?}"
    );
}

#[test]
fn classify_uses_all_three_tiers_and_flags_cold_start() {
    let classification = classify_fixture("tiers");
    let route = |relative: &str| {
        classification
            .routes
            .iter()
            .find(|row| row.relative == relative)
            .unwrap_or_else(|| panic!("no route row for {relative}: {:?}", classification.routes))
    };

    let zelda = route("nes/Zelda.nes");
    assert_eq!(
        (zelda.system_id.as_str(), zelda.tier),
        ("nes", sort::Tier::Folder)
    );

    let clone = route("loose/Clone Wars.zip");
    assert_eq!(
        (clone.system_id.as_str(), clone.tier),
        ("gba", sort::Tier::Embedding)
    );

    // Unknown extensions never enter the classification.
    assert!(classification
        .routes
        .iter()
        .all(|row| row.relative != "loose/notes.txt"));
    assert!(classification
        .needs_review
        .iter()
        .all(|row| row.relative != "loose/notes.txt"));

    // Mystery lands in review with the nearest title + similarity.
    assert_eq!(classification.needs_review.len(), 1);
    let review = &classification.needs_review[0];
    assert_eq!(review.review_id, "r1");
    assert_eq!(review.nearest_stem.as_deref(), Some("advance wars"));
    assert!(review.similarity.unwrap() > 0.79 && review.similarity.unwrap() < 0.81);

    // Seven indexed files (one per deterministically routed file) is
    // below COLD_START_TITLES: honest degradation flag.
    assert!(classification.cold_start);
    assert_eq!(classification.index_stats.titles, 7);
}

#[test]
fn variant_collapse_keeps_preferred_region_and_all_its_discs() {
    let classification = classify_fixture("collapse");
    let decision = |relative: &str| {
        classification
            .routes
            .iter()
            .find(|row| row.relative == relative)
            .unwrap_or_else(|| panic!("missing {relative}"))
            .variant
            .clone()
    };

    // USA preference: keep (U), skip the Europe variant.
    assert_eq!(
        decision("gba/Advance Wars (U).gba"),
        sort::VariantDecision::Keep
    );
    assert!(matches!(
        decision("gba/Advance Wars (Europe).gba"),
        sort::VariantDecision::SkipRegion { .. }
    ));

    // Multi-disc integrity: both USA discs kept, the Japan disc skipped.
    assert_eq!(
        decision("gba/Final Fantasy (USA) (Disc 1).gba"),
        sort::VariantDecision::Keep
    );
    assert_eq!(
        decision("gba/Final Fantasy (USA) (Disc 2).gba"),
        sort::VariantDecision::Keep
    );
    assert!(matches!(
        decision("gba/Final Fantasy (Japan) (Disc 1).gba"),
        sort::VariantDecision::SkipRegion { .. }
    ));

    // The auto-routed clone wars keeps (its group has one member).
    assert_eq!(
        decision("loose/Clone Wars.zip"),
        sort::VariantDecision::Keep
    );

    let ff_group = classification
        .groups
        .iter()
        .find(|g| g.stem == "final fantasy")
        .unwrap();
    assert_eq!((ff_group.kept, ff_group.skipped_variants), (2, 1));
}

#[test]
fn duplicates_within_winning_region_collapse_deterministically() {
    let root = scratch("dupes");
    fs::create_dir_all(root.join("gba")).unwrap();
    fs::write(root.join("gba/Game (USA) [!].gba"), b"12345678").unwrap();
    fs::write(root.join("gba/Game (USA).gba"), b"1234567890").unwrap();
    let store = scratch("dupes-store");
    let _guard = STORE_ENV_LOCK.lock();
    std::env::set_var("CFW_STUDIO_DATA", &store);
    let systems = smart_systems();
    let mut embed = controlled_embed;
    let (index, _) = index::ensure_index("dupes", &root, &systems, None, "w1", &mut embed).unwrap();
    let mut progress = |_, _| {};
    let classification = sort::classify(
        &root,
        &systems,
        &[],
        None,
        &index,
        false,
        &[tags::Region::Usa],
        &mut embed,
        &mut progress,
    )
    .unwrap();
    std::env::remove_var("CFW_STUDIO_DATA");

    // Tie-break: shortest raw name wins ("Game (USA).gba").
    let kept = classification
        .routes
        .iter()
        .find(|r| r.variant == sort::VariantDecision::Keep)
        .unwrap();
    assert_eq!(kept.relative, "gba/Game (USA).gba");
    let skipped = classification
        .routes
        .iter()
        .find(|r| matches!(r.variant, sort::VariantDecision::SkipDuplicate { .. }))
        .unwrap();
    assert_eq!(skipped.relative, "gba/Game (USA) [!].gba");
}

#[test]
fn plan_smart_marks_copy_when_the_dest_size_differs() {
    // plan_copy skips only byte-size-equal destinations; the smart
    // planner used to skip on bare existence, hiding stale files.
    let root = smart_library_fixture("stale-dest");
    let store = scratch("stale-dest-store");
    let _guard = STORE_ENV_LOCK.lock();
    std::env::set_var("CFW_STUDIO_DATA", &store);
    let systems = smart_systems();
    let mut embed = controlled_embed;
    let (index, _) = index::ensure_index("stale", &root, &systems, None, "w1", &mut embed).unwrap();
    let mut progress = |_, _| {};
    let classification = sort::classify(
        &root,
        &systems,
        &[],
        None,
        &index,
        false,
        &[tags::Region::Usa],
        &mut embed,
        &mut progress,
    )
    .unwrap();
    std::env::remove_var("CFW_STUDIO_DATA");

    // Skip every review row (loose files) - they are not under test.
    let applied: std::collections::HashMap<String, Option<String>> = classification
        .needs_review
        .iter()
        .map(|row| (row.review_id.clone(), None))
        .collect();

    let dest_root = scratch("stale-dest-card");
    let dest = dest_root.join("roms/gba/Advance Wars (U).gba");
    fs::create_dir_all(dest.parent().unwrap()).unwrap();

    fs::write(&dest, b"stale-bytes").unwrap(); // wrong size
    let plan = sort::plan_smart(
        &root,
        &dest_root,
        "rocknix_roms_nested",
        &systems,
        &classification,
        &applied,
        None,
    )
    .unwrap();
    let item = plan
        .items
        .iter()
        .find(|i| i.relative_dest == "roms/gba/Advance Wars (U).gba")
        .unwrap();
    assert_eq!(item.action, CopyAction::Copy, "stale dest re-copies");

    fs::write(&dest, b"rom-bytes").unwrap(); // fixture files are b"rom-bytes"
    let plan = sort::plan_smart(
        &root,
        &dest_root,
        "rocknix_roms_nested",
        &systems,
        &classification,
        &applied,
        None,
    )
    .unwrap();
    let item = plan
        .items
        .iter()
        .find(|i| i.relative_dest == "roms/gba/Advance Wars (U).gba")
        .unwrap();
    assert_eq!(item.action, CopyAction::SkipUnchanged);
}

#[test]
fn review_ids_are_sequential_and_stable_across_runs() {
    let first = classify_fixture("stable-a");
    let second = classify_fixture("stable-b");
    assert_eq!(
        first.needs_review, second.needs_review,
        "ids + rows identical across identical libraries"
    );
    assert_eq!(first.needs_review[0].review_id, "r1");
}

#[test]
fn verify_resolutions_guards_size_and_validates_systems() {
    let classification = classify_fixture("guard");
    let systems = smart_systems();
    let review = &classification.needs_review[0];

    // Valid choice.
    let mut ok = std::collections::HashMap::new();
    ok.insert(
        review.review_id.clone(),
        sort::Resolution {
            system_id: Some("nes".into()),
            size: review.size,
        },
    );
    let applied = sort::verify_resolutions(&classification, &systems, &ok).unwrap();
    assert_eq!(applied.get("r1"), Some(&Some("nes".to_string())));

    // Library changed since Preview (size differs): refuse.
    let mut stale = std::collections::HashMap::new();
    stale.insert(
        "r1".into(),
        sort::Resolution {
            system_id: Some("nes".into()),
            size: review.size + 1,
        },
    );
    let err = sort::verify_resolutions(&classification, &systems, &stale).unwrap_err();
    assert!(err.contains("preview again"), "got: {err}");

    // Unknown system id is refused (enum-validated).
    let mut hostile = std::collections::HashMap::new();
    hostile.insert(
        "r1".into(),
        sort::Resolution {
            system_id: Some("../evil".into()),
            size: review.size,
        },
    );
    assert!(sort::verify_resolutions(&classification, &systems, &hostile).is_err());

    // Unknown review id is refused.
    let mut ghost = std::collections::HashMap::new();
    ghost.insert(
        "r99".into(),
        sort::Resolution {
            system_id: None,
            size: 1,
        },
    );
    assert!(sort::verify_resolutions(&classification, &systems, &ghost).is_err());
}

#[test]
fn plan_smart_builds_destinations_resolutions_and_bios() {
    let root = smart_library_fixture("plan");
    fs::create_dir_all(root.join("BIOS")).unwrap();
    fs::write(root.join("BIOS/scph.bin"), b"bios").unwrap();
    let store = scratch("plan-store");
    let _guard = STORE_ENV_LOCK.lock();
    std::env::set_var("CFW_STUDIO_DATA", &store);
    let systems = smart_systems();
    let mut embed = controlled_embed;
    let (index, _) = index::ensure_index("plan", &root, &systems, None, "w1", &mut embed).unwrap();
    let mut progress = |_, _| {};
    let classification = sort::classify(
        &root,
        &systems,
        &[],
        None,
        &index,
        false,
        &[tags::Region::Usa],
        &mut embed,
        &mut progress,
    )
    .unwrap();
    std::env::remove_var("CFW_STUDIO_DATA");

    let review = &classification.needs_review[0];
    let mut applied = std::collections::HashMap::new();
    applied.insert(review.review_id.clone(), Some("nes".to_string()));

    let dest_root = scratch("plan-dest");
    let plan = sort::plan_smart(
        &root,
        &dest_root,
        "rocknix_roms_nested",
        &systems,
        &classification,
        &applied,
        Some("bios"),
    )
    .unwrap();

    let dests: Vec<&str> = plan
        .items
        .iter()
        .map(|item| item.relative_dest.as_str())
        .collect();
    // Tier-1 subpath preserved under the mapped prefix.
    assert!(dests.contains(&"roms/nes/Zelda.nes"));
    assert!(dests.contains(&"roms/gba/Final Fantasy (USA) (Disc 1).gba"));
    // Tier-3 loose files land flat; the resolved review follows the user.
    assert!(dests.contains(&"roms/gba/Clone Wars.zip"));
    assert!(dests.contains(&"roms/nes/Mystery Game (Japan).zip"));
    // Skipped variants are excluded.
    assert!(!dests.iter().any(|d| d.contains("Europe")));
    assert!(!dests.iter().any(|d| d.contains("Japan) (Disc 1).gba")));
    // Bios mirrors plan_copy's handling (case-insensitive folder).
    assert!(dests.contains(&"roms/bios/scph.bin"));

    // Unresolved reviews block planning.
    let empty = std::collections::HashMap::new();
    let err = sort::plan_smart(
        &root,
        &dest_root,
        "rocknix_roms_nested",
        &systems,
        &classification,
        &empty,
        None,
    )
    .unwrap_err();
    assert!(err.contains("need review"), "got: {err}");
}

// ------------------------------------------------------------------
// PR 6: serve client protocol (mock-server, no live engine)
// ------------------------------------------------------------------
use cfw_zero_touch_lib::needle::client::{ClientError, ServeClient};
use cfw_zero_touch_lib::needle::serve;

/// A hand-rolled HTTP/1.1 mock server: one scripted responder serving
/// connection after connection until `max_requests`, on an OS-picked
/// port. The responder sees the full request text (headers + body) and
/// returns a `(status, body)` pair, so mocks can refuse as well as answer.
fn mock_server(
    max_requests: usize,
    respond: impl Fn(&str) -> (u16, String) + Send + 'static,
) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        let start = std::time::Instant::now();
        let mut served = 0usize;
        while served < max_requests && start.elapsed() < std::time::Duration::from_secs(20) {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    // Deterministic reads/writes regardless of the
                    // listener's non-blocking accept loop.
                    socket.set_nonblocking(false).ok();
                    // ureq reads responses to completion on the connection,
                    // so wait for the full request bytes (headers, plus any
                    // body) with a stack instead of a single short read.
                    let mut held = Vec::new();
                    let mut complete = false;
                    let start = std::time::Instant::now();
                    while start.elapsed() < std::time::Duration::from_secs(5) && !complete {
                        let mut chunk = [0u8; 4096];
                        match socket.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => {
                                held.extend_from_slice(&chunk[..n]);
                                if let Some(end) = find_headers_end(&held) {
                                    let content_length = content_length_of(&held[..end]);
                                    if held.len() - end >= content_length {
                                        complete = true;
                                    }
                                }
                            }
                            Err(error) => {
                                // On Windows an accepted socket can
                                // inherit the listener's non-blocking
                                // mode: a not-ready read must keep
                                // waiting, dropping the connection
                                // here surfaces client-side as an
                                // abort (10053) on slow machines.
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::TimedOut
                                        | std::io::ErrorKind::Interrupted
                                ) {
                                    std::thread::sleep(std::time::Duration::from_millis(2));
                                    continue;
                                }
                                break;
                            }
                        }
                    }
                    if !complete {
                        continue;
                    }
                    let request = String::from_utf8_lossy(&held).to_string();
                    let (status, body) = respond(&request);
                    let response = format!(
                        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        status_phrase(status),
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes());
                    served += 1;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
    });
    (addr, handle)
}

/// Numeric HTTP status carried alongside a mock answer body, so mocks
/// can refuse (429) as well as answer (200).
pub struct MockAnswer(pub u16, pub String);

fn status_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        429 => "Too Many Requests",
        _ => "Error",
    }
}

/// Byte index just past the first `\r\n\r\n`, if present.
fn find_headers_end(held: &[u8]) -> Option<usize> {
    held.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

/// Case-insensitive `Content-Length` value in the headers, defaulting to 0.
fn content_length_of(headers: &[u8]) -> usize {
    let text = String::from_utf8_lossy(headers).to_lowercase();
    text.lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

const CLASSIFY_CALL: &str = r#"{
  "function_calls": [
    {"name": "explain_card", "arguments": {"card_family": "r35s_stock", "observation": "a stock card", "next_step": "copy", "suggested_profile_id": "r35s-stock-card"}}
  ],
  "suppressed_calls": [],
  "confidence": 0.9,
  "reasoning": ""
}"#;

#[test]
fn client_speaks_reset_then_complete_and_returns_the_call() {
    let (addr, server) = mock_server(2, |request| {
        if request.starts_with("POST /reset") {
            return (200u16, "{}".into());
        }
        assert!(
            request.starts_with("POST /complete"),
            "unexpected: {request}"
        );
        assert!(
            request.contains(r#""input""#),
            "complete carries the input: {request}"
        );
        (200u16, CLASSIFY_CALL.into())
    });

    let client = ServeClient::with_base_url(format!("http://{addr}"), 2);
    let turn = client.ask("CardFacts: label ROMS").expect("one good turn");
    assert_eq!(turn.call.name, "explain_card");
    assert!(!turn.withheld);
    assert_eq!(turn.confidence, Some(0.9));
    assert_eq!(
        turn.call.arguments["suggested_profile_id"],
        "r35s-stock-card"
    );
    server.join().unwrap();
}

#[test]
fn client_prefers_grounded_calls_and_discounts_withheld_ones() {
    let both = r#"{
      "function_calls": [
        {"name": "explain_card", "arguments": {"a": 1}}
      ],
      "suppressed_calls": [
        {"name": "explain_card", "arguments": {"a": 2}}
      ],
      "confidence": 0.6,
      "reasoning": ""
    }"#;
    let (addr, server) = mock_server(2, move |request| {
        if request.starts_with("POST /reset") {
            return (200u16, "{}".into());
        }
        (200u16, both.into())
    });
    let turn = ServeClient::with_base_url(format!("http://{addr}"), 1)
        .ask("facts")
        .unwrap();
    assert!(!turn.withheld, "grounded calls win over withheld ones");
    assert_eq!(turn.call.arguments["a"], 1);
    server.join().unwrap();

    let only_suppressed = r#"{
      "function_calls": [],
      "suppressed_calls": [
        {"name": "explain_card", "arguments": {"a": 9}}
      ],
      "confidence": 0.55,
      "reasoning": ""
    }"#;
    let (addr, server) = mock_server(2, move |request| {
        if request.starts_with("POST /reset") {
            return (200u16, "{}".into());
        }
        (200u16, only_suppressed.into())
    });
    let turn = ServeClient::with_base_url(format!("http://{addr}"), 1)
        .ask("facts")
        .unwrap();
    assert!(turn.withheld);
    assert_eq!(turn.call.arguments["a"], 9);
    server.join().unwrap();
}

#[test]
fn client_returns_nocall_on_empty_calls() {
    let (addr, server) = mock_server(2, |request| {
        if request.starts_with("POST /reset") {
            return (200u16, "{}".into());
        }
        (
            200u16,
            r#"{"function_calls": [], "suppressed_calls": [], "confidence": 0.9}"#.into(),
        )
    });
    let err = ServeClient::with_base_url(format!("http://{addr}"), 1)
        .ask("junk")
        .unwrap_err();
    assert!(matches!(err, ClientError::NoCall), "got: {err:?}");
    server.join().unwrap();
}

#[test]
fn client_treats_error_status_as_api_error_without_retry() {
    // 429 is an API error, not a transport failure: no retry. The mock
    // refuses the FIRST request (the reset itself) with a valid turn
    // envelope that carries no calls; the turn parser reports NoCall and
    // the served-request counter proves no second attempt is ever made.
    let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = served.clone();
    let (addr, server) = mock_server(8, move |request| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if request.starts_with("POST /reset") {
            return (
                429u16,
                r#"{
  "function_calls": [],
  "suppressed_calls": [],
  "confidence": 0.0,
  "reasoning": "refused"
}"#
                .into(),
            );
        }
        (200u16, "{}".into())
    });
    let err = ServeClient::with_base_url(format!("http://{addr}"), 3)
        .ask("reset then refused")
        .unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Api {
                status_code: 429,
                ..
            }
        ),
        "the refusal surfaces with its real status: {err:?}"
    );
    assert_eq!(
        served.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a refused turn must not be retried"
    );
    server.join().unwrap();
}

#[test]
fn pick_free_port_returns_connectable_ports() {
    let port = serve::pick_free_port().unwrap();
    assert!(port != 0);
    // The picked port is immediately bindable (i.e. free).
    let again = std::net::TcpListener::bind(("127.0.0.1", port));
    assert!(again.is_ok(), "picked port should be free right now");
}

// ------------------------------------------------------------------
// PR 7: Card Doctor (facts, templates, vocabulary validation)
// ------------------------------------------------------------------
use cfw_zero_touch_lib::firstboot::CardSafety;
use cfw_zero_touch_lib::needle::doctor::{self, CardFacts, CardFamily, VolumeDisk};
use cfw_zero_touch_lib::volume::VolumeInfo;

fn doctor_facts() -> CardFacts {
    CardFacts {
        volume_letter: "E".into(),
        volume_label: "EASYROMS".into(),
        file_system: "exFAT".into(),
        total_bytes: 60_000_000_000,
        is_empty: false,
        decision: "ready".into(),
        decision_reason: String::new(),
        disk_volumes: vec![
            VolumeDisk {
                letter: "D".into(),
                label: "BOOT".into(),
            },
            VolumeDisk {
                letter: "E".into(),
                label: "EASYROMS".into(),
            },
        ],
        root_folders: vec!["gba".into(), "nes".into()],
        firstboot_state: "safe".into(),
        firstboot_reason: String::new(),
    }
}

fn corrupted_garbage() -> CardFacts {
    let mut facts = doctor_facts();
    facts.firstboot_state = "corrupted".into();
    facts.firstboot_reason = String::new();
    facts
}

fn facts_volume() -> VolumeInfo {
    VolumeInfo {
        id: "E-TEST".into(),
        letter: "E".into(),
        label: "EASYROMS".into(),
        file_system: "exFAT".into(),
        total_bytes: 60_000_000_000,
        is_empty: false,
        is_removable: true,
    }
}

#[test]
fn doctor_armed_template_names_first_boot() {
    let mut facts = doctor_facts();
    facts.firstboot_state = "armed".into();
    facts.firstboot_reason = "firstboot is still armed (expandtoexfat.sh)".into();
    let report = doctor::explain(&facts);
    assert_eq!(report.heading, "This card has not finished its first boot");
    assert!(
        report.steps[1].contains("game menu"),
        "got: {:?}",
        report.steps
    );
    assert!(report.engine_text.is_none() && report.suggested_profile_id.is_none());
}

#[test]
fn doctor_unknown_template_names_boot_visibility() {
    let mut facts = doctor_facts();
    facts.firstboot_state = "unknown".into();
    facts.firstboot_reason = "could not find which disk holds E:.".into();
    let report = doctor::explain(&facts);
    assert_eq!(
        report.heading,
        "The app cannot verify first boot on this card"
    );
}

#[test]
fn family_identification_reads_layout_signatures() {
    let mut facts = doctor_facts();
    facts.root_folders = vec!["Roms/FC".into(), "Easytitles".into(), "Emulators".into()];
    assert_eq!(doctor::identify_family(&facts), CardFamily::R35sStock);
    facts.root_folders = vec!["Roms/PSP".into(), "Roms/NEOGEO".into(), "bios".into()];
    assert_eq!(doctor::identify_family(&facts), CardFamily::R36sClone);
    facts.volume_label = "SHARE".into();
    facts.root_folders = vec!["roms".into()];
    assert_eq!(doctor::identify_family(&facts), CardFamily::RocknixShare);
    facts.volume_label = "EASYROMS".into();
    facts.root_folders = vec!["gba".into()];
    assert_eq!(doctor::identify_family(&facts), CardFamily::DarkosEasyroms);
}

#[test]
fn identify_family_reads_real_card_shapes() {
    // Flat app-filled clone card (the r36s profile layout, label ROMS).
    let mut facts = doctor_facts();
    facts.volume_label = "ROMS".into();
    facts.root_folders = [
        "bios",
        "gb",
        "gba",
        "gbc",
        "megadrive",
        "nes",
        "pcengine",
        "snes",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(doctor::identify_family(&facts), CardFamily::R36sClone);

    // A flat ArkOS card is still easyroms — the label rule outranks the
    // flat signature.
    facts.volume_label = "EASYROMS".into();
    assert_eq!(doctor::identify_family(&facts), CardFamily::DarkosEasyroms);

    // Stock clone partition: collect_facts surfaces the second-level
    // Roms/* names the fingerprints read.
    let card = scratch("doctor-stock");
    for sub in ["Roms/PSP", "Roms/NEOGEO", "bios"] {
        fs::create_dir_all(card.join(sub)).unwrap();
    }
    let facts = doctor::collect_facts(
        &facts_volume(),
        "ready",
        "",
        vec![],
        &CardSafety::Safe,
        &card,
    );
    assert!(facts.root_folders.iter().any(|f| f == "roms/psp"));
    assert!(facts.root_folders.iter().any(|f| f == "roms/neogeo"));
    assert_eq!(doctor::identify_family(&facts), CardFamily::R36sClone);
}

#[test]
fn engine_turns_are_vocabulary_validated() {
    let good = serde_json::json!({
        "card_family": "r35s_stock",
        "observation": "a stock card",
        "next_step": "copy",
        "suggested_profile_id": "r35s-stock-card",
    });
    let (text, profile) = doctor::parse_engine_turn("explain_card", &good).unwrap();
    assert_eq!(text.as_deref(), Some("a stock card"));
    assert_eq!(profile.as_deref(), Some("r35s-stock-card"));
    assert!(doctor::parse_engine_turn("other_tool", &good).is_none());
    let mut without_obs = good.clone();
    without_obs.as_object_mut().unwrap().remove("observation");
    let (text, profile) = doctor::parse_engine_turn("explain_card", &without_obs).unwrap();
    assert!(text.is_none());
    assert_eq!(profile.as_deref(), Some("r35s-stock-card"));
    let mut custom_profile = good.clone();
    custom_profile["suggested_profile_id"] = serde_json::json!("custom-handheld");
    assert!(doctor::parse_engine_turn("explain_card", &custom_profile).is_none());
}

#[test]
fn gates_are_byte_identical_with_engine_off_on_and_garbage() {
    let mut facts = doctor_facts();
    facts.firstboot_state = "armed".into();
    facts.firstboot_reason = "firstboot is still armed (expandtoexfat.sh)".into();
    let off = doctor::safety_of(&facts);
    assert_eq!(
        off,
        CardSafety::Armed("firstboot is still armed (expandtoexfat.sh)".into())
    );
    let with_engine = doctor::diagnose(&facts, None);
    assert_eq!(with_engine.steps[0], "The handheld would format EASYROMS and erase anything copied now. firstboot is still armed (expandtoexfat.sh)");
    let garbage = corrupted_garbage();
    assert_eq!(
        doctor::safety_of(&garbage),
        CardSafety::Unknown(String::new())
    );
    let report = doctor::explain(&garbage);
    assert_eq!(
        report.heading,
        "The app cannot verify first boot on this card"
    );
}

#[test]
fn collect_facts_caps_root_folders_and_serializes_cleanly() {
    let root = scratch("doctor-facts");
    for i in 0..100 {
        fs::create_dir_all(root.join(format!("folder{i:03}"))).unwrap();
    }
    fs::write(root.join("topfile.txt"), b"x").unwrap();
    let volume = facts_volume();
    let facts = doctor::collect_facts(
        &volume,
        "ready",
        "",
        vec![VolumeDisk {
            letter: "E".into(),
            label: "EASYROMS".into(),
        }],
        &CardSafety::Safe,
        &root,
    );
    assert_eq!(facts.root_folders.len(), 64, "capped");
    assert!(
        facts.root_folders.windows(2).all(|pair| pair[0] <= pair[1]),
        "sorted"
    );
    let json = serde_json::to_string(&facts).unwrap();
    assert!(
        !json.contains("topfile"),
        "file names never enter the facts"
    );
}

#[cfg(windows)]
#[test]
fn junctioned_library_folders_scan_their_targets_not_the_links() {
    // A ROM bridge built as NTFS junctions: roms/gba -> real/GBA. The
    // scan must follow the links (finding the real ROMs once) instead
    // of reading each junction as a stray no-extension file.
    let real = scratch("junction-real");
    fs::create_dir_all(real.join("GBA")).unwrap();
    fs::write(real.join("GBA/Advance Wars (U).gba"), b"rom").unwrap();
    let lib = scratch("junction-lib");
    fs::create_dir_all(&lib).unwrap();
    let mkjunction = |name: &str| {
        std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                name,
                real.join("GBA").as_os_str().to_string_lossy().as_ref(),
            ])
            .current_dir(&lib)
            .output()
            .unwrap_or_else(|error| panic!("could not run mklink: {error}"))
    };
    let status = mkjunction("gba");
    assert!(status.status.success(), "mklink /J failed: {status:?}");

    let files = index::scan_library(&lib).unwrap();
    let rels: Vec<String> = files
        .iter()
        .map(|f| f.relative.to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(
        rels,
        vec!["gba/Advance Wars (U).gba".to_string()],
        "the junction must resolve to its target file: {rels:?}"
    );

    // Two junctions to one target collapse to one scan of it.
    let status = mkjunction("gba2");
    assert!(status.status.success(), "mklink /J failed: {status:?}");
    let files = index::scan_library(&lib).unwrap();
    assert_eq!(files.len(), 1, "duplicate link target counted once");
}

#[test]
fn marker_summary_lists_only_watched_boot_files() {
    let boot = scratch("doctor-boot");
    fs::write(boot.join("expandtoexfat.sh"), b"x").unwrap();
    fs::write(boot.join("readme.txt"), b"x").unwrap();
    assert_eq!(
        doctor::marker_summary(Some(&boot)),
        vec!["expandtoexfat.sh".to_string()]
    );
    assert!(doctor::marker_summary(None).is_empty());
}

// ------------------------------------------------------------------
// PR 8: embedding dedupe in copy preview (offline, injected embeddings)
// ------------------------------------------------------------------
use cfw_zero_touch_lib::needle::dedupe::{self, IncomingFile, ResidentFile};
use cfw_zero_touch_lib::romcopy::{CopyAction, CopyItem, CopyPlan};

/// Same hand-picked vectors as the PR 4 fixture, plus the dedupe legs:
// - "advance wars" vs "clone wars" sit at ~0.999 (same game, grouped);
// - "mystery game" sits at 0.8 vs "advance wars" (review, never grouped);
// - "golden sun" is orthogonal (different game, never grouped).
fn dedupe_embed(stem: &str) -> Result<Vec<f32>, String> {
    match stem {
        "advance wars" => Ok(vec![1.0, 0.0, 0.0, 0.0]),
        "clone wars" => Ok(vec![0.999, 0.0447, 0.0, 0.0]),
        "golden sun" => Ok(vec![0.0, 1.0, 0.0, 0.0]),
        "mystery game" => Ok(vec![0.8, 0.0, 0.6, 0.0]),
        _ => Ok(vec![0.25, 0.25, 0.25, 0.25]),
    }
}

fn dedupe_systems() -> Vec<SystemFolder> {
    vec![
        system("gba", "gba", &[".gba", ".zip"]),
        system("nes", "nes", &[".nes", ".zip"]),
    ]
}

fn resident(system: &str, dest: &str) -> ResidentFile {
    ResidentFile {
        system_id: system.into(),
        stem: tags::clean_stem(dest.rsplit('/').next().unwrap_or(dest)),
        relative_dest: dest.into(),
    }
}

fn incoming(system: &str, dest: &str, size: u64) -> IncomingFile {
    IncomingFile {
        system_id: system.into(),
        stem: tags::clean_stem(dest.rsplit('/').next().unwrap_or(dest)),
        relative_dest: dest.into(),
        size,
    }
}

#[test]
fn near_identical_stems_group_within_one_system() {
    let residents = vec![resident("gba", "gba/Advance Wars (U).gba")];
    let incoming = vec![incoming("gba", "gba/Advance Wars (USA) [!].gba", 8)];
    let mut embed = dedupe_embed;
    let groups = dedupe::group_duplicates(&residents, &incoming, &mut embed);
    assert_eq!(groups.len(), 1, "same game, same system: one group");
    assert_eq!(groups[0].system_id, "gba");
    assert_eq!(groups[0].resident, "gba/Advance Wars (U).gba");
    assert_eq!(groups[0].incoming.len(), 1);
    assert!(
        groups[0].incoming[0].similarity >= dedupe::DEDUPE_SIM,
        "similarity: {}",
        groups[0].incoming[0].similarity
    );
}

#[test]
fn unrelated_and_cross_system_stems_never_group() {
    let residents = vec![
        resident("gba", "gba/Advance Wars (U).gba"),
        resident("gba", "gba/Golden Sun (USA).gba"),
    ];
    let incoming = vec![
        incoming("gba", "gba/Mystery Game (Japan).zip", 3),
        incoming("nes", "nes/Advance Wars (USA) [!].nes", 8),
    ];
    let mut embed = dedupe_embed;
    let groups = dedupe::group_duplicates(&residents, &incoming, &mut embed);
    assert!(
        groups.is_empty(),
        "0.8 similarity and cross-system pairs group nothing: {groups:?}"
    );
}

#[test]
fn scan_card_lists_mapped_system_files_only() {
    let card = scratch("dedupe-card");
    fs::create_dir_all(card.join("gba")).unwrap();
    fs::write(card.join("gba/Advance Wars (U).gba"), b"resident").unwrap();
    fs::write(card.join("gba/notes.txt"), b"resident").unwrap();
    fs::create_dir_all(card.join("unmapped")).unwrap();
    fs::write(card.join("unmapped/stray.gba"), b"x").unwrap();
    let residents = dedupe::scan_card(&card, "arkos_easyroms_root", &dedupe_systems());
    let dests: Vec<&str> = residents.iter().map(|r| r.relative_dest.as_str()).collect();
    assert!(dests.contains(&"gba/Advance Wars (U).gba"));
    assert!(
        !dests.iter().any(|d| d.contains("unmapped")),
        "unmapped folders are skipped: {dests:?}"
    );
    assert!(
        residents.iter().all(|r| r.system_id == "gba"),
        "mapped through the gba system: {residents:?}"
    );
}

#[test]
fn plan_incoming_maps_destinations_back_to_systems() {
    let plan = CopyPlan {
        items: vec![
            CopyItem {
                source: PathBuf::from("lib/gba/Game.gba"),
                relative_dest: "roms/gba/Game.gba".into(),
                bytes: 4,
                action: CopyAction::Copy,
            },
            CopyItem {
                source: PathBuf::from("lib/skip.gba"),
                relative_dest: "roms/gba/Skip.gba".into(),
                bytes: 4,
                action: CopyAction::SkipUnchanged,
            },
            CopyItem {
                source: PathBuf::from("lib/bios/scph.bin"),
                relative_dest: "roms/bios/scph.bin".into(),
                bytes: 4,
                action: CopyAction::Copy,
            },
        ],
        warning: None,
    };
    let incoming = dedupe::plan_incoming(&plan, "rocknix_roms_nested", &dedupe_systems());
    assert_eq!(incoming.len(), 1, "copy items in a mapped system only");
    assert_eq!(incoming[0].system_id, "gba");
    assert_eq!(incoming[0].relative_dest, "roms/gba/Game.gba");
}

#[test]
fn scan_card_lists_only_game_files_the_system_accepts() {
    // Live-QA case: a video sharing a stem with a game (pulsar.mp4 vs
    // pulsar.zip) must never become a dedupe resident.
    let card = scratch("dedupe-ext");
    fs::create_dir_all(card.join("neogeo/downloaded_videos")).unwrap();
    fs::write(card.join("neogeo/pulsar.zip"), b"rom").unwrap();
    fs::write(card.join("neogeo/downloaded_videos/pulsar.mp4"), b"vid").unwrap();
    fs::write(card.join("neogeo/notes.txt"), b"n").unwrap();
    let systems = vec![system("neogeo", "neogeo", &[".zip"])];
    let residents = dedupe::scan_card(&card, "arkos_easyroms_root", &systems);
    let dests: Vec<&str> = residents.iter().map(|r| r.relative_dest.as_str()).collect();
    assert_eq!(dests, vec!["neogeo/pulsar.zip"]);

    // A card holding ONLY the video: no residents at all, so a
    // same-stem incoming game groups nothing.
    let video_only = scratch("dedupe-ext-video");
    fs::create_dir_all(video_only.join("neogeo/downloaded_videos")).unwrap();
    fs::write(
        video_only.join("neogeo/downloaded_videos/pulsar.mp4"),
        b"vid",
    )
    .unwrap();
    let residents = dedupe::scan_card(&video_only, "arkos_easyroms_root", &systems);
    assert!(
        residents.is_empty(),
        "videos never residents: {residents:?}"
    );
    let incoming = vec![incoming("neogeo", "neogeo/Pulsar (U).zip", 8)];
    let mut embed = dedupe_embed;
    assert!(dedupe::group_duplicates(&residents, &incoming, &mut embed).is_empty());
}

#[test]
fn apply_keep_choices_defaults_to_skip_and_never_touches_residents() {
    let residents = vec![resident("gba", "gba/Advance Wars (U).gba")];
    let incoming = vec![
        incoming("gba", "gba/Advance Wars (USA) [!].gba", 8),
        incoming("gba", "gba/Golden Sun (USA).gba", 7),
    ];
    let mut embed = dedupe_embed;
    let groups = dedupe::group_duplicates(&residents, &incoming, &mut embed);
    assert_eq!(groups.len(), 1, "only the renamed twin groups");

    let plan = CopyPlan {
        items: vec![
            CopyItem {
                source: PathBuf::from("lib/a"),
                relative_dest: "gba/Advance Wars (USA) [!].gba".into(),
                bytes: 8,
                action: CopyAction::Copy,
            },
            CopyItem {
                source: PathBuf::from("lib/b"),
                relative_dest: "gba/Golden Sun (USA).gba".into(),
                bytes: 7,
                action: CopyAction::Copy,
            },
        ],
        warning: None,
    };
    // Default: grouped incoming is skipped, the ungrouped game stays.
    let (filtered, removed) =
        dedupe::apply_keep_choices(plan, &groups, &std::collections::HashSet::new());
    assert_eq!(removed, 1);
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].relative_dest, "gba/Golden Sun (USA).gba");

    // Explicit keep restores the grouped file.
    let plan = CopyPlan {
        items: vec![CopyItem {
            source: PathBuf::from("lib/a"),
            relative_dest: "gba/Advance Wars (USA) [!].gba".into(),
            bytes: 8,
            action: CopyAction::Copy,
        }],
        warning: None,
    };
    let mut keep = std::collections::HashSet::new();
    keep.insert("gba/Advance Wars (USA) [!].gba".to_string());
    let (kept, removed) = dedupe::apply_keep_choices(plan, &groups, &keep);
    assert_eq!((kept.items.len(), removed), (1, 0));

    // The resident file is scan output, never a plan item: nothing in
    // this module can delete from the card — removal only filters the
    // incoming plan.
    assert!(
        !groups[0].resident.contains("USA) [!]"),
        "resident is the on-card file: {}",
        groups[0].resident
    );
}

#[test]
fn embed_failures_degrade_to_no_groups_never_an_error() {
    let residents = vec![resident("gba", "gba/Advance Wars (U).gba")];
    let incoming = vec![incoming("gba", "gba/Advance Wars (USA) [!].gba", 8)];
    let mut failing = |_: &str| -> Result<Vec<f32>, String> { Err("helper down".into()) };
    let groups = dedupe::group_duplicates(&residents, &incoming, &mut failing);
    assert!(groups.is_empty(), "no embeddings: no groups, no error");
}

#[test]
fn end_to_end_card_scan_groups_against_plan_stems() {
    // A card holding "Advance Wars (U).gba" plus a plan carrying its
    // renamed twin and an unrelated game: one group, default skip
    // removes only the twin.
    let card = scratch("dedupe-e2e-card");
    fs::create_dir_all(card.join("gba")).unwrap();
    fs::write(card.join("gba/Advance Wars (U).gba"), b"resident").unwrap();
    let residents = dedupe::scan_card(&card, "arkos_easyroms_root", &dedupe_systems());
    let plan = CopyPlan {
        items: vec![
            CopyItem {
                source: PathBuf::from("lib/twin"),
                relative_dest: "gba/Advance Wars (USA) [!].gba".into(),
                bytes: 8,
                action: CopyAction::Copy,
            },
            CopyItem {
                source: PathBuf::from("lib/other"),
                relative_dest: "gba/Golden Sun (USA).gba".into(),
                bytes: 7,
                action: CopyAction::Copy,
            },
        ],
        warning: None,
    };
    let incoming = dedupe::plan_incoming(&plan, "arkos_easyroms_root", &dedupe_systems());
    assert_eq!(incoming.len(), 2);
    let mut embed = dedupe_embed;
    let groups = dedupe::group_duplicates(&residents, &incoming, &mut embed);
    assert_eq!(groups.len(), 1);
    let card_before: Vec<String> = index::scan_library(&card)
        .unwrap()
        .iter()
        .map(|f| f.relative.to_string_lossy().replace('\\', "/"))
        .collect();
    let (filtered, removed) =
        dedupe::apply_keep_choices(plan, &groups, &std::collections::HashSet::new());
    assert_eq!(removed, 1);
    assert_eq!(filtered.items.len(), 1);
    let card_after: Vec<String> = index::scan_library(&card)
        .unwrap()
        .iter()
        .map(|f| f.relative.to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(card_before, card_after, "the card is untouched by dedupe");
}
