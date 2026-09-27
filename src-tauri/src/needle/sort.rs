//! Smart-sort planner (PR 4): routing tiers 1-3, variant collapse,
//! review ids, and the smart CopyPlan builder.
//!
//! Advisory only — this module proposes routes; destinations still come
//! from `folder_map::storage_folder` + profile values, every destination
//! component is validated (the `ensure_safe_dest` rules, mirrored here
//! because the moat module itself stays untouched), and execution goes
//! through the unchanged `romcopy::execute_copy`.
//!
//! Routing tiers, first hit wins:
//! 1. folder match (deterministic, `find_named_dir` semantics),
//! 2. unique-extension match (deterministic),
//! 3. embedding k=1 cosine vs the library index at ≥ AUTO_ROUTE_SIM;
//!    below the threshold the file becomes a review row carrying the
//!    nearest title and similarity. Routes are enum-validated: an index
//!    hit only routes when its system id is among the included systems.
//!
//! Review ids are opaque (`r1`, `r2`, …) and assigned sequentially over
//! the deterministic scan order — a file's relative path is unique in a
//! library, so the order is total and ids are stable across the preview
//! and copy-time classifications (the copy-time guard depends on it).

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::folder_map::storage_folder;
use crate::needle::index::{DeterministicRouter, EmbedFn, LibraryIndex};
use crate::needle::tags::{self, Region};
use crate::profiles::SystemFolder;
use crate::romcopy::{CopyAction, CopyItem, CopyPlan};

/// Cosine at or above which a tier-3 nearest neighbor auto-routes.
/// The spike's precision point: 97.1% accuracy at 86.2% coverage.
/// 0.975 (94.7% @ 95% coverage) is the documented coverage-first
/// alternative.
pub const AUTO_ROUTE_SIM: f64 = 0.985;

/// Below this many indexed titles the smart option degrades honestly
/// (cold start) instead of promising a review wall.
pub const COLD_START_TITLES: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Tier {
    Folder,
    UniqueExtension,
    Embedding,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Folder => "folder",
            Tier::UniqueExtension => "extension",
            Tier::Embedding => "embedding",
        }
    }
}

