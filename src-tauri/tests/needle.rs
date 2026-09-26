use std::fs;
use std::path::{Path, PathBuf};

use cfw_zero_touch_lib::flash;
use cfw_zero_touch_lib::needle::acquire;
use cfw_zero_touch_lib::needle::manifest::{self, ArtifactRole};

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cfw-needle-{name}-{}-{}",
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
    assert_eq!(manifest::NEEDLE_ARTIFACTS.len(), 4);

    let runtime: Vec<_> = manifest::NEEDLE_ARTIFACTS
        .iter()
        .filter(|a| a.role == ArtifactRole::RuntimeDownload)
        .collect();
    assert_eq!(
        runtime.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec!["weights", "serve-engine"],
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
            sha.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
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

    let path = acquire::ensure_in(
        &dirs,
        &sha,
        "needle3.cact",
        fetch_writer(bytes),
    )
    .expect("verified download should succeed");

    assert_eq!(path, dirs[0].join("needle3.cact"));
    assert!(path.is_file());
    assert!(!dirs[0].join("needle3.cact.partial").exists(), "no partial left behind");
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn ensure_in_discards_mismatched_download() {
    let dirs = vec![scratch("acquire-bad")];
    let expected = sha256_of(b"the pinned bytes");
    let served: &'static [u8] = b"hostile or corrupted bytes";

    let err = acquire::ensure_in(
        &dirs,
        &expected,
        "needle3.cact",
        fetch_writer(served),
    )
    .expect_err("checksum mismatch must fail");

    assert!(err.contains("checksum"), "got: {err}");
    assert!(!dirs[0].join("needle3.cact.partial").exists(), "partial discarded");
    assert!(!dirs[0].join("needle3.cact").exists(), "no final artifact");
}

#[test]
fn ensure_in_uses_cached_verified_file_without_fetching() {
    let dirs = vec![scratch("acquire-cache-primary"), scratch("acquire-cache-secondary")];
    let bytes: &'static [u8] = b"cached weights";
    let sha = sha256_of(bytes);
    // Verified copy sits in the SECOND cache directory (store fallback).
    fs::write(dirs[1].join("needle3.cact"), bytes).unwrap();

    let mut fetched = false;
    let path = acquire::ensure_in(
        &dirs,
        &sha,
        "needle3.cact",
        |_| {
            fetched = true;
            Err("must not fetch when a verified copy is cached".into())
        },
    )
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

    let path = acquire::ensure_in(
        &dirs,
        &sha,
        "needle3.cact",
        fetch_writer(bytes),
    )
    .expect("corrupt cache should be replaced by a verified download");

    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn status_for_reports_presence_and_verification() {
    let dirs = vec![scratch("status")];
    // Empty cache: nothing present, including build inputs.
    let empty = acquire::status_for(&dirs, &manifest::NEEDLE_ARTIFACTS);
    assert_eq!(empty.len(), 4);
    for row in &empty {
        assert!(!row.present, "empty cache reports nothing present: {:?}", row.id);
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
