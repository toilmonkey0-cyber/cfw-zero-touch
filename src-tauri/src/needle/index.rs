//! Per-profile library index: build, persist, query (design PR 2b).
//!
//! The index holds one entry `(system_id, canonical_stem, vector)` per
//! library file that the DETERMINISTIC tiers route (folder match or
//! unique extension). Files that do not route deterministically are the
//! future tier-3 queries and are excluded. One index per profile at
//! `store_root()/needle/index/<profile_id>.bin` — tier routing is
//! profile-dependent (`.bin` is psx-unique in one profile, psx+megadrive
//! in another), so a shared cache would reuse the wrong index.
//!
//! Reuse is gated by three fingerprints; any mismatch rebuilds:
//! - library (root path + per-file relative path/size/mtime),
//! - profile routing (the `(system_id, folder, extensions)` tuples used),
//! - weights (the pin tag of the `.cact` the vectors came from —
//!   embeddings are a function of (text, weights)).
//!
//! Persistence is `.tmp`-then-rename (the `.cfwpart` discipline). Loading
//! bounds-checks everything against the file length before allocating:
//! a corrupt or hostile cache is an error, never memory unsafety.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::tags;
use crate::profiles::SystemFolder;
use crate::store;

pub const INDEX_MAGIC: &[u8; 8] = b"CFWNIDX1";
pub const INDEX_FORMAT_VERSION: u32 = 1;
/// Upper bound on entries accepted from disk (hostile-cache defense).
pub const MAX_INDEX_ENTRIES: u64 = 200_000;
/// Upper bound on the embedding dimension accepted from disk.
pub const MAX_INDEX_DIM: u64 = 8_192;
pub const MAX_STEM_BYTES_ON_DISK: u64 = 4096;
pub const MAX_SYSTEM_ID_BYTES_ON_DISK: u64 = 64;
/// Walk cap: libraries beyond this many files refuse with an error
/// instead of silently truncating.
pub const MAX_LIBRARY_FILES: usize = 200_000;

/// Embedding source seam so tests run offline with synthetic vectors;
/// production glues this to the `cfw-embed` helper.
pub type EmbedFn<'a> = dyn FnMut(&str) -> Result<Vec<f32>, String> + 'a;

// ---------------------------------------------------------------------
// Deterministic routing (tiers 1-2)
// ---------------------------------------------------------------------

/// Folder-match + unique-extension routing, built from the profile's
/// included systems. First system in profile order wins a folder-name
/// claim; an extension routes only when exactly one system claims it.
pub struct DeterministicRouter {
    /// lowercase folder/id -> system id (first claim wins)
    folders: HashMap<String, String>,
    /// lowercase extension (with dot) -> system id, unique claimants only
    extensions: HashMap<String, String>,
    fingerprint: [u8; 32],
}

impl DeterministicRouter {
    pub fn from_systems(systems: &[SystemFolder]) -> Self {
        let mut folders: HashMap<String, String> = HashMap::new();
        let mut ext_claims: HashMap<String, Vec<String>> = HashMap::new();
        let mut hasher = Sha256::new();
        for system in systems {
            folders
                .entry(system.folder.to_lowercase())
                .or_insert_with(|| system.id.clone());
            folders
                .entry(system.id.to_lowercase())
                .or_insert_with(|| system.id.clone());
            for ext in &system.extensions {
                ext_claims
                    .entry(normalized_extension(ext))
                    .or_default()
                    .push(system.id.clone());
            }
            // Routing fingerprint input: the tuples actually used.
            hasher.update(system.id.as_bytes());
            hasher.update(&[0]);
            hasher.update(system.folder.as_bytes());
            hasher.update(&[0]);
            let mut exts: Vec<&String> = system.extensions.iter().collect();
            exts.sort();
            for ext in exts {
                hasher.update(ext.as_bytes());
                hasher.update(&[0]);
            }
            hasher.update(&[0]);
        }
        let extensions = ext_claims
            .into_iter()
            .filter_map(|(ext, claims)| {
                if claims.len() == 1 {
                    let system_id = claims.into_iter().next().expect("len checked above");
                    Some((ext, system_id))
                } else {
                    None
                }
            })
            .collect();
        Self {
            folders,
            extensions,
            fingerprint: hasher.finalize().into(),
        }
    }

