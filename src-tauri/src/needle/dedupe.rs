//! Embedding dedupe in copy preview (PR 8): group plan items that look
//! like games already on the card.
//!
//! Advisory only — dedupe only REMOVES items from the copy plan (default
//! skip for incoming); residents on the card are never touched and
//! `romcopy::execute_copy` is unchanged. Grouping is within one system at
//! cosine >= `DEDUPE_SIM` over cleaned stems (the same `clean_stem` the
//! index uses, so "Advance Wars (U).gba" and "Advance Wars (USA) [!].gba"
//! meet at ~0.99 while unrelated titles sit ~0.92-0.97).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use super::index::EmbedFn;
use crate::folder_map::storage_folder;
use crate::profiles::SystemFolder;
use crate::romcopy::{CopyAction, CopyPlan};

/// Cosine at or above which an incoming stem counts as the same game as
/// a resident stem, within one system. Same band as the tier-3
/// auto-route point: own-title variants sit ~0.99, unrelated ~0.92-0.97.
pub const DEDUPE_SIM: f64 = 0.985;

/// One file already on the card, in a mapped system folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidentFile {
    pub system_id: String,
    pub stem: String,
    pub relative_dest: String,
}

/// One plan item considered for dedupe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingFile {
    pub system_id: String,
    pub stem: String,
    pub relative_dest: String,
    pub size: u64,
}

/// One incoming file inside a group, as shown in preview.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateItem {
    pub relative_dest: String,
    pub size: u64,
    pub similarity: f64,
}

/// One resident + the incoming files that look like it. The UI offers
/// keep/skip per group; default is skip incoming (residents untouched).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub system_id: String,
    pub resident: String,
    pub incoming: Vec<DuplicateItem>,
}

fn cosine(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let (x, y) = (*x as f64, *y as f64);
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some(dot / (na.sqrt() * nb.sqrt()))
}

/// Embeds each unique stem once; failures embed as missing (the file is
/// then never grouped — dedupe degrades to no groups, never an error).
fn embed_unique(stems: &[String], embed: &mut EmbedFn) -> HashMap<String, Vec<f32>> {
    let mut seen = HashSet::new();
    let mut out = HashMap::new();
    for stem in stems {
        if seen.insert(stem.clone()) {
            if let Ok(vector) = embed(stem) {
                out.insert(stem.clone(), vector);
            }
        }
    }
    out
}

/// Groups incoming files against residents within the same system.
/// Deterministic order: groups sorted by (system_id, resident), incoming
/// sorted by relative_dest.
pub fn group_duplicates(
    residents: &[ResidentFile],
    incoming: &[IncomingFile],
    embed: &mut EmbedFn,
) -> Vec<DuplicateGroup> {
    let mut stems: Vec<String> = Vec::new();
    for file in residents {
        stems.push(file.stem.clone());
    }
    for item in incoming {
        stems.push(item.stem.clone());
    }
    let vectors = embed_unique(&stems, embed);

    // Resident vectors by (system_id, index).
    let mut by_system: HashMap<&str, Vec<(&ResidentFile, &[f32])>> = HashMap::new();
    for resident in residents {
        if let Some(vector) = vectors.get(&resident.stem) {
            by_system
                .entry(resident.system_id.as_str())
                .or_default()
                .push((resident, vector));
        }
    }

    // Incoming -> best resident match within the same system.
    let mut groups: HashMap<String, DuplicateGroup> = HashMap::new();
    for item in incoming {
        let Some(vector) = vectors.get(&item.stem) else {
            continue;
        };
        let empty = Vec::new();
        let candidates = by_system.get(item.system_id.as_str()).unwrap_or(&empty);
        let mut best: Option<(&ResidentFile, f64)> = None;
        for (resident, resident_vector) in candidates {
            if let Some(similarity) = cosine(vector, resident_vector) {
                if similarity >= DEDUPE_SIM && best.map(|(_, s)| similarity > s).unwrap_or(true) {
                    best = Some((resident, similarity));
                }
            }
        }
        if let Some((resident, similarity)) = best {
            let key = format!("{}\0{}", item.system_id, resident.relative_dest);
            groups
                .entry(key)
                .or_insert_with(|| DuplicateGroup {
                    system_id: item.system_id.clone(),
                    resident: resident.relative_dest.clone(),
                    incoming: Vec::new(),
                })
                .incoming
                .push(DuplicateItem {
                    relative_dest: item.relative_dest.clone(),
                    size: item.size,
                    similarity,
                });
        }
    }

    let mut out: Vec<DuplicateGroup> = groups.into_values().collect();
    for group in &mut out {
        group
            .incoming
            .sort_by(|a, b| a.relative_dest.cmp(&b.relative_dest));
    }
    out.sort_by_key(|g| (g.system_id.clone(), g.resident.clone()));
    out
}

