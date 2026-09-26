//! Verified acquisition of Needle artifacts into the local cache.
//!
//! Reuses the flash pipeline's `ensure_image` discipline: every download
//! lands as `<name>.partial`, is checked against the compiled-in SHA-256,
//! and is renamed into place only when it matches; a mismatch is discarded.
//! A verified copy in any cache directory short-circuits the download.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::manifest::{self, Artifact, ArtifactRole};
use crate::flash;
use crate::store;

/// Cache directories, most preferred first: `CFW_NEEDLE_DIR` (developer
/// override or vendored copy) then the store root's `needle` folder
/// (`CFW_STUDIO_DATA` overrides the store root for tests).
pub fn cache_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Ok(dir) = std::env::var("CFW_NEEDLE_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            directories.push(PathBuf::from(trimmed));
        }
    }
    directories.push(store::store_root().join("needle"));
    directories
}

/// Resolves a manifest id to a runtime-downloadable artifact. This is the
/// IPC boundary for `needle_acquire`: build inputs and unknown ids are
/// rejected, so no free-form string reaches the filesystem.
pub fn runtime_artifact(id: &str) -> Result<&'static Artifact, String> {
    let artifact =
        manifest::find(id).ok_or_else(|| format!("unknown needle artifact: {id}"))?;
    if artifact.role != ArtifactRole::RuntimeDownload {
        return Err(format!(
            "{} is a build-time input and is never downloaded at runtime",
            artifact.id
        ));
    }
    Ok(artifact)
}

/// Acquire core over explicit cache directories (testable, no env).
/// Same discipline as the flash pipeline: a verified copy in any cache
/// directory short-circuits the fetch; otherwise the fetch (which owns
/// the URL) writes `<file_name>.partial`, the bytes are checked against
/// `sha256`, and the file is renamed into place only on a match (a
/// mismatch is discarded).
pub fn ensure_in(
    directories: &[PathBuf],
    sha256: &str,
    file_name: &str,
    mut fetch: impl FnMut(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    for directory in directories {
        let cached = directory.join(file_name);
        if cached.is_file() && file_matches(&cached, sha256)? {
            return Ok(cached);
        }
    }
    let destination_dir = directories
        .first()
        .ok_or("no needle cache directory is configured")?;
    std::fs::create_dir_all(destination_dir)
        .map_err(|error| format!("could not create the needle cache: {error}"))?;
    let partial = destination_dir.join(format!("{file_name}.partial"));
    fetch(&partial)?;
    if !file_matches(&partial, sha256)? {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "downloaded {file_name} did not match the published checksum"
        ));
    }
    let final_path = destination_dir.join(file_name);
    if final_path.exists() {
        let _ = std::fs::remove_file(&final_path);
    }
    std::fs::rename(&partial, &final_path)
        .map_err(|error| format!("could not save {file_name}: {error}"))?;
    Ok(final_path)
}

fn file_matches(path: &Path, expected_sha256: &str) -> Result<bool, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let actual = flash::sha256_reader(file)?;
    Ok(actual.eq_ignore_ascii_case(expected_sha256))
}

/// Acquires one manifest artifact using the production cache directories.
pub fn ensure_artifact(
    artifact: &Artifact,
    fetch: impl FnMut(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    ensure_in(
        &cache_directories(),
        artifact.sha256,
        artifact.file_name,
        fetch,
    )
}

/// One manifest row as seen by the frontend status surface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStatus {
    pub id: String,
    /// "runtime-download" | "build-input"
    pub role: String,
    pub file_name: String,
    pub size: u64,
    pub present: bool,
    /// True only when the cached file's SHA-256 matches the pin.
    pub verified: bool,
    pub path: Option<String>,
}

fn role_name(role: ArtifactRole) -> &'static str {
    match role {
        ArtifactRole::RuntimeDownload => "runtime-download",
        ArtifactRole::BuildInput => "build-input",
    }
}

/// Reports presence and pin-verification of the given artifact rows,
/// checking the cache directories in order. The first directory holding
/// the file name wins; build inputs show up only when a developer
/// pre-seeds them.
pub fn status_for(directories: &[PathBuf], artifacts: &[Artifact]) -> Vec<ArtifactStatus> {
    artifacts
        .iter()
        .map(|artifact| {
            let mut found: Option<(PathBuf, bool)> = None;
            for directory in directories {
                let candidate = directory.join(artifact.file_name);
                if !candidate.is_file() {
                    continue;
                }
                let verified = std::fs::File::open(&candidate)
                    .ok()
                    .and_then(|file| flash::sha256_reader(file).ok())
                    .map(|hash| hash.eq_ignore_ascii_case(artifact.sha256))
                    .unwrap_or(false);
                found = Some((candidate, verified));
                break;
            }
            let (path, verified) = match found {
                Some((path, verified)) => (Some(path), verified),
                None => (None, false),
            };
            ArtifactStatus {
                id: artifact.id.to_string(),
                role: role_name(artifact.role).to_string(),
                file_name: artifact.file_name.to_string(),
                size: artifact.size,
                present: path.is_some(),
                verified,
                path: path.map(|p| p.display().to_string()),
            }
        })
        .collect()
}

/// Production status over `cache_directories()` and the compiled-in pins.
pub fn artifact_status() -> Vec<ArtifactStatus> {
    status_for(&cache_directories(), &manifest::NEEDLE_ARTIFACTS)
}
