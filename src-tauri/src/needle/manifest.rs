//! Compiled-in pin table for the Needle artifacts.
//!
//! Every pin (URL, SHA-256, size) was verified against the bytes actually
//! served at `NEEDLE_PIN_REVISION` when this table was written. The pins are
//! compiled into the app on purpose: the profile feed never becomes a trust
//! anchor for native artifacts (design Key Decision 4).

/// Hugging Face repository revision the pins below were taken from.
pub const NEEDLE_PIN_REVISION: &str = "b274efcb211a9eef48c9a88da4b43bd569696a39";

/// How an artifact enters the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactRole {
    /// Downloaded at runtime, after user consent, verified against `sha256`.
    RuntimeDownload,
    /// Vendored at build time (CI or `CFW_NEEDLE_DIR`); never downloaded.
    BuildInput,
}

/// One pinned Needle artifact.
#[derive(Debug, Clone, Copy)]
pub struct Artifact {
    /// Stable identifier used in IPC (`needle_acquire`) and diag events.
    /// Ids are fixed vocabulary, never free-form paths.
    pub id: &'static str,
    pub role: ArtifactRole,
    /// `https://` URL the artifact is served from.
    pub url: &'static str,
    /// SHA-256 of the exact bytes, lowercase hex.
    pub sha256: &'static str,
    /// Exact byte size, used for consent UI and quick sanity checks.
    pub size: u64,
    /// File name the artifact is cached under (matches the URL's last
    /// path segment so `flash::ensure_image` derives the same name).
    pub file_name: &'static str,
    pub purpose: &'static str,
}

/// The pins. Order is stable: runtime downloads first, build inputs after.
pub const NEEDLE_ARTIFACTS: [Artifact; 4] = [
    Artifact {
        id: "weights",
        role: ArtifactRole::RuntimeDownload,
        url: "https://huggingface.co/Cactus-Compute/needle3/resolve/main/needle3.cact",
        sha256: "c9d915eca282ed42d1a09b143b592adb4cc6744ffe2d294adf5cfc5548170c38",
        size: 35_335_380,
        file_name: "needle3.cact",
        purpose: "Needle 3 model weights (35 MB) for local classification",
    },
    Artifact {
        id: "serve-engine",
        role: ArtifactRole::RuntimeDownload,
        url: "https://huggingface.co/Cactus-Compute/needle3/resolve/main/windows-x86_64/needle.exe",
        sha256: "8dfa55f2a1280f4c7f2b9d7396ca2d3d4580ccc05c3bb3da62101b405323b062",
        size: 1_276_928,
        file_name: "needle.exe",
        purpose: "Serve-mode HTTP engine (Phase 2 Card Doctor; unused in Phase 1)",
    },
    Artifact {
        id: "embed-lib",
        role: ArtifactRole::BuildInput,
        url: "https://huggingface.co/Cactus-Compute/needle3/resolve/main/windows-x86_64/libneedle.a",
        sha256: "6fb0b9bccfa9f54d46e05a279273c15021570a53a8b3945613d80d299ca1f634",
        size: 1_808_664,
        file_name: "libneedle.a",
        purpose: "Static C API archive, statically linked into cfw-embed at build time (PR 2)",
    },
    Artifact {
        id: "embed-header",
        role: ArtifactRole::BuildInput,
        url: "https://huggingface.co/Cactus-Compute/needle3/resolve/main/windows-x86_64/needle.h",
        sha256: "3aa713942528d944598458cecb4a262f2cc49349bec63355f91df0b159964e55",
        size: 1_187,
        file_name: "needle.h",
        purpose: "C API header for cfw-embed (PR 2)",
    },
];

/// Finds an artifact by its manifest id.
pub fn find(id: &str) -> Option<&'static Artifact> {
    NEEDLE_ARTIFACTS.iter().find(|artifact| artifact.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = NEEDLE_ARTIFACTS.iter().map(|a| a.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), NEEDLE_ARTIFACTS.len());
    }

    #[test]
    fn find_rejects_traversal() {
        assert!(find("../evil").is_none());
        assert!(find("weights/needle3.cact").is_none());
    }
}