/// Scans the card's mapped system folders for resident games. Only
/// files the system accepts (its extension list, case-insensitive) are
/// residents — videos, saves and notes that happen to share a folder
/// never match incoming games by stem. Bios and unmapped folders are
/// skipped. Missing folders are empty, never an error.
pub fn scan_card(dest_root: &Path, layout: &str, systems: &[SystemFolder]) -> Vec<ResidentFile> {
    let mut residents = Vec::new();
    for system in systems {
        let Ok(prefix) = storage_folder(layout, &system.folder) else {
            continue;
        };
        let dir = dest_root.join(prefix.replace('/', std::path::MAIN_SEPARATOR_STR));
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut stack: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&path) {
                    stack.extend(entries.flatten().map(|e| e.path()));
                }
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if !crate::romcopy::extension_matches(&path, &system.extensions) {
                continue; // e.g. neogeo/downloaded_videos/pulsar.mp4
            }
            let stem =
                super::tags::clean_stem(path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
            if stem.is_empty() {
                continue;
            }
            let relative_dest = path
                .strip_prefix(dest_root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            residents.push(ResidentFile {
                system_id: system.id.clone(),
                stem,
                relative_dest,
            });
        }
    }
    residents.sort_by_key(|r| (r.system_id.clone(), r.relative_dest.clone()));
    residents
}

/// Maps each Copy plan item back to its system via the destination
/// prefix. Items whose destination matches no system (bios, unknown)
/// are never deduped.
pub fn plan_incoming(plan: &CopyPlan, layout: &str, systems: &[SystemFolder]) -> Vec<IncomingFile> {
    let mut prefixes: Vec<(String, String)> = Vec::new();
    for system in systems {
        if let Ok(prefix) = storage_folder(layout, &system.folder) {
            prefixes.push((prefix, system.id.clone()));
        }
    }
    // Longest prefix first so nested prefixes win.
    prefixes.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    let mut out = Vec::new();
    for item in &plan.items {
        if item.action != CopyAction::Copy {
            continue;
        }
        let Some((_, system_id)) = prefixes.iter().find(|(prefix, _)| {
            item.relative_dest == *prefix || item.relative_dest.starts_with(&format!("{prefix}/"))
        }) else {
            continue;
        };
        let file_name = item.relative_dest.rsplit('/').next().unwrap_or("");
        let stem = super::tags::clean_stem(file_name);
        if stem.is_empty() {
            continue;
        }
        out.push(IncomingFile {
            system_id: system_id.to_string(),
            stem,
            relative_dest: item.relative_dest.clone(),
            size: item.bytes,
        });
    }
    out
}

/// Removes grouped incoming files from the plan unless their
/// relative_dest is in `keep`. Returns the filtered plan plus the count
/// removed. Residents are never modified — only plan items go away.
pub fn apply_keep_choices(
    plan: CopyPlan,
    groups: &[DuplicateGroup],
    keep: &HashSet<String>,
) -> (CopyPlan, usize) {
    let mut grouped: HashSet<&str> = HashSet::new();
    for group in groups {
        for item in &group.incoming {
            if !keep.contains(&item.relative_dest) {
                grouped.insert(item.relative_dest.as_str());
            }
        }
    }
    let before = plan.items.len();
    let items = plan
        .items
        .into_iter()
        .filter(|item| !grouped.contains(item.relative_dest.as_str()))
        .collect::<Vec<_>>();
    let removed = before - items.len();
    (
        CopyPlan {
            items,
            warning: plan.warning,
        },
        removed,
    )
}
