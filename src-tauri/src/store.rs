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