    /// Routes one library-relative path deterministically, or `None`.
    pub fn route(&self, relative: &Path) -> Option<&str> {
        // Tier 1: first path component matches a system folder/id
        // (`find_named_dir` semantics: library-root subdirectory,
        // case-insensitive).
        if let Some(first) = relative.iter().next() {
            let first = first.to_string_lossy().to_lowercase();
            if let Some(system_id) = self.folders.get(&first) {
                return Some(system_id.as_str());
            }
        }
        // Tier 2: extension claimed by exactly one system.
        if let Some(name) = relative.file_name().and_then(|n| n.to_str()) {
            let dot = name.rfind('.')?;
            let ext = name[dot..].to_lowercase();
            if let Some(system_id) = self.extensions.get(&ext) {
                return Some(system_id.as_str());
            }
        }
        None
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}

fn normalized_extension(ext: &str) -> String {
    let trimmed = ext.trim().to_lowercase();
    if trimmed.starts_with('.') {
        trimmed
    } else {
        format!(".{trimmed}")
    }
}

// ---------------------------------------------------------------------
// Library scan
// ---------------------------------------------------------------------

pub struct LibraryFile {
    pub relative: PathBuf,
    pub size: u64,
    pub modified_ms: u64,
}

/// Deterministic library scan: every file with its library-relative
/// path, size, and mtime, sorted by relative-path bytes (a total order
/// — a relative path is unique within a library). Shared by index
/// builds and classification.
pub fn scan_library(root: &Path) -> Result<Vec<LibraryFile>, String> {
    let mut files = Vec::new();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<LibraryFile>) -> Result<(), String> {
        let entries =
            fs::read_dir(dir).map_err(|error| format!("could not read {}: {error}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = entry
                .metadata()
                .map_err(|error| format!("could not stat {}: {error}", path.display()))?;
            if meta.is_dir() {
                walk(&path, root, out)?;
                continue;
            }
            if out.len() >= MAX_LIBRARY_FILES {
                return Err(format!(
                    "library exceeds the {}-file scan cap",
                    MAX_LIBRARY_FILES
                ));
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| format!("path escaped the library: {}", path.display()))?
                .to_path_buf();
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            out.push(LibraryFile {
                relative,
                size: meta.len(),
                modified_ms,
            });
        }
        Ok(())
    }
    walk(root, root, &mut files)?;
    // Deterministic order: relative path bytes.
    files.sort_by(|a, b| a.relative.as_os_str().as_encoded_bytes().cmp(b.relative.as_os_str().as_encoded_bytes()));
    Ok(files)
}

fn library_fingerprint(root: &Path, files: &[LibraryFile]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().to_lowercase().as_bytes());
    hasher.update(&[0]);
    hasher.update(&files.len().to_le_bytes());
    for file in files {
        hasher.update(file.relative.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(&file.size.to_le_bytes());
        hasher.update(&file.modified_ms.to_le_bytes());
    }
    hasher.finalize().into()
}

fn weights_fingerprint(weights_tag: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(weights_tag.as_bytes());
    hasher.finalize().into()
}

// ---------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    pub system_id: String,
    pub stem: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryIndex {
    pub dim: usize,
    pub entries: Vec<IndexEntry>,
    pub library: [u8; 32],
    pub routing: [u8; 32],
    pub weights: [u8; 32],
}

impl LibraryIndex {
    /// k=1 cosine query. Returns the best entry index and similarity.
    pub fn query(&self, vector: &[f32]) -> Option<(usize, f64)> {
        if vector.len() != self.dim {
            return None;
        }
        let q_norm = norm(vector);
        if q_norm == 0.0 {
            return None;
        }
        let mut best: Option<(usize, f64)> = None;
        for (index, entry) in self.entries.iter().enumerate() {
            let dot: f64 = entry
                .vector
                .iter()
                .zip(vector)
                .map(|(a, b)| (*a as f64) * (*b as f64))
                .sum();
            let e_norm = norm(&entry.vector);
            if e_norm == 0.0 {
                continue;
            }
            let similarity = dot / (e_norm * q_norm);
            if best.map(|(_, s)| similarity > s).unwrap_or(true) {
                best = Some((index, similarity));
            }
        }
        best
    }
}

fn norm(v: &[f32]) -> f64 {
    v.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt()
}

// ---------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------

/// Validates a profile id before it becomes a file name (IPC boundary).
pub fn index_path(profile_id: &str) -> Result<PathBuf, String> {
    let ok = !profile_id.is_empty()
        && profile_id.len() <= 64
        && profile_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(format!("invalid profile id: {profile_id:?}"));
    }
    Ok(store::store_root().join("needle").join("index").join(format!("{profile_id}.bin")))
}

fn encode(index: &LibraryIndex) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + index.entries.len() * (index.dim * 4 + 16));
    out.extend_from_slice(INDEX_MAGIC);
    out.extend_from_slice(&INDEX_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(index.dim as u32).to_le_bytes());
    out.extend_from_slice(&(index.entries.len() as u64).to_le_bytes());
    out.extend_from_slice(&index.library);
    out.extend_from_slice(&index.routing);
    out.extend_from_slice(&index.weights);
    for entry in &index.entries {
        for value in &entry.vector {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&(entry.system_id.len() as u32).to_le_bytes());
        out.extend_from_slice(entry.system_id.as_bytes());
        out.extend_from_slice(&(entry.stem.len() as u32).to_le_bytes());
        out.extend_from_slice(entry.stem.as_bytes());
    }
    out
}