/// What variant collapse decided for a routed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VariantDecision {
    Keep,
    /// A higher-priority region won this title.
    SkipRegion {
        winner: Region,
    },
    /// Same region and disc as the kept file.
    SkipDuplicate {
        kept: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRow {
    pub relative: String,
    pub size: u64,
    pub system_id: String,
    pub tier: Tier,
    pub variant: VariantDecision,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRow {
    pub review_id: String,
    pub relative: String,
    pub size: u64,
    pub nearest_stem: Option<String>,
    pub nearest_system: Option<String>,
    pub similarity: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantGroup {
    pub system_id: String,
    pub stem: String,
    pub kept: usize,
    pub skipped_variants: usize,
    pub skipped_duplicates: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub titles: usize,
    pub rebuilt: bool,
    pub dim: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub routes: Vec<RouteRow>,
    pub needs_review: Vec<ReviewRow>,
    pub groups: Vec<VariantGroup>,
    pub index_stats: IndexStats,
    pub cold_start: bool,
}

fn system_by_id<'a>(systems: &'a [SystemFolder], id: &str) -> Option<&'a SystemFolder> {
    systems.iter().find(|s| s.id.eq_ignore_ascii_case(id))
}

fn extension_of(relative: &Path) -> String {
    relative
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default()
}

fn extension_accepted(system: &SystemFolder, ext: &str) -> bool {
    // Mirrors romcopy::collect_files: an empty extension list accepts
    // everything; otherwise the file must carry a listed extension.
    if system.extensions.is_empty() {
        return true;
    }
    system.extensions.iter().any(|listed| {
        let normalized = if listed.starts_with('.') {
            listed.to_lowercase()
        } else {
            format!(".{}", listed.to_lowercase())
        };
        normalized == ext
    })
}

fn any_system_accepts_extension(systems: &[SystemFolder], ext: &str) -> bool {
    systems.iter().any(|system| extension_accepted(system, ext))
}

/// One tier-1/2/3 routed file before variant collapse.
#[derive(Debug, Clone)]
struct Pending {
    relative: String,
    size: u64,
    system_id: String,
    tier: Tier,
}

/// Runs the three routing tiers over the library. `progress` receives
/// (done, total) as files are classified.
#[allow(clippy::too_many_arguments)]
pub fn classify(
    library: &Path,
    systems_all: &[SystemFolder],
    include: &[String],
    index: &LibraryIndex,
    index_rebuilt: bool,
    regions: &[Region],
    embed: &mut EmbedFn,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Classification, String> {
    let included: Vec<SystemFolder> = systems_all
        .iter()
        .filter(|system| {
            include.is_empty() || include.iter().any(|id| id.eq_ignore_ascii_case(&system.id))
        })
        .cloned()
        .collect();
    let router = DeterministicRouter::from_systems(&included);

    let files = crate::needle::index::scan_library(library)?;
    let total = files.len();

    let mut routed: Vec<Pending> = Vec::new();
    let mut review: Vec<ReviewRow> = Vec::new();

    for (position, file) in files.iter().enumerate() {
        progress(position + 1, total);
        let ext = extension_of(&file.relative);
        // Files whose extension no included system accepts are not ROMs
        // for this profile (today's extension filter, unchanged).
        if !ext.is_empty() && !any_system_accepts_extension(&included, &ext) {
            continue;
        }

        // Tier 1/2: deterministic routing; the extension filter still
        // applies against the routed system.
        if let Some(system_id) = router.route(&file.relative) {
            let system = system_by_id(&included, system_id)
                .ok_or_else(|| format!("router produced unknown system {system_id}"))?;
            if extension_accepted(system, &ext) {
                let tier = if file
                    .relative
                    .iter()
                    .next()
                    .map(|first| {
                        let first = first.to_string_lossy().to_lowercase();
                        first == system.folder.to_lowercase() || first == system.id.to_lowercase()
                    })
                    .unwrap_or(false)
                {
                    Tier::Folder
                } else {
                    Tier::UniqueExtension
                };
                routed.push(Pending {
                    relative: file.relative.to_string_lossy().replace('\\', "/"),
                    size: file.size,
                    system_id: system_id.to_string(),
                    tier,
                });
                continue;
            }
        }

        // Tier 3: embedding nearest neighbor.
        let stem = tags::clean_stem_of(&file.relative);
        if let Ok(vector) = embed(&stem) {
            if let Some((entry_index, similarity)) = index.query(&vector) {
                let entry = &index.entries[entry_index];
                let known = system_by_id(&included, &entry.system_id);
                if similarity >= AUTO_ROUTE_SIM {
                    if let Some(system) = known {
                        if extension_accepted(system, &ext) {
                            routed.push(Pending {
                                relative: file.relative.to_string_lossy().replace('\\', "/"),
                                size: file.size,
                                system_id: entry.system_id.clone(),
                                tier: Tier::Embedding,
                            });
                            continue;
                        }
                    }
                }
                review.push(ReviewRow {
                    review_id: String::new(), // assigned after sorting below
                    relative: file.relative.to_string_lossy().replace('\\', "/"),
                    size: file.size,
                    nearest_stem: Some(entry.stem.clone()),
                    nearest_system: Some(entry.system_id.clone()),
                    similarity: Some(similarity),
                });
                continue;
            }
        }
        // No index, no hit, or embed failure: review with no suggestion.
        review.push(ReviewRow {
            review_id: String::new(),
            relative: file.relative.to_string_lossy().replace('\\', "/"),
            size: file.size,
            nearest_stem: None,
            nearest_system: None,
            similarity: None,
        });
    }

    // The scan order is already the total order (relative-path bytes);
    // ids are sequential and stable across runs.
    for (index, row) in review.iter_mut().enumerate() {
        row.review_id = format!("r{}", index + 1);
    }

    // Variant collapse.
    let (routes, groups) = collapse_variants(routed, regions);

    Ok(Classification {
        cold_start: index.entries.len() < COLD_START_TITLES,
        index_stats: IndexStats {
            titles: index.entries.len(),
            rebuilt: index_rebuilt,
            dim: index.dim,
        },
        routes,
        needs_review: review,
        groups,
    })
}

/// Groups routed files by (system, clean stem) and applies the region
/// preference: keep every file of the highest-priority region present,
/// skip other regions' variants; within the winning region keep one file
/// per disc slot with deterministic tie-breaks (shortest raw name, then
/// larger file, then lexicographic).
fn collapse_variants(
    routed: Vec<Pending>,
    regions: &[Region],
) -> (Vec<RouteRow>, Vec<VariantGroup>) {
    // Group by (system_id, clean stem of the file name).
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (position, file) in routed.iter().enumerate() {
        let stem = tags::clean_stem(&file.relative);
        groups
            .entry((file.system_id.clone(), stem))
            .or_default()
            .push(position);
    }

    let mut decisions: Vec<VariantDecision> = vec![VariantDecision::Keep; routed.len()];
    let mut group_views = Vec::new();

    let mut sorted_groups: Vec<(&(String, String), &Vec<usize>)> = groups.iter().collect();
    sorted_groups.sort_by(|a, b| (a.0).0.cmp(&(b.0).0).then((a.0).1.cmp(&(b.0).1)));

    for ((system_id, stem), members) in sorted_groups {
        let mut kept = 0usize;
        let mut skipped_variants = 0usize;
        let mut skipped_duplicates = 0usize;

        let region_of = |index: usize| tags::parse_region(&routed[index].relative);
        // Highest-priority region present (preference order); files with
        // no region tag fall back only when no preferred region exists.
        let mut winner: Option<Region> = None;
        for preferred in regions {
            if members
                .iter()
                .any(|&index| region_of(index) == Some(*preferred))
            {
                winner = Some(*preferred);
                break;
            }
        }
        let winner_matches = |index: usize| match (winner, region_of(index)) {
            (Some(w), Some(r)) => w == r,
            (None, _) => true, // no preferred region present: keep all, dedupe only
            (Some(_), None) => false,
        };

        // One file per disc slot among winners; tie-break deterministically.
        let mut slots: HashMap<Option<u8>, usize> = HashMap::new();
        let mut ordered: Vec<usize> = members.clone();
        ordered.sort_by(|&a, &b| {
            routed[a]
                .relative
                .len()
                .cmp(&routed[b].relative.len())
                .then(routed[b].size.cmp(&routed[a].size))
                .then(routed[a].relative.cmp(&routed[b].relative))
        });
        for &index in &ordered {
            if !winner_matches(index) {
                decisions[index] = VariantDecision::SkipRegion {
                    winner: winner.unwrap_or(Region::Usa),
                };
                skipped_variants += 1;
                continue;
            }
            let slot = tags::parse_disc(&routed[index].relative).map(|disc| disc.number);
            match slots.get(&slot) {
                Some(&kept_index) => {
                    decisions[index] = VariantDecision::SkipDuplicate {
                        kept: routed[kept_index].relative.clone(),
                    };
                    skipped_duplicates += 1;
                }
                None => {
                    slots.insert(slot, index);
                    kept += 1;
                }
            }
        }
        group_views.push(VariantGroup {
            system_id: system_id.clone(),
            stem: stem.clone(),
            kept,
            skipped_variants,
            skipped_duplicates,
        });
    }

    let routes = routed
        .into_iter()
        .zip(decisions)
        .map(|(file, variant)| RouteRow {
            relative: file.relative,
            size: file.size,
            system_id: file.system_id,
            tier: file.tier,
            variant,
        })
        .collect();
    (routes, group_views)
}

// ---------------------------------------------------------------------
// Review resolutions + copy-time guard
// ---------------------------------------------------------------------

/// A user review choice from the UI. `system_id` of None means skip.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub system_id: Option<String>,
    /// Preview-time file size, for the copy-time guard.
    pub size: u64,
}

/// Validates resolutions against a FRESH classification: every id must
/// exist, point at the same file (path + size) the user reviewed, and
/// name an included system (or skip). Returns id -> chosen system.
pub fn verify_resolutions(
    classification: &Classification,
    systems: &[SystemFolder],
    resolutions: &HashMap<String, Resolution>,
) -> Result<HashMap<String, Option<String>>, String> {
    let mut applied = HashMap::new();
    for (review_id, resolution) in resolutions {
        let Some(row) = classification
            .needs_review
            .iter()
            .find(|row| &row.review_id == review_id)
        else {
            return Err(format!(
                "review {review_id} does not match the current library — preview again"
            ));
        };
        if row.size != resolution.size {
            return Err(format!(
                "the library changed since Preview — preview again ({review_id} now has {} bytes, the review was for {})",
                row.size, resolution.size
            ));
        }
        if let Some(system_id) = &resolution.system_id {
            if system_by_id(systems, system_id).is_none() {
                return Err(format!(
                    "review {review_id} names unknown system {system_id}"
                ));
            }
        }
        applied.insert(review_id.clone(), resolution.system_id.clone());
    }
    Ok(applied)
}

// ---------------------------------------------------------------------
// Smart CopyPlan
// ---------------------------------------------------------------------

/// Destination-component validation, the `romcopy::ensure_safe_dest`
/// rules mirrored (the moat module itself stays untouched).
fn ensure_safe_dest(dest: &str) -> Result<(), String> {
    for part in dest.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return Err(format!("unsafe destination folder \"{dest}\""));
        }
    }
    Ok(())
}

fn safe_relative_dest(prefix: &str, sub_path: &str) -> Result<String, String> {
    ensure_safe_dest(prefix)?;
    for part in sub_path.split('/') {
        ensure_safe_dest(part)?;
    }
    if prefix.is_empty() {
        Ok(sub_path.to_string())
    } else {
        Ok(format!("{prefix}/{sub_path}"))
    }
}

/// Builds the smart CopyPlan: kept route rows + resolved review rows +
/// bios (mirroring plan_copy's bios handling). Skip-variant and
/// unresolved-review files are excluded — the UI surfaces them from the
/// classification, and unresolved reviews are an error before planning.
pub fn plan_smart(
    library: &Path,
    dest_root: &Path,
    layout: &str,
    systems: &[SystemFolder],
    classification: &Classification,
    applied: &HashMap<String, Option<String>>,
    bios_folder: Option<&str>,
) -> Result<CopyPlan, String> {
    let mut items = Vec::new();

    // A destination that already exists on the card is skipped
    // (plan_copy's SkipUnchanged semantics; execute_copy decides bytes).
    let action_for = |relative_dest: &str| {
        if dest_root.join(relative_dest).is_file() {
            CopyAction::SkipUnchanged
        } else {
            CopyAction::Copy
        }
    };

    let relative_to_sub = |relative: &str, tier: Tier, system: &SystemFolder| -> String {
        match tier {
            // Folder-routed files keep their subpath under the system
            // folder (mirrors collect_files).
            Tier::Folder => {
                let prefix = format!("{}/", system.folder);
                relative
                    .strip_prefix(&prefix)
                    .or_else(|| relative.strip_prefix(&format!("{}/", system.id)))
                    .unwrap_or(relative)
                    .to_string()
            }
            // Loose files land flat under the system folder.
            _ => relative.rsplit('/').next().unwrap_or(relative).to_string(),
        }
    };

    for route in &classification.routes {
        if route.variant != VariantDecision::Keep {
            continue;
        }
        let Some(system) = system_by_id(systems, &route.system_id) else {
            return Err(format!(
                "classification routed {} to unknown system {}",
                route.relative, route.system_id
            ));
        };
        let prefix = storage_folder(layout, &system.folder)?;
        let sub = relative_to_sub(&route.relative, route.tier, system);
        let relative_dest = safe_relative_dest(&prefix, &sub)?;
        items.push(CopyItem {
            source: library.join(&route.relative),
            action: action_for(&relative_dest),
            relative_dest,
            bytes: route.size,
        });
    }

    for row in &classification.needs_review {
        let Some(choice) = applied.get(&row.review_id) else {
            return Err(format!(
                "{} file(s) still need review before planning",
                classification.needs_review.len() - applied.len()
            ));
        };
        let Some(system_id) = choice else {
            continue; // user chose skip
        };
        let Some(system) = system_by_id(systems, system_id) else {
            return Err(format!(
                "review {} names unknown system {system_id}",
                row.review_id
            ));
        };
        let prefix = storage_folder(layout, &system.folder)?;
        let sub = row
            .relative
            .rsplit('/')
            .next()
            .unwrap_or(&row.relative)
            .to_string();
        let relative_dest = safe_relative_dest(&prefix, &sub)?;
        items.push(CopyItem {
            source: library.join(&row.relative),
            action: action_for(&relative_dest),
            relative_dest,
            bytes: row.size,
        });
    }

    if let Some(bios_folder) = bios_folder {
        // find_named_dir semantics: case-insensitive top-level match.
        let bios_source = std::fs::read_dir(library).ok().and_then(|entries| {
            entries.flatten().map(|entry| entry.path()).find(|path| {
                path.is_dir()
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.eq_ignore_ascii_case("bios"))
                        .unwrap_or(false)
            })
        });
        if let Some(bios_source) = bios_source {
            let prefix = storage_folder(layout, bios_folder)?;
            let mut stack = vec![bios_source.clone()];
            while let Some(dir) = stack.pop() {
                let entries = std::fs::read_dir(&dir)
                    .map_err(|error| format!("could not read {}: {error}", dir.display()))?;
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    let sub = path
                        .strip_prefix(&bios_source)
                        .unwrap_or(path.as_path())
                        .to_string_lossy()
                        .replace('\\', "/");
                    let relative_dest = safe_relative_dest(&prefix, &sub)?;
                    items.push(CopyItem {
                        source: path,
                        action: action_for(&relative_dest),
                        relative_dest,
                        bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
                    });
                }
            }
        }
    }

    Ok(CopyPlan {
        items,
        warning: None,
    })
}