/// Writes `.tmp` then renames into place.
pub fn save(index: &LibraryIndex, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp)
            .map_err(|error| format!("could not create {}: {error}", tmp.display()))?;
        file.write_all(&encode(index))
            .and_then(|_| file.flush())
            .map_err(|error| format!("could not write {}: {error}", tmp.display()))?;
    }
    fs::rename(&tmp, path)
        .map_err(|error| format!("could not save {}: {error}", path.display()))
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        if self.pos + len > self.data.len() {
            return Err("index file is truncated".into());
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
}

/// Loads and fully validates an index file. Every length is checked
/// against the actual file size before allocation; anything unexpected is
/// an error, never a panic or unchecked allocation.
pub fn load(path: &Path) -> Result<LibraryIndex, String> {
    let mut file =
        File::open(path).map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let mut reader = Reader { data: &data, pos: 0 };

    if reader.take(8)? != INDEX_MAGIC {
        return Err("index file has a bad magic".into());
    }
    let version = reader.u32()?;
    if version != INDEX_FORMAT_VERSION {
        return Err(format!("index format version {version} is not supported"));
    }
    let dim = reader.u32()? as u64;
    if dim == 0 || dim > MAX_INDEX_DIM {
        return Err(format!("index dimension {dim} is out of bounds"));
    }
    let count = reader.u64()?;
    if count > MAX_INDEX_ENTRIES {
        return Err(format!("index claims {count} entries (cap {MAX_INDEX_ENTRIES})"));
    }
    let library: [u8; 32] = reader.take(32)?.try_into().unwrap();
    let routing: [u8; 32] = reader.take(32)?.try_into().unwrap();
    let weights: [u8; 32] = reader.take(32)?.try_into().unwrap();

    // Total size check before allocating anything large.
    let per_vector = dim as usize * 4;
    let expected_min = reader.pos + count as usize * per_vector;
    if expected_min > data.len() {
        return Err("index file is truncated (matrix)".into());
    }

    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let bytes = reader.take(per_vector)?;
        let vector: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let system_len = reader.u32()? as u64;
        if system_len == 0 || system_len > MAX_SYSTEM_ID_BYTES_ON_DISK {
            return Err("index system id length is out of bounds".into());
        }
        let system_id = String::from_utf8(reader.take(system_len as usize)?.to_vec())
            .map_err(|_| "index system id is not UTF-8".to_string())?;
        let stem_len = reader.u32()? as u64;
        if stem_len == 0 || stem_len > MAX_STEM_BYTES_ON_DISK {
            return Err("index stem length is out of bounds".into());
        }
        let stem = String::from_utf8(reader.take(stem_len as usize)?.to_vec())
            .map_err(|_| "index stem is not UTF-8".to_string())?;
        entries.push(IndexEntry {
            system_id,
            stem,
            vector,
        });
    }
    if reader.pos != data.len() {
        return Err("index file has trailing bytes".into());
    }
    Ok(LibraryIndex {
        dim: dim as usize,
        entries,
        library,
        routing,
        weights,
    })
}

// ---------------------------------------------------------------------
// Build / reuse
// ---------------------------------------------------------------------

/// Ensures a current index for `(profile, library, systems, weights)`,
/// reusing the persisted one when all three fingerprints match.
/// Returns the index and whether it was rebuilt (embed calls happened).
pub fn ensure_index(
    profile_id: &str,
    library: &Path,
    systems: &[SystemFolder],
    weights_tag: &str,
    embed: &mut EmbedFn,
) -> Result<(LibraryIndex, bool), String> {
    let path = index_path(profile_id)?;
    let files = scan_library(library)?;
    let lib_fp = library_fingerprint(library, &files);
    let router = DeterministicRouter::from_systems(systems);
    let routing_fp = router.fingerprint();
    let weights_fp = weights_fingerprint(weights_tag);

    if let Ok(existing) = load(&path) {
        if existing.library == lib_fp
            && existing.routing == routing_fp
            && existing.weights == weights_fp
        {
            return Ok((existing, false));
        }
    }

    let mut entries = Vec::new();
    let mut dim = 0usize;
    for file in &files {
        let Some(system_id) = router.route(&file.relative) else {
            continue; // tier-3 query candidate, not an index entry
        };
        let stem = tags::clean_stem_of(&file.relative);
        let vector = embed(&stem)?;
        if dim == 0 {
            dim = vector.len();
        } else if vector.len() != dim {
            return Err(format!(
                "embedding dimension changed mid-build ({dim} then {})",
                vector.len()
            ));
        }
        entries.push(IndexEntry {
            system_id: system_id.to_string(),
            stem,
            vector,
        });
    }
    if entries.is_empty() {
        dim = 0;
    }
    let index = LibraryIndex {
        dim,
        entries,
        library: lib_fp,
        routing: routing_fp,
        weights: weights_fp,
    };
    save(&index, &path)?;
    Ok((index, true))
}
