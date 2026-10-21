pub mod backup;
pub mod block_device;
pub mod detect;
pub mod diag;
pub mod feed;
pub mod firstboot;
pub mod flash;
pub mod folder_map;
pub mod format_gate;
pub mod fullcheck;
pub mod igir;
pub mod license;
pub mod needle;
pub mod prepare;
pub mod probe;
pub mod profiles;
pub mod restore;
pub mod romcopy;
pub mod seed;
pub mod store;
pub mod volume;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use profiles::Profile;
use romcopy::CopyPlan;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use volume::{decide, VolumeDecision, VolumeInfo};

fn repo_root() -> PathBuf {
    if let Ok(root) = std::env::var("CFW_STUDIO_ROOT") {
        return PathBuf::from(root);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

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
        candidates.push(
            PathBuf::from(root)
                .join("specs")
                .join("profile.schema.json"),
        );
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

fn profile_by_id(app: &tauri::AppHandle, id: &str) -> Result<Profile, String> {
    load_all(app)?
        .into_iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| format!("unknown profile {id}"))
}

fn require_volume(volume_id: &str) -> Result<VolumeInfo, String> {
    volume::list_volumes()?
        .into_iter()
        .find(|volume| volume.id == volume_id)
        .ok_or_else(|| format!("removable drive {volume_id} is not connected"))
}

fn card_root(volume: &VolumeInfo) -> PathBuf {
    PathBuf::from(format!("{}:\\", volume.letter))
}

/// One lettered partition of a disk, from `Get-Partition`.
#[derive(Debug, Clone, Copy)]
pub struct LetteredPartition {
    pub letter: char,
    pub disk: u32,
}

fn lettered_partitions() -> Result<Vec<LetteredPartition>, String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Partition | Where-Object { $_.DriveLetter } | Select-Object DriveLetter, DiskNumber | ConvertTo-Json -Compress",
        ])
        .output()
        .map_err(|error| format!("could not list partitions: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_lettered_partitions(&text)
}

fn parse_lettered_partitions(text: &str) -> Result<Vec<LetteredPartition>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("could not read the partition list: {error}"))?;
    let rows = if value.is_array() {
        value.as_array().cloned().unwrap_or_default()
    } else {
        vec![value]
    };
    let mut partitions = Vec::new();
    for row in rows {
        let Some(letter) = row
            .get("DriveLetter")
            .and_then(|value| value.as_str())
            .and_then(|text| text.chars().next())
        else {
            continue;
        };
        let disk = row
            .get("DiskNumber")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as u32;
        if disk == 0 {
            continue;
        }
        partitions.push(LetteredPartition { letter, disk });
    }
    Ok(partitions)
}

fn disk_number_of(letter: &str, partitions: &[LetteredPartition]) -> u32 {
    let Some(want) = letter.chars().next() else {
        return 0;
    };
    partitions
        .iter()
        .find(|row| row.letter.eq_ignore_ascii_case(&want))
        .map(|row| row.disk)
        .unwrap_or(0)
}

/// Judge whether ROMs may be copied onto this card for this profile.
///
/// Only flashed single-card ArkOS/dArkOS profiles carry the firstboot wipe;
/// ROMs-only cards have no BOOT partition and are always safe to fill.
fn check_card_safety(
    profile: &Profile,
    volume: &VolumeInfo,
) -> Result<firstboot::CardSafety, String> {
    let gate_applies =
        profile.image.is_some() && profile.rom_schema.layout == "arkos_easyroms_root";
    if !gate_applies {
        return Ok(firstboot::CardSafety::Safe);
    }
    let partitions = lettered_partitions()?;
    let Some(disk) = partitions
        .iter()
        .find(|row| {
            row.letter
                .eq_ignore_ascii_case(&volume.letter.chars().next().unwrap_or('?'))
        })
        .map(|row| row.disk)
    else {
        return Ok(firstboot::CardSafety::Unknown(format!(
            "could not find which disk holds {}:. Reinsert the card and refresh.",
            volume.letter
        )));
    };
    let volumes = volume::list_volumes()?;
    let disk_volumes: firstboot::DiskVolumes = partitions
        .iter()
        .filter(|row| row.disk == disk)
        .filter_map(|row| {
            volumes
                .iter()
                .find(|volume| volume.letter.starts_with(row.letter))
                .map(|volume| (row.letter, volume.label.clone()))
        })
        .collect();
    let boot_root =
        firstboot::boot_letter(&disk_volumes).map(|letter| PathBuf::from(format!("{letter}:\\")));
    Ok(firstboot::card_safety(&disk_volumes, boot_root.as_deref()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FirstbootStateView {
    state: String,
    reason: String,
}

#[tauri::command]
fn firstboot_state(
    app: tauri::AppHandle,
    profile_id: String,
    volume_id: String,
) -> Result<FirstbootStateView, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let volume = require_volume(&volume_id)?;
    let (state, reason) = match check_card_safety(&profile, &volume)? {
        firstboot::CardSafety::Safe => ("safe".into(), String::new()),
        firstboot::CardSafety::Armed(reason) => {
            diag::log(
                "WARN",
                "firstboot_refused",
                &format!("volume={}: {reason}", volume.letter),
            );
            ("armed".into(), reason)
        }
        firstboot::CardSafety::Unknown(reason) => {
            diag::log(
                "WARN",
                "firstboot_unverified",
                &format!("volume={}: {reason}", volume.letter),
            );
            ("unknown".into(), reason)
        }
    };
    Ok(FirstbootStateView { state, reason })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VolumeView {
    id: String,
    letter: String,
    label: String,
    file_system: String,
    total_bytes: u64,
    is_empty: bool,
    decision: String,
    relabel: bool,
    reason: String,
    disk_number: u32,
}

fn view_volume(volume: VolumeInfo, expected_label: &str, disk_number: u32) -> VolumeView {
    let decision = decide(&volume, expected_label);
    let (decision_name, relabel, reason) = match decision {
        VolumeDecision::Ready { relabel } => ("ready", relabel, String::new()),
        VolumeDecision::NeedsFormat { reason } => ("needs_format", false, reason),
        VolumeDecision::Rejected { reason } => ("rejected", false, reason),
    };
    VolumeView {
        id: volume.id,
        letter: volume.letter,
        label: volume.label,
        file_system: volume.file_system,
        total_bytes: volume.total_bytes,
        is_empty: volume.is_empty,
        decision: decision_name.into(),
        relabel,
        reason,
        disk_number,
    }
}

#[derive(Serialize)]
struct SeedResult {
    folders: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanFile {
    relative_dest: String,
    bytes: u64,
    action: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanView {
    files: Vec<PlanFile>,
    warning: Option<String>,
    copy_count: usize,
    skip_count: usize,
    duplicates: Vec<needle::dedupe::DuplicateGroup>,
    duplicate_skipped: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CopyProgress {
    index: usize,
    total: usize,
    relative_dest: String,
    done: bool,
}

#[tauri::command]
fn detect_stock_device(app: tauri::AppHandle) -> Result<detect::DetectionView, String> {
    let profiles = load_all(&app)?;
    let detection = match detect::scan_observations() {
        Ok(observations) => detect::identify(&profiles, &observations),
        Err(error) => {
            diag::log("WARN", "detect_failed", &error);
            detect::Detection::None
        }
    };
    if let detect::Detection::Match {
        profile_id,
        device,
        screen,
    } = &detection
    {
        diag::log(
            "INFO",
            "detect_match",
            &format!("profile={profile_id} device={device} screen={screen}"),
        );
    }
    Ok(detect::view(&profiles, detection))
}

#[tauri::command]
fn list_profiles(app: tauri::AppHandle) -> Result<Vec<Profile>, String> {
    let mut profiles = load_all(&app)?;
    profiles.retain(|profile| {
        profile.storage_modes.iter().any(|mode| {
            mode == "roms_card_only" || (mode == "single_card" && profile.image.is_some())
        })
    });
    Ok(profiles)
}

#[tauri::command]
fn list_volumes(app: tauri::AppHandle, profile_id: String) -> Result<Vec<VolumeView>, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let label = profile.rom_schema.volume_label.unwrap_or_default();
    let volumes = volume::list_volumes()?;
    let partitions = lettered_partitions()?;
    Ok(volumes
        .into_iter()
        .map(|volume| {
            let disk_number = disk_number_of(&volume.letter, &partitions);
            view_volume(volume, &label, disk_number)
        })
        .collect())
}

#[tauri::command]
fn prepare_card(
    app: tauri::AppHandle,
    profile_id: String,
    volume_id: String,
    confirmation: String,
    displayed_bytes: u64,
) -> Result<VolumeView, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let label = profile
        .rom_schema
        .volume_label
        .clone()
        .unwrap_or_else(|| "EASYROMS".into());
    let volume = require_volume(&volume_id)?;
    let file_system = profile.rom_schema.format_fs.as_deref().unwrap_or("exFAT");
    let mut shell = prepare::ElevatedPowerShell;
    diag::log(
        "INFO",
        "prepare_start",
        &format!(
            "volume={}: label={label} size={} fs={file_system}",
            volume.letter, volume.total_bytes
        ),
    );
    if let Err(error) = prepare::prepare_volume(
        &volume,
        &label,
        &confirmation,
        displayed_bytes,
        file_system,
        &mut shell,
    ) {
        diag::log(
            "ERROR",
            "prepare_failed",
            &format!("volume={}: {error}", volume.letter),
        );
        return Err(error);
    }
    diag::log("INFO", "prepare_done", &format!("volume={}", volume.letter));
    let refreshed = require_volume(&volume_id).unwrap_or(volume);
    let partitions = lettered_partitions().unwrap_or_default();
    let disk_number = disk_number_of(&refreshed.letter, &partitions);
    Ok(view_volume(refreshed, &label, disk_number))
}

#[tauri::command]
fn seed_card(
    app: tauri::AppHandle,
    profile_id: String,
    volume_id: String,
) -> Result<SeedResult, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let label = profile.rom_schema.volume_label.clone().unwrap_or_default();
    let volume = require_volume(&volume_id)?;
    let decision = decide(&volume, &label);
    let folders = folder_map::folders_for(&profile)?;
    let created = seed::seed_folders(&card_root(&volume), &folders, &decision)?;
    Ok(SeedResult { folders: created })
}

fn build_plan(
    profile: &Profile,
    volume: &VolumeInfo,
    library: &Path,
    include: &[String],
) -> Result<CopyPlan, String> {
    romcopy::plan_copy(
        library,
        &card_root(volume),
        &profile.rom_schema.layout,
        &profile.rom_schema.systems,
        include,
        profile.rom_schema.bios_folder.as_deref(),
    )
}

// ---------------------------------------------------------------------
// Smart sort (needle PR 4): classify + smart payload on plan/copy
// ---------------------------------------------------------------------

/// Optional smart-sort payload on plan_roms/copy_roms: region preference
/// plus the user's review resolutions (opaque ids, preview-time sizes
/// for the copy-time guard).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SmartPayload {
    #[serde(default)]
    regions: Vec<String>,
    #[serde(default)]
    resolutions: std::collections::HashMap<String, needle::sort::Resolution>,
}

/// Optional dedupe payload on plan_roms/copy_roms: per-group keep/skip
/// choices keyed by incoming relative_dest. Absent groups default to
/// skip incoming (residents are never touched).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DedupePayload {
    #[serde(default)]
    keep: Vec<String>,
}

fn parse_regions(raw: &[String]) -> Result<Vec<needle::tags::Region>, String> {
    if raw.is_empty() {
        return Ok(vec![needle::tags::Region::Usa]);
    }
    raw.iter()
        .map(|region| match region.to_ascii_uppercase().as_str() {
            "USA" => Ok(needle::tags::Region::Usa),
            "EUR" | "EUROPE" => Ok(needle::tags::Region::Europe),
            "JPN" | "JAPAN" => Ok(needle::tags::Region::Japan),
            "WORLD" => Ok(needle::tags::Region::World),
            other => Err(format!("unknown region preference {other:?}")),
        })
        .collect()
}

/// Resolves the cfw-embed helper: explicit override, beside the app,
/// then the needle store cache.
fn resolve_embed_helper() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CFW_EMBED_EXE") {
        let path = PathBuf::from(path.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("cfw-embed.exe")))?;
    if beside.is_file() {
        return Some(beside);
    }
    let cached = needle::acquire::cache_directories()
        .iter()
        .map(|dir| dir.join("cfw-embed.exe"))
        .find(|path| path.is_file());
    cached
}

/// Resolves verified weights from the needle cache (never downloads —
/// downloads happen only through needle_acquire behind consent).
fn resolve_weights() -> Option<PathBuf> {
    let weights = needle::manifest::find("weights")?;
    needle::acquire::cache_directories()
        .iter()
        .map(|dir| dir.join(weights.file_name))
        .find(|path| {
            path.is_file()
                && std::fs::File::open(path)
                    .ok()
                    .and_then(|file| flash::sha256_reader(file).ok())
                    .map(|hash| hash.eq_ignore_ascii_case(weights.sha256))
                    .unwrap_or(false)
        })
}

/// Runs classify + index ensure inside one helper session. Emits
/// classify-progress and returns the classification.
fn classify_with_engine(
    app: &tauri::AppHandle,
    profile: &Profile,
    library: &Path,
    include: &[String],
    regions: &[needle::tags::Region],
) -> Result<needle::sort::Classification, String> {
    let helper =
        resolve_embed_helper().ok_or_else(|| "smart sort engine is not installed".to_string())?;
    let weights =
        resolve_weights().ok_or_else(|| "smart sort engine is not installed".to_string())?;
    let weights_tag = needle::manifest::find("weights")
        .map(|weights| weights.sha256)
        .unwrap_or("unknown");

    needle::embed_client::with_helper(&helper, &weights, |session| {
        let mut embed = |stem: &str| session.embed(stem);
        let (index, rebuilt) = needle::index::ensure_index(
            &profile.id,
            library,
            &profile.rom_schema.systems,
            profile.rom_schema.bios_folder.as_deref(),
            weights_tag,
            &mut embed,
        )?;
        let classification = needle::sort::classify(
            library,
            &profile.rom_schema.systems,
            include,
            profile.rom_schema.bios_folder.as_deref(),
            &index,
            rebuilt,
            regions,
            &mut embed,
            &mut |done, total| {
                let _ = app.emit("classify-progress", format!("{done}/{total}"));
            },
        )?;
        let routed = classification
            .routes
            .iter()
            .filter(|r| r.variant == needle::sort::VariantDecision::Keep)
            .count();
        let review = classification.needs_review.len();
        let skipped = classification.routes.len() - routed;
        diag::log(
            "info",
            "needle_classify_done",
            &format!("routed={routed} review={review} unmatched={skipped}"),
        );
        Ok(classification)
    })
}

#[tauri::command]
fn classify_library(
    app: tauri::AppHandle,
    profile_id: String,
    library: String,
    include: Vec<String>,
    regions: Vec<String>,
) -> Result<needle::sort::Classification, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let regions = parse_regions(&regions)?;
    classify_with_engine(&app, &profile, Path::new(&library), &include, &regions)
}

/// Shared smart path for plan_roms/copy_roms: classify, verify
/// resolutions against the fresh classification (the copy-time guard),
/// and build the smart CopyPlan.
fn smart_plan(
    app: &tauri::AppHandle,
    profile: &Profile,
    volume: &VolumeInfo,
    library: &Path,
    include: &[String],
    smart: &SmartPayload,
) -> Result<CopyPlan, String> {
    let regions = parse_regions(&smart.regions)?;
    let classification = classify_with_engine(app, profile, library, include, &regions)?;
    let applied = needle::sort::verify_resolutions(
        &classification,
        &profile.rom_schema.systems,
        &smart.resolutions,
    )?;
    let plan = needle::sort::plan_smart(
        library,
        &card_root(volume),
        &profile.rom_schema.layout,
        &profile.rom_schema.systems,
        &classification,
        &applied,
        profile.rom_schema.bios_folder.as_deref(),
    )?;
    let variants = classification
        .routes
        .iter()
        .filter(|r| r.variant != needle::sort::VariantDecision::Keep)
        .count();
    diag::log(
        "info",
        "needle_copy_smart",
        &format!("files={} variants_skipped={variants}", plan.items.len()),
    );
    Ok(plan)
}

/// Dedupe overlay shared by plan and copy: scans the card, groups plan
/// items against residents (smart path only — embeddings through the
/// helper), and removes grouped items unless kept. Returns the plan
/// view with groups for preview; copy applies the same removal and
/// previews it through the view only.
fn dedupe_plan(
    app: &tauri::AppHandle,
    profile: &Profile,
    dest_root: &Path,
    plan: CopyPlan,
    dedupe: Option<&DedupePayload>,
) -> Result<PlanView, String> {
    let _ = app;
    let residents = needle::dedupe::scan_card(
        dest_root,
        &profile.rom_schema.layout,
        &profile.rom_schema.systems,
    );
    let incoming = needle::dedupe::plan_incoming(
        &plan,
        &profile.rom_schema.layout,
        &profile.rom_schema.systems,
    );
    if residents.is_empty() || incoming.is_empty() {
        return Ok(plan_view(plan));
    }
    let helper =
        resolve_embed_helper().ok_or_else(|| "smart sort engine is not installed".to_string())?;
    let weights =
        resolve_weights().ok_or_else(|| "smart sort engine is not installed".to_string())?;
    needle::embed_client::with_helper(&helper, &weights, |session| {
        let mut embed = |stem: &str| session.embed(stem);
        let groups = needle::dedupe::group_duplicates(&residents, &incoming, &mut embed);
        let keep: std::collections::HashSet<String> = dedupe
            .map(|d| d.keep.iter().cloned().collect())
            .unwrap_or_default();
        let (filtered, removed) = needle::dedupe::apply_keep_choices(plan, &groups, &keep);
        diag::log(
            "info",
            "needle_plan_dedupe",
            &format!(
                "groups={} duplicate_incoming_skipped={removed}",
                groups.len()
            ),
        );
        Ok(plan_view_with_duplicates(filtered, groups, removed))
    })
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn plan_roms(
    app: tauri::AppHandle,
    profile_id: String,
    volume_id: String,
    library: String,
    include: Vec<String>,
    smart: Option<SmartPayload>,
    dedupe: Option<DedupePayload>,
) -> Result<PlanView, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let volume = require_volume(&volume_id)?;
    let plan = match &smart {
        Some(smart) => smart_plan(
            &app,
            &profile,
            &volume,
            Path::new(&library),
            &include,
            smart,
        )?,
        None => build_plan(&profile, &volume, Path::new(&library), &include)?,
    };
    // Dedupe overlay: group plan items against what is already on the
    // card (smart path embeds through the helper; the plain path has no
    // embeddings, so it previews no groups). Grouped items default to
    // skip unless listed in dedupe.keep; residents are never modified.
    if smart.is_some() {
        let with_groups = dedupe_plan(&app, &profile, &card_root(&volume), plan, dedupe.as_ref())?;
        return Ok(with_groups);
    }
    Ok(plan_view(plan))
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn copy_roms(
    app: tauri::AppHandle,
    profile_id: String,
    volume_id: String,
    library: String,
    include: Vec<String>,
    dry_run: bool,
    smart: Option<SmartPayload>,
    dedupe: Option<DedupePayload>,
) -> Result<romcopy::CopyReport, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let volume = require_volume(&volume_id)?;
    let label = profile.rom_schema.volume_label.clone().unwrap_or_default();
    volume::allow_copy(&volume, &label)?;
    if let firstboot::CardSafety::Armed(reason) | firstboot::CardSafety::Unknown(reason) =
        check_card_safety(&profile, &volume)?
    {
        diag::log(
            "WARN",
            "firstboot_copy_refused",
            &format!("volume={}: {reason}", volume.letter),
        );
        return Err(format!(
            "refusing to copy onto this card: {reason}. Boot the handheld once until the game menu appears, shut it down from the menu, and put the card back."
        ));
    }
    // Smart copies re-classify here (embeddings are deterministic) and
    // verify the review resolutions still match the library — the
    // copy-time guard — before the unchanged moat takes over. Dedupe
    // then removes grouped incoming files (default skip) from the plan;
    // residents are never touched.
    let mut plan = match &smart {
        Some(smart) => smart_plan(
            &app,
            &profile,
            &volume,
            Path::new(&library),
            &include,
            smart,
        )?,
        None => build_plan(&profile, &volume, Path::new(&library), &include)?,
    };
    if smart.is_some() {
        let keep: std::collections::HashSet<String> = dedupe
            .as_ref()
            .map(|d| d.keep.iter().cloned().collect())
            .unwrap_or_default();
        let residents = needle::dedupe::scan_card(
            &card_root(&volume),
            &profile.rom_schema.layout,
            &profile.rom_schema.systems,
        );
        let incoming = needle::dedupe::plan_incoming(
            &plan,
            &profile.rom_schema.layout,
            &profile.rom_schema.systems,
        );
        if !residents.is_empty() && !incoming.is_empty() {
            let helper = resolve_embed_helper()
                .ok_or_else(|| "smart sort engine is not installed".to_string())?;
            let weights = resolve_weights()
                .ok_or_else(|| "smart sort engine is not installed".to_string())?;
            let filtered = needle::embed_client::with_helper(&helper, &weights, |session| {
                let mut embed = |stem: &str| session.embed(stem);
                let groups = needle::dedupe::group_duplicates(&residents, &incoming, &mut embed);
                let (filtered, removed) = needle::dedupe::apply_keep_choices(plan, &groups, &keep);
                diag::log(
                    "info",
                    "needle_copy_dedupe",
                    &format!("duplicate_incoming_skipped={removed}"),
                );
                Ok(filtered)
            })?;
            plan = filtered;
        }
    }
    diag::log(
        "INFO",
        "copy_start",
        &format!(
            "profile={profile_id} volume={} files={} dry_run={dry_run}",
            volume.letter,
            plan.items.len()
        ),
    );
    let root = card_root(&volume);
    let total = plan.items.len();
    let mut copied = 0;
    let mut skipped = 0;
    let mut bytes_copied = 0;
    for (index, item) in plan.items.iter().enumerate() {
        let _ = app.emit(
            "copy-progress",
            CopyProgress {
                index,
                total,
                relative_dest: item.relative_dest.clone(),
                done: false,
            },
        );
        let one = CopyPlan {
            items: vec![item.clone()],
            warning: None,
        };
        let report = match romcopy::execute_copy(&one, &root, dry_run) {
            Ok(report) => report,
            Err(error) => {
                diag::log(
                    "ERROR",
                    "copy_failed",
                    &format!("profile={profile_id} volume={}: {error}", volume.letter),
                );
                return Err(error);
            }
        };
        copied += report.copied;
        skipped += report.skipped;
        bytes_copied += report.bytes_copied;
    }
    let _ = app.emit(
        "copy-progress",
        CopyProgress {
            index: total,
            total,
            relative_dest: String::new(),
            done: true,
        },
    );
    diag::log(
        "INFO",
        "copy_done",
        &format!(
            "profile={profile_id} volume={} copied={copied} skipped={skipped} bytes={bytes_copied}",
            volume.letter
        ),
    );
    Ok(romcopy::CopyReport {
        copied,
        skipped,
        bytes_copied,
    })
}

fn plan_view(plan: CopyPlan) -> PlanView {
    plan_view_with_duplicates(plan, Vec::new(), 0)
}

/// Renders a plan plus its dedupe overlay: the groups previewed and how
/// many grouped incoming files were removed (default skip).
fn plan_view_with_duplicates(
    plan: CopyPlan,
    duplicates: Vec<needle::dedupe::DuplicateGroup>,
    duplicate_skipped: usize,
) -> PlanView {
    let copy_count = plan
        .items
        .iter()
        .filter(|item| item.action == romcopy::CopyAction::Copy)
        .count();
    let skip_count = plan.items.len() - copy_count;
    PlanView {
        files: plan
            .items
            .into_iter()
            .map(|item| PlanFile {
                relative_dest: item.relative_dest,
                bytes: item.bytes,
                action: match item.action {
                    romcopy::CopyAction::Copy => "copy".into(),
                    romcopy::CopyAction::SkipUnchanged => "skip".into(),
                },
            })
            .collect(),
        warning: plan.warning,
        copy_count,
        skip_count,
        duplicates,
        duplicate_skipped,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FlashDiskView {
    number: u32,
    name: String,
    size_bytes: u64,
}

#[tauri::command]
fn list_flash_disks() -> Result<Vec<FlashDiskView>, String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Disk | Where-Object { $_.BusType -eq 'USB' -and $_.Number -ne 0 } | Select-Object Number, FriendlyName, Size | ConvertTo-Json -Compress",
        ])
        .output()
        .map_err(|error| format!("could not list disks: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("could not read the disk list: {error}"))?;
    let rows = if value.is_array() {
        value.as_array().cloned().unwrap_or_default()
    } else {
        vec![value]
    };
    let mut disks = Vec::new();
    for row in rows {
        let number = row.get("Number").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let size_bytes = row.get("Size").and_then(|v| v.as_u64()).unwrap_or(0);
        if number == 0 || size_bytes == 0 {
            continue;
        }
        disks.push(FlashDiskView {
            number,
            name: row
                .get("FriendlyName")
                .and_then(|v| v.as_str())
                .unwrap_or("USB disk")
                .to_string(),
            size_bytes,
        });
    }
    Ok(disks)
}

fn image_cache_dirs() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Ok(dir) = std::env::var("CFW_IMAGE_DIR") {
        directories.push(PathBuf::from(dir));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        directories.push(PathBuf::from(local).join("cfw-card-studio").join("images"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let home = PathBuf::from(home).join("Downloads");
        directories.push(home.join("rocknix-rgb10x"));
        directories.push(home.join("darkos-rg351mp"));
    }
    if directories.is_empty() {
        directories.push(std::env::temp_dir().join("cfw-card-studio").join("images"));
    }
    directories
}

fn prepare_split_7z(
    app: &tauri::AppHandle,
    image: &profiles::ImageSource,
) -> Result<PathBuf, String> {
    let first = image
        .parts
        .first()
        .ok_or("this 7z image has no download parts")?;
    let image_name = flash::extracted_image_name(&flash::image_file_name(&first.url)?)?;
    let directories = image_cache_dirs();
    for directory in &directories {
        let candidate = directory.join(&image_name);
        if candidate.is_file() {
            let hash =
                flash::sha256_reader(std::fs::File::open(&candidate).map_err(|error| {
                    format!("could not read {}: {error}", candidate.display())
                })?)?;
            if hash.eq_ignore_ascii_case(&image.sha256) {
                return Ok(candidate);
            }
        }
    }
    let mut part_paths = Vec::new();
    for part in &image.parts {
        let part_url = part.url.clone();
        let _ = app.emit("flash-progress", "Downloading an OS image part…");
        part_paths.push(flash::ensure_image(
            &directories,
            &part.url,
            &part.sha256,
            |dest| download_image(&part_url, dest),
        )?);
    }
    let extract_dir = part_paths[0]
        .parent()
        .ok_or("image part has no folder")?
        .to_path_buf();
    let _ = app.emit("flash-progress", "Unpacking the OS image…");
    let status = std::process::Command::new(seven_zip()?)
        .args([
            "x",
            &format!("-o{}", extract_dir.display()),
            "-y",
            part_paths[0].to_str().ok_or("image path is not text")?,
        ])
        .status()
        .map_err(|error| format!("could not unpack the image: {error}"))?;
    if !status.success() {
        return Err("7-Zip could not unpack the OS image".into());
    }
    let extracted = extract_dir.join(&image_name);
    let hash = flash::sha256_reader(
        std::fs::File::open(&extracted)
            .map_err(|error| format!("could not read {}: {error}", extracted.display()))?,
    )?;
    if !hash.eq_ignore_ascii_case(&image.sha256) {
        return Err("unpacked OS image did not match the published checksum".into());
    }
    Ok(extracted)
}

fn seven_zip() -> Result<PathBuf, String> {
    for candidate in [
        r"C:\Program Files\7-Zip\7z.exe",
        r"C:\Program Files (x86)\7-Zip\7z.exe",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err("7-Zip is required to unpack this OS image".into())
}

fn download_image(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let response = ureq::get(url)
        .call()
        .map_err(|error| format!("could not download the image: {error}"))?;
    let mut file = std::fs::File::create(dest)
        .map_err(|error| format!("could not save the download: {error}"))?;
    std::io::copy(&mut response.into_reader(), &mut file)
        .map_err(|error| format!("could not save the download: {error}"))?;
    Ok(())
}

fn flash_tool() -> Result<PathBuf, String> {
    let exe =
        std::env::current_exe().map_err(|error| format!("could not find this app: {error}"))?;
    let tool = exe
        .parent()
        .ok_or("app path has no folder")?
        .join("cfw-flash.exe");
    if tool.is_file() {
        Ok(tool)
    } else {
        Err(format!("flash tool is missing at {}", tool.display()))
    }
}

#[tauri::command]
fn flash_os(
    app: tauri::AppHandle,
    profile_id: String,
    disk_number: u32,
    confirmation: String,
) -> Result<String, String> {
    if confirmation != "FLASH" {
        return Err("type FLASH to erase the selected USB disk".into());
    }
    if disk_number == 0 {
        return Err("refusing to flash disk 0".into());
    }
    let profile = profile_by_id(&app, &profile_id)?;
    let image = profile.image.ok_or("this profile has no OS image")?;
    let _ = app.emit("flash-progress", "Checking the OS image…");
    let image_path = if image.compressed.as_deref() == Some("7z") {
        prepare_split_7z(&app, &image)?
    } else {
        let image_url = image.url.clone();
        flash::ensure_image(&image_cache_dirs(), &image.url, &image.sha256, |dest| {
            let _ = app.emit("flash-progress", "Downloading the OS image…");
            download_image(&image_url, dest)
        })?
    };
    let _ = app.emit(
        "flash-progress",
        "Image checksum matched. Waiting for approval, then writing and reading the card back.",
    );
    diag::log(
        "INFO",
        "flash_start",
        &format!("disk={disk_number} profile={profile_id}"),
    );
    let tool = flash_tool()?;
    // Unpredictable names: a same-user process must not be able to pre-place
    // or swap the elevated script between write and launch.
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );
    let log = std::env::temp_dir().join(format!("cfw-flash-ui-{unique}.txt"));
    let script_path = std::env::temp_dir().join(format!("cfw-flash-ui-{unique}.ps1"));
    restore::write_new_nofollow(&log, b"")
        .map_err(|error| format!("could not prepare the flash log: {error}"))?;
    let script = format!(
        "& '{tool}' --image='{image}' --sha256='{sha}' --disk={disk_number} --confirm='{confirmation}' --log-file='{log}'\r\nexit $LASTEXITCODE\r\n",
        tool = tool.display().to_string().replace('\'', "''"),
        image = image_path.display().to_string().replace('\'', "''"),
        sha = image.sha256.replace('\'', ""),
        log = log.display().to_string().replace('\'', "''"),
        confirmation = confirmation.replace('\'', ""),
    );
    restore::write_new_nofollow(&script_path, script.as_bytes())
        .map_err(|error| format!("could not write the launcher: {error}"))?;
    let launcher = format!(
        "Start-Process -FilePath powershell.exe -Verb RunAs -Wait -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{}'",
        script_path.display().to_string().replace('\'', "''")
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &launcher])
        .status()
        .map_err(|error| format!("could not start the flash tool: {error}"))?;
    // The launcher is "-Wait", so the elevated run has finished; remove the
    // script so no predictable elevated artifact lingers in %TEMP%.
    let _ = std::fs::remove_file(&script_path);
    let text = restore::read_nofollow_string(&log).unwrap_or_default();
    if !status.success() || !text.contains("RESULT=OK") {
        let reason = if text.trim().is_empty() {
            "the flash did not finish. The approval prompt may have been dismissed.".to_string()
        } else {
            text
        };
        diag::log(
            "ERROR",
            "flash_failed",
            &format!("disk={disk_number} profile={profile_id}: {reason}"),
        );
        return Err(reason);
    }
    diag::log(
        "INFO",
        "flash_done",
        &format!("disk={disk_number} profile={profile_id}"),
    );
    Ok(text)
}

static STOCK_BACKUP_RUNNING: AtomicBool = AtomicBool::new(false);
static STOCK_BACKUP_CANCEL: AtomicBool = AtomicBool::new(false);

struct StockBackupGuard;

impl Drop for StockBackupGuard {
    fn drop(&mut self) {
        STOCK_BACKUP_RUNNING.store(false, Ordering::SeqCst);
    }
}

fn begin_stock_backup() -> Result<StockBackupGuard, String> {
    if PROBE_RUNNING.load(Ordering::SeqCst) || FULL_RUNNING.load(Ordering::SeqCst) {
        return Err("a card test is already running".into());
    }
    if RESTORE_RUNNING.load(Ordering::SeqCst) {
        return Err("a backup restore is already running".into());
    }
    if STOCK_BACKUP_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("a stock-card backup is already running".into());
    }
    STOCK_BACKUP_CANCEL.store(false, Ordering::SeqCst);
    Ok(StockBackupGuard)
}

/// Read-only on purpose. Do not add `.write(true)`: a stock-card backup must
/// not be able to change the card. The wipe path stays in `cfw-flash` and is
/// not called from here. Tests never call this; they use fixture files.
fn open_stock_disk_read_only(number: u32) -> Result<std::fs::File, String> {
    if number == 0 {
        return Err("refusing to open disk 0".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(format!(r"\\.\PhysicalDrive{number}"))
            .map_err(|error| {
                format!(
                    "could not open disk {number} read-only ({error}). If Windows refused access, run Card Studio as administrator and try again. The card was not written."
                )
            })
    }
    #[cfg(not(windows))]
    {
        Err(format!(
            "reading physical disk {number} is implemented for Windows; the card was not opened"
        ))
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupReportView {
    status: String,
    image: String,
    sidecar: String,
    sha256: String,
    partial: String,
    bytes: u64,
}

#[tauri::command]
fn backup_stock_card(
    app: tauri::AppHandle,
    disk_number: u32,
    displayed_bytes: u64,
    destination: String,
) -> Result<BackupReportView, String> {
    let _guard = begin_stock_backup()?;
    if disk_number == 0 {
        return Err("refusing to open disk 0".into());
    }
    let destination = PathBuf::from(&destination);
    backup::check_destination(&destination).map_err(|error| error.to_string())?;
    let info = query_restore_disk(disk_number)?;
    let disk = backup::BackupDisk {
        number: info.number,
        bus: info.bus.clone(),
        size_bytes: info.size_bytes,
        system_disk: info.system_disk || info.number == 0,
        displayed_bytes,
    };
    backup::authorize_backup(&disk).map_err(|block| block.message().to_string())?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let free = backup::directory_free_bytes(parent)
        .map_err(|error| format!("could not check free space: {error}"))?;
    let need = backup::bytes_required(info.size_bytes);
    if free < need {
        return Err(format!(
            "not enough free space for the backup: need {need} bytes, {free} free"
        ));
    }
    diag::log(
        "INFO",
        "backup_start",
        &format!("disk={disk_number} bytes={}", info.size_bytes),
    );
    let serial = query_disk_serial(disk_number)?;
    let file = open_stock_disk_read_only(disk_number).inspect_err(|error| {
        diag::log("ERROR", "backup_failed", error);
    })?;
    let mut source = backup::ReadOnlyFile::new(file, info.size_bytes);
    let prefix_sha256 = probe::prefix_sha256(&mut source).map_err(|error| error.to_string())?;
    let mut last_emit = 0u64;
    let outcome = backup::backup_image(
        &mut source,
        &destination,
        free,
        &STOCK_BACKUP_CANCEL,
        |progress| {
            let boundary = progress.bytes_read == progress.bytes_total;
            if boundary || progress.bytes_read.saturating_sub(last_emit) >= 1024 * 1024 {
                last_emit = progress.bytes_read;
                let _ = app.emit("backup-progress", &progress);
            }
        },
    )
    .map_err(|error| {
        diag::log("ERROR", "backup_failed", &error.to_string());
        error.to_string()
    })?;
    match outcome {
        backup::BackupOutcome::Complete {
            image,
            sidecar,
            sha256,
            bytes,
        } => {
            diag::log(
                "INFO",
                "backup_done",
                &format!("disk={disk_number} bytes={bytes}"),
            );
            let samples = probe::sample_block_hashes(&mut source).map_err(|error| {
                format!(
                    "{error} The backup file is {} but this session will not use it for a card test.",
                    image.display()
                )
            })?;
            let identity = probe::CardIdentity {
                serial,
                prefix_sha256,
                bytes,
                model: info.model.clone(),
                samples,
            };
            probe::write_identity(&image, &identity).map_err(|error| {
                format!(
                    "{error} The backup file is {} but this session will not use it for a card test.",
                    image.display()
                )
            })?;
            session_backups().remember(probe::SessionBackup {
                image: image.clone(),
                size_bytes: bytes,
                model: info.model.clone(),
                sha256: sha256.clone(),
                identity,
            });
            Ok(BackupReportView {
                status: "complete".into(),
                image: image.display().to_string(),
                sidecar: sidecar.display().to_string(),
                sha256,
                partial: String::new(),
                bytes,
            })
        }
        backup::BackupOutcome::Cancelled { partial } => {
            diag::log(
                "INFO",
                "backup_cancelled",
                &format!("disk={disk_number} partial={}", partial.display()),
            );
            Ok(BackupReportView {
                status: "cancelled".into(),
                image: String::new(),
                sidecar: String::new(),
                sha256: String::new(),
                partial: partial.display().to_string(),
                bytes: 0,
            })
        }
    }
}

#[tauri::command]
fn cancel_stock_backup() {
    STOCK_BACKUP_CANCEL.store(true, Ordering::SeqCst);
}

static RESTORE_RUNNING: AtomicBool = AtomicBool::new(false);
static RESTORE_CANCEL: AtomicBool = AtomicBool::new(false);
static RESTORE_CANCEL_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        RESTORE_RUNNING.store(false, Ordering::SeqCst);
        *restore_cancel_slot() = None;
    }
}

fn restore_cancel_slot() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    RESTORE_CANCEL_FILE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn begin_restore() -> Result<RestoreGuard, String> {
    if PROBE_RUNNING.load(Ordering::SeqCst) || FULL_RUNNING.load(Ordering::SeqCst) {
        return Err("a card test is already running".into());
    }
    if STOCK_BACKUP_RUNNING.load(Ordering::SeqCst) {
        return Err("a stock-card backup is already running".into());
    }
    if RESTORE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("a backup restore is already running".into());
    }
    RESTORE_CANCEL.store(false, Ordering::SeqCst);
    Ok(RestoreGuard)
}

fn signal_restore_cancel() {
    RESTORE_CANCEL.store(true, Ordering::SeqCst);
    if let Some(path) = restore_cancel_slot().clone() {
        let _ = restore::signal_cancel_file(&path);
    }
}

/// Read-only `Get-Disk`. This does not open `\\.\PhysicalDriveN`.
fn query_restore_disk(number: u32) -> Result<restore::RestoreDisk, String> {
    if number == 0 {
        return Err("refusing to open disk 0".into());
    }
    let script = backup::disk_query_script(number);
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|error| format!("could not inspect the disk: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.contains('|'))
        .ok_or("no disk info")?;
    restore::parse_restore_disk(line, number)
}

fn powershell_literal(text: &str) -> Result<String, String> {
    if text.chars().any(|ch| matches!(ch, '\n' | '\r' | '\0')) {
        return Err("a restore path contains a newline".into());
    }
    Ok(text.replace('\'', "''"))
}

struct RestoreTemps {
    script: PathBuf,
    log: PathBuf,
    cancel: PathBuf,
    progress: PathBuf,
}

impl Drop for RestoreTemps {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.script);
        let _ = std::fs::remove_file(&self.log);
        let _ = std::fs::remove_file(&self.cancel);
        let _ = std::fs::remove_file(&self.progress);
    }
}

fn restore_temp_paths() -> RestoreTemps {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );
    let dir = std::env::temp_dir();
    RestoreTemps {
        script: dir.join(format!("cfw-restore-ui-{unique}.ps1")),
        log: dir.join(format!("cfw-restore-ui-{unique}.txt")),
        cancel: dir.join(format!("cfw-restore-cancel-{unique}.txt")),
        progress: dir.join(format!("cfw-restore-progress-{unique}.txt")),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreReportView {
    status: String,
    sha256: String,
    card_sha256: String,
    bytes: u64,
    detail: String,
}

fn restore_view_from_verdict(
    verdict: restore::HelperVerdict,
    image_bytes: u64,
) -> Result<RestoreReportView, String> {
    match verdict {
        restore::HelperVerdict::Passed {
            sha256,
            bytes,
            detail,
        } => Ok(RestoreReportView {
            status: "pass".into(),
            card_sha256: sha256.clone(),
            sha256,
            bytes: if bytes == 0 { image_bytes } else { bytes },
            detail,
        }),
        restore::HelperVerdict::Failed {
            expected,
            actual,
            bytes,
            detail,
        } => Ok(RestoreReportView {
            status: "fail".into(),
            sha256: expected,
            card_sha256: actual,
            bytes: if bytes == 0 { image_bytes } else { bytes },
            detail,
        }),
        restore::HelperVerdict::Cancelled { detail } => Ok(RestoreReportView {
            status: "cancelled".into(),
            sha256: String::new(),
            card_sha256: String::new(),
            bytes: 0,
            detail,
        }),
        restore::HelperVerdict::Incomplete { detail } => Err(detail),
    }
}

/// Restore a verified `.img.gz` backup by launching `cfw-flash`.
///
/// The helper is the only process that opens a physical drive for writing.
/// This command refuses the disk before that launch.
#[tauri::command]
fn restore_backup(
    app: tauri::AppHandle,
    disk_number: u32,
    displayed_bytes: u64,
    displayed_model: String,
    confirmation: String,
    image_path: String,
) -> Result<RestoreReportView, String> {
    let _guard = begin_restore()?;
    if disk_number == 0 {
        return Err("refusing to open disk 0".into());
    }
    if confirmation != "FLASH" {
        return Err(restore::RestoreBlock::Confirmation.message().into());
    }
    let tool = flash_tool()?;
    let image = PathBuf::from(&image_path);
    let disk = query_restore_disk(disk_number)?;
    let target = restore::RestoreTarget {
        disk: &disk,
        confirmed_number: disk_number,
        confirmation: &confirmation,
        displayed_bytes,
        displayed_model: &displayed_model,
    };
    let mut last_emit = 0u64;
    let inspected = match restore::prepare_restore(&image, &target, &RESTORE_CANCEL, |progress| {
        let boundary = progress.bytes_total > 0 && progress.bytes_done == progress.bytes_total;
        if boundary || progress.bytes_done.saturating_sub(last_emit) >= 1024 * 1024 {
            last_emit = progress.bytes_done;
            let _ = app.emit("restore-progress", &progress);
        }
    }) {
        Ok(inspected) => inspected,
        Err(restore::RestoreError::Cancelled { bytes_written }) => {
            return Ok(RestoreReportView {
                status: "cancelled".into(),
                sha256: String::new(),
                card_sha256: String::new(),
                bytes: bytes_written,
                detail: restore::cancel_detail(bytes_written),
            });
        }
        Err(error) => return Err(error.to_string()),
    };
    if RESTORE_CANCEL.load(Ordering::SeqCst) {
        return Ok(RestoreReportView {
            status: "cancelled".into(),
            sha256: String::new(),
            card_sha256: String::new(),
            bytes: 0,
            detail: restore::cancel_detail(0),
        });
    }
    if !restore::is_sha256_hex(&inspected.compressed_sha256)
        || !restore::is_sha256_hex(&inspected.sha256)
    {
        return Err("checksum is not a SHA-256 hex string".into());
    }

    let temps = restore_temp_paths();
    restore::write_new_nofollow(&temps.cancel, b"")
        .map_err(|error| format!("could not prepare cancel: {error}"))?;
    restore::write_new_nofollow(
        &temps.progress,
        format!("write 0 {}\n", inspected.image_bytes).as_bytes(),
    )
    .map_err(|error| format!("could not prepare progress: {error}"))?;
    restore::write_new_nofollow(&temps.log, b"")
        .map_err(|error| format!("could not prepare the restore log: {error}"))?;
    *restore_cancel_slot() = Some(temps.cancel.clone());

    let model = powershell_literal(&displayed_model)?;
    let script = format!(
        "& '{tool}' --image='{image}' --sha256='{sha}' --disk={disk_number} --confirm='FLASH' --restore --sidecar-sha256='{sidecar}' --uncompressed-bytes={image_bytes} --expected-bytes={displayed_bytes} --expected-model='{model}' --cancel-file='{cancel}' --progress-file='{progress}' --log-file='{log}'\r\nexit $LASTEXITCODE\r\n",
        tool = powershell_literal(&tool.display().to_string())?,
        image = powershell_literal(&image.display().to_string())?,
        sha = inspected.compressed_sha256,
        sidecar = inspected.sha256,
        image_bytes = inspected.image_bytes,
        cancel = powershell_literal(&temps.cancel.display().to_string())?,
        progress = powershell_literal(&temps.progress.display().to_string())?,
        log = powershell_literal(&temps.log.display().to_string())?,
    );
    restore::write_new_nofollow(&temps.script, script.as_bytes())
        .map_err(|error| format!("could not write the launcher: {error}"))?;

    let _ = app.emit(
        "restore-progress",
        restore::RestoreProgress {
            phase: "write".into(),
            bytes_done: 0,
            bytes_total: inspected.image_bytes,
        },
    );
    let model_log = displayed_model.replace(['\n', '\r'], " ");
    diag::log(
        "INFO",
        "restore_start",
        &format!("disk={disk_number} bytes={displayed_bytes} model={model_log}"),
    );

    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_flag = std::sync::Arc::clone(&stop);
    let progress_path = temps.progress.clone();
    let app_progress = app.clone();
    let poller = std::thread::spawn(move || {
        let mut last = String::new();
        loop {
            if let Ok(text) = restore::read_nofollow_string(&progress_path) {
                if text != last {
                    if let Some(progress) = restore::parse_progress_line(&text) {
                        let _ = app_progress.emit("restore-progress", &progress);
                    }
                    last = text;
                }
            }
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });

    let launcher = format!(
        "$p = Start-Process -FilePath powershell.exe -Verb RunAs -Wait -PassThru -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{}'; if ($null -eq $p -or $null -eq $p.ExitCode) {{ exit 1 }}; exit $p.ExitCode",
        temps.script.display().to_string().replace('\'', "''")
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &launcher])
        .status();
    stop.store(true, Ordering::SeqCst);
    let _ = poller.join();
    let text = restore::read_nofollow_string(&temps.log)
        .unwrap_or_else(|error| format!("could not read the restore log: {error}"));
    let status = status.map_err(|error| format!("could not start the flash tool: {error}"))?;
    let exit_code = restore::parse_exit_code(&text)
        .or_else(|| status.code())
        .unwrap_or(-1);
    match restore::classify_helper_stop(exit_code, &text) {
        restore::StopKind::CancelledUntouched => {
            diag::log("INFO", "restore_cancelled", &format!("disk={disk_number}"));
            return Ok(RestoreReportView {
                status: "cancelled".into(),
                sha256: String::new(),
                card_sha256: String::new(),
                bytes: 0,
                detail: restore::cancel_detail(0),
            });
        }
        restore::StopKind::CancelledErased => {
            diag::log("INFO", "restore_cancelled", &format!("disk={disk_number}"));
            return Ok(RestoreReportView {
                status: "cancelled".into(),
                sha256: String::new(),
                card_sha256: String::new(),
                bytes: 0,
                detail: restore::cancel_detail(1),
            });
        }
        restore::StopKind::FailedAfterClear { detail } => {
            diag::log(
                "ERROR",
                "restore_failed",
                &format!("disk={disk_number}: {detail}"),
            );
            return Err(detail);
        }
        restore::StopKind::Other => {}
    }
    if RESTORE_CANCEL.load(Ordering::SeqCst)
        && !text.contains("RESULT=OK")
        && !text.contains("verify=fail")
    {
        diag::log("INFO", "restore_cancelled", &format!("disk={disk_number}"));
        if text.trim().is_empty() {
            return Ok(RestoreReportView {
                status: "cancelled".into(),
                sha256: String::new(),
                card_sha256: String::new(),
                bytes: 0,
                detail: restore::cancel_detail(0),
            });
        }
        return Err(format!("{text}\nThe card was not written."));
    }

    let verdict = restore::judge_helper_log(&text, &inspected.sha256);
    let passed = matches!(verdict, restore::HelperVerdict::Passed { .. });
    if passed && !status.success() {
        diag::log(
            "ERROR",
            "restore_failed",
            &format!("disk={disk_number}: {text}"),
        );
        return Err(if text.trim().is_empty() {
            "the restore did not finish. The approval prompt may have been dismissed.".into()
        } else {
            text
        });
    }
    match restore_view_from_verdict(verdict, inspected.image_bytes) {
        Ok(view) => {
            let event = match view.status.as_str() {
                "pass" => "restore_done",
                "fail" => "restore_verify_failed",
                _ => "restore_cancelled",
            };
            let level = if view.status == "fail" {
                "ERROR"
            } else {
                "INFO"
            };
            diag::log(
                level,
                event,
                &format!(
                    "disk={disk_number} status={} bytes={}",
                    view.status, view.bytes
                ),
            );
            Ok(view)
        }
        Err(detail) => {
            diag::log(
                "ERROR",
                "restore_failed",
                &format!("disk={disk_number}: {detail}"),
            );
            Err(detail)
        }
    }
}

#[tauri::command]
fn cancel_restore() {
    signal_restore_cancel();
}

static PROBE_RUNNING: AtomicBool = AtomicBool::new(false);
static PROBE_CANCEL: AtomicBool = AtomicBool::new(false);
static PROBE_CANCEL_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
static FULL_RUNNING: AtomicBool = AtomicBool::new(false);
static FULL_CANCEL: AtomicBool = AtomicBool::new(false);
static FULL_CANCEL_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

struct ProbeGuard;

impl Drop for ProbeGuard {
    fn drop(&mut self) {
        PROBE_RUNNING.store(false, Ordering::SeqCst);
        *probe_cancel_slot() = None;
    }
}

fn probe_cancel_slot() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    PROBE_CANCEL_FILE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn session_backups() -> std::sync::MutexGuard<'static, probe::BackupSession> {
    static SESSION: std::sync::OnceLock<Mutex<probe::BackupSession>> = std::sync::OnceLock::new();
    SESSION
        .get_or_init(|| Mutex::new(probe::BackupSession::default()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn begin_probe() -> Result<ProbeGuard, String> {
    if STOCK_BACKUP_RUNNING.load(Ordering::SeqCst) {
        return Err("a stock-card backup is already running".into());
    }
    if RESTORE_RUNNING.load(Ordering::SeqCst) {
        return Err("a backup restore is already running".into());
    }
    if FULL_RUNNING.load(Ordering::SeqCst) {
        return Err("a card test is already running".into());
    }
    if PROBE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("a card test is already running".into());
    }
    PROBE_CANCEL.store(false, Ordering::SeqCst);
    Ok(ProbeGuard)
}

fn signal_probe_cancel() {
    PROBE_CANCEL.store(true, Ordering::SeqCst);
    if let Some(path) = probe_cancel_slot().clone() {
        let _ = restore::signal_cancel_file(&path);
    }
}

fn probe_disk_from_restore(disk: &restore::RestoreDisk, serial: String) -> probe::ProbeDisk {
    probe::ProbeDisk {
        number: disk.number,
        bus: disk.bus.clone(),
        size_bytes: disk.size_bytes,
        system_disk: disk.system_disk,
        model: disk.model.clone(),
        serial,
    }
}

fn query_disk_serial(number: u32) -> Result<String, String> {
    if number == 0 {
        return Err("refusing to open disk 0".into());
    }
    let script = probe::disk_serial_script(number);
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|error| format!("could not read the disk serial: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not read the disk serial: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    Ok(probe::normalize_serial(line))
}

fn query_partition_table_id(number: u32) -> Result<String, String> {
    if number == 0 {
        return Err("refusing to open disk 0".into());
    }
    let script = probe::partition_table_script(number);
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|error| format!("could not read the partition table id: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not read the partition table id: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    probe::normalize_partition_table_id(line)
}

struct ProbeTemps {
    log: PathBuf,
    cancel: PathBuf,
}

impl Drop for ProbeTemps {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.log);
        let _ = std::fs::remove_file(&self.cancel);
    }
}

fn probe_temp_paths() -> ProbeTemps {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );
    let dir = std::env::temp_dir();
    ProbeTemps {
        log: dir.join(format!("cfw-probe-ui-{unique}.txt")),
        cancel: dir.join(format!("cfw-probe-cancel-{unique}.txt")),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeReportView {
    status: String,
    kind: String,
    claimed_bytes: u64,
    real_bytes: u64,
    read_mbps: f64,
    write_mbps: f64,
    overwritten: bool,
    summary: String,
}

fn cancelled_probe_view(claimed_bytes: u64, overwritten: bool, summary: String) -> ProbeReportView {
    probe_view(probe::ProbeLog {
        status: "cancelled".into(),
        kind: String::new(),
        claimed_bytes,
        real_bytes: 0,
        read_mbps: 0.0,
        write_mbps: 0.0,
        overwritten,
        summary,
    })
}

fn probe_view(parsed: probe::ProbeLog) -> ProbeReportView {
    ProbeReportView {
        status: parsed.status,
        kind: parsed.kind,
        claimed_bytes: parsed.claimed_bytes,
        real_bytes: parsed.real_bytes,
        read_mbps: parsed.read_mbps,
        write_mbps: parsed.write_mbps,
        overwritten: parsed.overwritten,
        summary: parsed.summary,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiskIdentityView {
    disk_number: u32,
    serial: String,
    size_bytes: u64,
    partition_table_id: String,
}

/// Disk number, serial, size, and partition-table id from `Get-Disk`.
/// Does not open `\\.\PhysicalDriveN`.
///
/// The serial may be the card reader's serial. `partition_table_id` is the
/// GPT disk GUID or the MBR disk signature, which stays with the card.
#[tauri::command]
fn read_disk_identity(disk_number: u32) -> Result<DiskIdentityView, String> {
    if disk_number == 0 {
        return Err("refusing to open disk 0".into());
    }
    let queried = query_restore_disk(disk_number)?;
    let serial = query_disk_serial(disk_number)?;
    let disk = probe_disk_from_restore(&queried, serial);
    let (serial, size_bytes) = probe::accept_read_only_identity(&disk)?;
    let partition_table_id = query_partition_table_id(disk_number)?;
    Ok(DiskIdentityView {
        disk_number: disk.number,
        serial,
        size_bytes,
        partition_table_id,
    })
}

/// USB and SD cards only. Disk 0, system disks, and boot disks are left out.
#[tauri::command]
fn list_probe_disks() -> Result<Vec<FlashDiskView>, String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Disk | Where-Object { ($_.BusType -eq 'USB' -or $_.BusType -eq 'SD') -and $_.Number -ne 0 -and -not $_.IsSystem -and -not $_.IsBoot } | Select-Object Number, FriendlyName, Size | ConvertTo-Json -Compress",
        ])
        .output()
        .map_err(|error| format!("could not list disks: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("could not read the disk list: {error}"))?;
    let rows = if value.is_array() {
        value.as_array().cloned().unwrap_or_default()
    } else {
        vec![value]
    };
    let mut disks = Vec::new();
    for row in rows {
        let number = row.get("Number").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let size_bytes = row.get("Size").and_then(|v| v.as_u64()).unwrap_or(0);
        if number == 0 || size_bytes == 0 {
            continue;
        }
        disks.push(FlashDiskView {
            number,
            name: row
                .get("FriendlyName")
                .and_then(|v| v.as_str())
                .unwrap_or("USB disk")
                .to_string(),
            size_bytes,
        });
    }
    Ok(disks)
}

struct ScreenGate {
    remembered: Option<probe::SessionBackup>,
}

enum ScreenStop {
    Cancelled {
        bytes: u64,
        overwritten: bool,
        summary: String,
    },
    Failed(String),
}

/// App-side blank or identity check shared by the quick probe and the full test.
///
/// A card that cannot be opened without admin is logged and left to the
/// helper. The helper still rechecks the same gates before it writes.
fn screen_gate(
    disk_number: u32,
    displayed_bytes: u64,
    displayed_model: &str,
    confirmation: &str,
    word: &str,
    cancel: &AtomicBool,
    skip_event: &str,
) -> Result<ScreenGate, ScreenStop> {
    let live = query_restore_disk(disk_number).map_err(ScreenStop::Failed)?;
    let remembered = session_backups()
        .find(live.size_bytes, &live.model)
        .cloned();
    let probe_disk = probe_disk_from_restore(&live, String::new());
    let plan = probe::ProbePlan {
        confirmation: confirmation.to_string(),
        disk_number,
        displayed_bytes,
        displayed_model: displayed_model.to_string(),
    };
    probe::authorize_card_write(&probe_disk, &plan, word)
        .map_err(|block| ScreenStop::Failed(block.message().to_string()))?;

    if let Some(backup) = &remembered {
        match restore::inspect_backup(&backup.image, cancel, |_| {}) {
            Ok(_) => {}
            Err(restore::RestoreError::Cancelled { .. }) => {
                return Err(ScreenStop::Cancelled {
                    bytes: live.size_bytes,
                    overwritten: false,
                    summary: probe::cancel_summary(false, true),
                });
            }
            Err(_) if cancel.load(Ordering::SeqCst) => {
                return Err(ScreenStop::Cancelled {
                    bytes: live.size_bytes,
                    overwritten: false,
                    summary: probe::cancel_summary(false, true),
                });
            }
            Err(error) => return Err(ScreenStop::Failed(error.to_string())),
        }
    }

    match open_stock_disk_read_only(disk_number) {
        Ok(file) => {
            let mut source = backup::ReadOnlyFile::new(file, live.size_bytes);
            if let Some(backup) = &remembered {
                let serial = query_disk_serial(disk_number).map_err(ScreenStop::Failed)?;
                let probe_disk = probe_disk_from_restore(&live, serial);
                match probe::verify_session_backup(backup, &probe_disk, &mut source, cancel) {
                    Ok(()) => {}
                    Err(probe::ProbeError::Failed {
                        overwritten,
                        message,
                    }) if !overwritten
                        && (cancel.load(Ordering::SeqCst) || message.starts_with("Cancelled.")) =>
                    {
                        return Err(ScreenStop::Cancelled {
                            bytes: live.size_bytes,
                            overwritten,
                            summary: message,
                        });
                    }
                    Err(error) => return Err(ScreenStop::Failed(error.to_string())),
                }
            } else if !probe::card_is_blank(&mut source)
                .map_err(|error| ScreenStop::Failed(error.to_string()))?
            {
                return Err(ScreenStop::Failed(
                    probe::ProbeBlock::NotBlank.message().into(),
                ));
            }
        }
        Err(error) => {
            let probe::AppPrecheck::Deferred { note } = probe::app_precheck(Some(&error)) else {
                return Err(ScreenStop::Failed(error));
            };
            diag::log("WARN", skip_event, &format!("disk={disk_number} {note}"));
        }
    }
    Ok(ScreenGate { remembered })
}

/// Test a removable card for fake capacity. Writes go through `cfw-flash`.
///
/// A backup of this card from the current process, or a blank card, is
/// required. The backup matches serial, a hash of the first 4 MiB, and the
/// sample blocks the probe overwrites, not only size and model. If the card
/// cannot be read without admin, that read is logged and skipped. The helper
/// still rechecks size, model, bus, system/boot, blank-or-identity, and the
/// sample blocks after the approval prompt, and locks volumes before a
/// backup-route write.
#[tauri::command]
fn probe_card(
    app: tauri::AppHandle,
    disk_number: u32,
    displayed_bytes: u64,
    displayed_model: String,
    confirmation: String,
) -> Result<ProbeReportView, String> {
    let _guard = begin_probe()?;
    if disk_number == 0 {
        return Err("refusing to open disk 0".into());
    }
    if confirmation != "PROBE" {
        return Err(probe::ProbeBlock::Confirmation.message().into());
    }
    let tool = flash_tool()?;
    let ScreenGate { remembered } = match screen_gate(
        disk_number,
        displayed_bytes,
        &displayed_model,
        &confirmation,
        "PROBE",
        &PROBE_CANCEL,
        "probe_precheck_skipped",
    ) {
        Ok(gate) => gate,
        Err(ScreenStop::Cancelled {
            bytes,
            overwritten,
            summary,
        }) => return Ok(cancelled_probe_view(bytes, overwritten, summary)),
        Err(ScreenStop::Failed(message)) => return Err(message),
    };

    let temps = probe_temp_paths();
    restore::write_new_nofollow(&temps.cancel, b"")
        .map_err(|error| format!("could not prepare cancel: {error}"))?;
    restore::write_new_nofollow(&temps.log, b"")
        .map_err(|error| format!("could not prepare the probe log: {error}"))?;
    *probe_cancel_slot() = Some(temps.cancel.clone());
    let request = probe::HelperRequest {
        disk_number,
        confirmation,
        displayed_bytes,
        displayed_model: displayed_model.clone(),
        backup_image: remembered.as_ref().map(|backup| backup.image.clone()),
        backup_sha256: remembered.as_ref().map(|backup| backup.sha256.clone()),
        backup_prefix_sha256: remembered
            .as_ref()
            .map(|backup| backup.identity.prefix_sha256.clone()),
        backup_serial: remembered
            .as_ref()
            .map(|backup| backup.identity.serial.clone()),
        cancel_file: temps.cancel.clone(),
        log_file: temps.log.clone(),
    };
    let launch = probe::helper_launch_command(&tool, &request)?;

    let model_log = displayed_model.replace(['\n', '\r'], " ");
    diag::log(
        "INFO",
        "probe_start",
        &format!("disk={disk_number} bytes={displayed_bytes} model={model_log}"),
    );
    let _ = app.emit(
        "probe-progress",
        "Checking the card. Windows may ask for approval before anything is written.",
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &launch])
        .status()
        .map_err(|error| format!("could not start the flash tool: {error}"))?;
    let text = restore::read_nofollow_string(&temps.log).unwrap_or_default();
    let user_cancelled = PROBE_CANCEL.load(Ordering::SeqCst);
    match probe::interpret_helper_log(status.code(), &text, user_cancelled, remembered.is_some()) {
        Ok(probe::HelperFollowup::Parsed(parsed)) => {
            let event = if parsed.status == "cancelled" {
                "probe_cancelled"
            } else {
                "probe_done"
            };
            diag::log(
                "INFO",
                event,
                &format!(
                    "disk={disk_number} status={} overwritten={}",
                    parsed.status, parsed.overwritten
                ),
            );
            Ok(probe_view(parsed))
        }
        Ok(probe::HelperFollowup::Cancelled {
            overwritten,
            summary,
        }) => {
            diag::log(
                "INFO",
                "probe_cancelled",
                &format!("disk={disk_number} overwritten={overwritten}"),
            );
            Ok(cancelled_probe_view(displayed_bytes, overwritten, summary))
        }
        Err(message) => {
            diag::log(
                "ERROR",
                "probe_failed",
                &format!("disk={disk_number}: {message}"),
            );
            Err(message)
        }
    }
}

#[tauri::command]
fn cancel_probe() {
    signal_probe_cancel();
}

struct FullGuard;

impl Drop for FullGuard {
    fn drop(&mut self) {
        FULL_RUNNING.store(false, Ordering::SeqCst);
        *full_cancel_slot() = None;
    }
}

fn full_cancel_slot() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    FULL_CANCEL_FILE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn begin_full() -> Result<FullGuard, String> {
    if STOCK_BACKUP_RUNNING.load(Ordering::SeqCst) {
        return Err("a stock-card backup is already running".into());
    }
    if RESTORE_RUNNING.load(Ordering::SeqCst) {
        return Err("a backup restore is already running".into());
    }
    if PROBE_RUNNING.load(Ordering::SeqCst) {
        return Err("a card test is already running".into());
    }
    if FULL_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("a card test is already running".into());
    }
    FULL_CANCEL.store(false, Ordering::SeqCst);
    Ok(FullGuard)
}

fn signal_full_cancel() {
    FULL_CANCEL.store(true, Ordering::SeqCst);
    if let Some(path) = full_cancel_slot().clone() {
        let _ = restore::signal_cancel_file(&path);
    }
}

struct FullTemps {
    log: PathBuf,
    cancel: PathBuf,
    progress: PathBuf,
}

impl Drop for FullTemps {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.log);
        let _ = std::fs::remove_file(&self.cancel);
        let _ = std::fs::remove_file(&self.progress);
    }
}

fn full_temp_paths() -> FullTemps {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );
    let dir = std::env::temp_dir();
    FullTemps {
        log: dir.join(format!("cfw-full-ui-{unique}.txt")),
        cancel: dir.join(format!("cfw-full-cancel-{unique}.txt")),
        progress: dir.join(format!("cfw-full-progress-{unique}.txt")),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FullReportView {
    status: String,
    overwritten: bool,
    summary: String,
    bad_count: u32,
    claimed_bytes: u64,
    write_mbps: f64,
    read_mbps: f64,
}

fn full_view(parsed: fullcheck::FullLog, claimed_bytes: u64) -> FullReportView {
    FullReportView {
        status: parsed.status,
        overwritten: parsed.overwritten,
        summary: parsed.summary,
        bad_count: u32::try_from(parsed.range_count).unwrap_or(u32::MAX),
        claimed_bytes: if parsed.claimed_bytes == 0 {
            claimed_bytes
        } else {
            parsed.claimed_bytes
        },
        write_mbps: parsed.write_mbps,
        read_mbps: parsed.read_mbps,
    }
}

fn cancelled_full_view(claimed_bytes: u64, overwritten: bool, summary: String) -> FullReportView {
    full_view(
        fullcheck::FullLog {
            status: "cancelled".into(),
            overwritten,
            summary,
            bad_ranges: Vec::new(),
            range_count: 0,
            bad_bytes: 0,
            seed: 0,
            claimed_bytes,
            write_mbps: 0.0,
            read_mbps: 0.0,
        },
        claimed_bytes,
    )
}

/// Write a pattern across the whole card and read it back. Optional and slow.
///
/// Uses the same blank-or-backup gate as the quick probe. The elevated helper
/// rechecks size, model, bus, system/boot, and identity after approval, and
/// locks volumes on the backup route before it writes.
#[tauri::command]
fn full_check_card(
    app: tauri::AppHandle,
    disk_number: u32,
    displayed_bytes: u64,
    displayed_model: String,
    confirmation: String,
) -> Result<FullReportView, String> {
    let _guard = begin_full()?;
    if disk_number == 0 {
        return Err("refusing to open disk 0".into());
    }
    if confirmation != fullcheck::FULL_WORD {
        return Err(probe::ProbeBlock::Confirmation.message().into());
    }
    let tool = flash_tool()?;
    let ScreenGate { remembered } = match screen_gate(
        disk_number,
        displayed_bytes,
        &displayed_model,
        &confirmation,
        fullcheck::FULL_WORD,
        &FULL_CANCEL,
        "fullcheck_precheck_skipped",
    ) {
        Ok(gate) => gate,
        Err(ScreenStop::Cancelled {
            bytes,
            overwritten,
            summary,
        }) => {
            return Ok(cancelled_full_view(bytes, overwritten, summary));
        }
        Err(ScreenStop::Failed(message)) => return Err(message),
    };

    let temps = full_temp_paths();
    restore::write_new_nofollow(&temps.cancel, b"")
        .map_err(|error| format!("could not prepare cancel: {error}"))?;
    restore::write_new_nofollow(&temps.log, b"")
        .map_err(|error| format!("could not prepare the full-check log: {error}"))?;
    restore::write_new_nofollow(&temps.progress, b"")
        .map_err(|error| format!("could not prepare progress: {error}"))?;
    *full_cancel_slot() = Some(temps.cancel.clone());
    let request = probe::HelperRequest {
        disk_number,
        confirmation,
        displayed_bytes,
        displayed_model: displayed_model.clone(),
        backup_image: remembered.as_ref().map(|backup| backup.image.clone()),
        backup_sha256: remembered.as_ref().map(|backup| backup.sha256.clone()),
        backup_prefix_sha256: remembered
            .as_ref()
            .map(|backup| backup.identity.prefix_sha256.clone()),
        backup_serial: remembered
            .as_ref()
            .map(|backup| backup.identity.serial.clone()),
        cancel_file: temps.cancel.clone(),
        log_file: temps.log.clone(),
    };
    let launch = probe::launch_command(&tool, "--full-check", &request, Some(&temps.progress))?;

    let model_log = displayed_model.replace(['\n', '\r'], " ");
    diag::log(
        "INFO",
        "fullcheck_start",
        &format!("disk={disk_number} bytes={displayed_bytes} model={model_log}"),
    );

    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_flag = std::sync::Arc::clone(&stop);
    let progress_path = temps.progress.clone();
    let app_progress = app.clone();
    let poller = std::thread::spawn(move || {
        let mut last = String::new();
        loop {
            if let Ok(text) = restore::read_nofollow_string(&progress_path) {
                if text != last {
                    if let Some(progress) = fullcheck::parse_progress_line(&text) {
                        let _ = app_progress.emit("fullcheck-progress", &progress);
                    }
                    last = text;
                }
            }
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });

    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &launch])
        .status();
    stop.store(true, Ordering::SeqCst);
    let _ = poller.join();
    let status = status.map_err(|error| format!("could not start the flash tool: {error}"))?;
    let text = restore::read_nofollow_string(&temps.log).unwrap_or_default();
    let user_cancelled = FULL_CANCEL.load(Ordering::SeqCst);
    match fullcheck::interpret_full_log(status.code(), &text, user_cancelled, remembered.is_some())
    {
        Ok(parsed) => {
            let event = if parsed.status == "cancelled" {
                "fullcheck_cancelled"
            } else {
                "fullcheck_done"
            };
            diag::log(
                "INFO",
                event,
                &format!(
                    "disk={disk_number} status={} overwritten={} bad={} seed={}",
                    parsed.status, parsed.overwritten, parsed.range_count, parsed.seed
                ),
            );
            Ok(full_view(parsed, displayed_bytes))
        }
        Err(message) => {
            diag::log(
                "ERROR",
                "fullcheck_failed",
                &format!("disk={disk_number}: {message}"),
            );
            Err(message)
        }
    }
}

#[tauri::command]
fn cancel_full_check() {
    signal_full_cancel();
}

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
            diag::log(
                "INFO",
                "feed_check",
                &format!(
                    "status={status} updates={} additions={}",
                    diff.updates.len(),
                    diff.additions.len()
                ),
            );
            Ok(FeedCheckView {
                status: status.into(),
                reason: String::new(),
                updates: diff.updates,
                additions: diff.additions.iter().map(|p| p.id.clone()).collect(),
                app_update: diff.app_update,
            })
        }
        Err(reason) => {
            diag::log(
                "INFO",
                "feed_check",
                &format!("status=error reason={reason}"),
            );
            Ok(FeedCheckView {
                status: "error".into(),
                reason,
                updates: Vec::new(),
                additions: Vec::new(),
                app_update: None,
            })
        }
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
    let report = feed::apply_feed(&store::profile_store_dir(), &parsed, &local)?;
    diag::log(
        "INFO",
        "feed_applied",
        &format!("applied={} skipped={}", report.applied, report.skipped),
    );
    Ok(report)
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

#[tauri::command]
fn diagnostics_path() -> String {
    diag::log_path().display().to_string()
}

/// Doctor diagnosis of one selected card: deterministic templates plus an
/// optional engine paraphrase. Display-only: the gates already decided;
/// this explains their outcome and never changes it.
#[tauri::command]
fn diagnose_card(
    app: tauri::AppHandle,
    profile_id: String,
    volume_id: String,
    with_engine: bool,
) -> Result<needle::doctor::DoctorReport, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let volume = require_volume(&volume_id)?;
    let label = profile.rom_schema.volume_label.clone().unwrap_or_default();
    let decision = decide(&volume, &label);
    let (decision_name, decision_reason) = match &decision {
        VolumeDecision::Ready { .. } => ("ready".to_string(), String::new()),
        VolumeDecision::NeedsFormat { reason } => ("needs_format".to_string(), reason.clone()),
        VolumeDecision::Rejected { reason } => ("rejected".to_string(), reason.clone()),
    };
    let safety = check_card_safety(&profile, &volume)
        .map_err(|error| format!("could not judge first boot on {}: {error}", volume.letter))?;
    let partitions = lettered_partitions().unwrap_or_default();
    let this_disk = partitions
        .iter()
        .find(|row| {
            row.letter
                .eq_ignore_ascii_case(&volume.letter.chars().next().unwrap_or('?'))
        })
        .map(|row| row.disk);
    let disk_volumes: Vec<needle::doctor::VolumeDisk> = match this_disk {
        Some(disk) => {
            let live = volume::list_volumes().unwrap_or_default();
            partitions
                .iter()
                .filter(|row| row.disk == disk)
                .map(|row| {
                    let label = live
                        .iter()
                        .find(|v| v.letter.starts_with(row.letter))
                        .map(|v| v.label.clone())
                        .unwrap_or_default();
                    needle::doctor::VolumeDisk {
                        letter: row.letter.to_string(),
                        label,
                    }
                })
                .collect()
        }
        None => Vec::new(),
    };
    let facts = needle::doctor::collect_facts(
        &volume,
        &decision_name,
        &decision_reason,
        disk_volumes,
        &safety,
        &card_root(&volume),
    );
    let report = if with_engine {
        match resolve_serve_paths() {
            Some((engine_exe, weights_path)) => {
                match needle::serve::ask_through_sidecar(
                    &engine_exe,
                    &weights_path,
                    &doctor_input(&facts),
                ) {
                    Ok(turn) => report_with_turn(&facts, &turn.call.name, &turn.call.arguments),
                    Err(error) => {
                        diag::log("WARN", "doctor_failed", &format!("engine: {error}"));
                        needle::doctor::diagnose(&facts, None)
                    }
                }
            }
            None => needle::doctor::diagnose(&facts, None),
        }
    } else {
        needle::doctor::diagnose(&facts, None)
    };
    let outcome = if report.engine_text.is_some() || report.suggested_profile_id.is_some() {
        "engine"
    } else {
        "template"
    };
    diag::log("info", "doctor_run", &format!("outcome={outcome}"));
    Ok(report)
}

fn doctor_input(facts: &needle::doctor::CardFacts) -> String {
    serde_json::to_string(facts)
        .map(|json| format!("Explain this card. CardFacts: {json}"))
        .unwrap_or_else(|_| "Explain this card.".to_string())
}

fn report_with_turn(
    facts: &needle::doctor::CardFacts,
    name: &str,
    arguments: &serde_json::Value,
) -> needle::doctor::DoctorReport {
    let parsed = needle::doctor::parse_engine_turn(name, arguments);
    let mut report = needle::doctor::explain(facts);
    if let Some((text, profile)) = parsed {
        report.engine_text = text;
        report.suggested_profile_id = profile;
    }
    report
}

fn resolve_serve_paths() -> Option<(PathBuf, PathBuf)> {
    let engine_artifact = needle::manifest::find("serve-engine")?;
    let weights_artifact = needle::manifest::find("weights")?;
    let directories = needle::acquire::cache_directories();
    let find = |artifact: &needle::manifest::Artifact| {
        directories.iter().find_map(|dir| {
            let candidate = dir.join(artifact.file_name);
            if !candidate.is_file() {
                return None;
            }
            let verified = std::fs::File::open(&candidate)
                .ok()
                .and_then(|file| flash::sha256_reader(file).ok())
                .map(|hash| hash.eq_ignore_ascii_case(artifact.sha256))
                .unwrap_or(false);
            verified.then(|| candidate.clone())
        })
    };
    Some((find(engine_artifact)?, find(weights_artifact)?))
}

/// Presence and pin-verification of every Needle artifact row. Advisory
/// only: the app behaves identically when nothing is acquired.
#[tauri::command]
fn needle_status() -> Vec<needle::acquire::ArtifactStatus> {
    needle::acquire::artifact_status()
}

/// Downloads one runtime artifact (`weights` | `serve-engine`) into the
/// needle cache after verifying it against the compiled-in SHA-256 pin.
/// Build inputs and unknown ids are rejected here, at the IPC boundary.
#[tauri::command]
fn needle_acquire(app: tauri::AppHandle, artifact_id: String) -> Result<String, String> {
    let artifact = needle::acquire::runtime_artifact(&artifact_id)?;
    diag::log("info", "needle_download_start", artifact.id);
    let _ = app.emit(
        "needle-progress",
        format!("Downloading {} ({})…", artifact.id, artifact.file_name),
    );
    let url = artifact.url;
    match needle::acquire::ensure_artifact(artifact, |dest| download_image(url, dest)) {
        Ok(path) => {
            let _ = app.emit("needle-progress", format!("{} ready", artifact.id));
            diag::log(
                "info",
                "needle_download_done",
                &format!("{} bytes={}", artifact.id, artifact.size),
            );
            Ok(path.display().to_string())
        }
        Err(error) => {
            diag::log(
                "WARN",
                "needle_download_failed",
                &format!("{}: {}", artifact.id, error),
            );
            Err(error)
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StageProgress {
    system_folder: String,
    line: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StageReportView {
    staging_path: String,
    staged_systems: Vec<String>,
    raw_copied_systems: Vec<String>,
    staged_files: usize,
}

/// igir runs from PATH when installed, otherwise through npx (Node.js).
/// Spawned as `npx.cmd` directly: Rust >= 1.77.2 quotes .cmd targets safely,
/// and a `cmd /C` wrapper would let argv metacharacters break out.
fn resolve_igir() -> Result<Vec<String>, String> {
    let on_path = |name: &str| {
        std::process::Command::new("where")
            .arg(name)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    };
    if on_path("igir.exe") {
        return Ok(vec!["igir".into()]);
    }
    if on_path("npx.cmd") {
        return Ok(vec!["npx.cmd".into(), "--yes".into(), "igir@latest".into()]);
    }
    if on_path("npx") {
        return Ok(vec!["npx".into(), "--yes".into(), "igir@latest".into()]);
    }
    Err(
        "igir sorting needs either igir on PATH or Node.js installed so igir can run through npx"
            .into(),
    )
}

fn run_igir(
    app: &tauri::AppHandle,
    base: &[String],
    system_folder: &str,
    args: &[String],
) -> Result<(), String> {
    use std::io::BufRead;
    let mut command = std::process::Command::new(&base[0]);
    command
        .args(&base[1..])
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start igir: {error}"))?;
    if let Some(stderr) = child.stderr.take() {
        let app = app.clone();
        let system_folder = system_folder.to_string();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                let _ = app.emit(
                    "stage-progress",
                    StageProgress {
                        system_folder: system_folder.clone(),
                        line,
                    },
                );
            }
        });
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not run igir: {error}"))?;
    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr);
        let tail = tail
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "igir failed for {system_folder}:\n{}",
            if tail.trim().is_empty() {
                format!("exit code {}", output.status)
            } else {
                tail
            }
        ));
    }
    Ok(())
}

#[tauri::command]
fn stage_library(
    app: tauri::AppHandle,
    profile_id: String,
    library: String,
    dat_folder: String,
    regions: Vec<String>,
    single: bool,
    include: Vec<String>,
) -> Result<StageReportView, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let staging = store::store_root().join("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|error| format!("could not clear the staging folder: {error}"))?;
    }
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("could not create {}: {error}", staging.display()))?;

    let dat = match dat_folder.trim() {
        "" => None,
        trimmed => Some(PathBuf::from(trimmed)),
    };
    let plans = igir::stage_plans(
        Path::new(&library),
        &staging,
        &profile.rom_schema.systems,
        &include,
        dat.as_deref(),
        &regions,
        single,
    );
    if plans.is_empty() {
        return Err("no systems were selected to stage".into());
    }
    let base = resolve_igir()?;
    diag::log(
        "INFO",
        "stage_start",
        &format!(
            "profile={profile_id} systems={} dat={}",
            plans.len(),
            dat.is_some()
        ),
    );
    let mut staged_systems = Vec::new();
    let mut raw_copied_systems = Vec::new();
    for plan in &plans {
        if dat.is_some() && plan.args.iter().any(|arg| arg == "--dat") {
            staged_systems.push(plan.system_folder.clone());
        } else {
            raw_copied_systems.push(plan.system_folder.clone());
        }
        if let Err(error) = run_igir(&app, &base, &plan.system_folder, &plan.args) {
            diag::log(
                "ERROR",
                "stage_failed",
                &format!("profile={profile_id}: {error}"),
            );
            return Err(error);
        }
    }

    let mut staged_files = 0;
    let mut stack = vec![staging.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .map_err(|error| format!("could not read {}: {error}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                staged_files += 1;
            }
        }
    }
    let staging_path_display = staging.display().to_string();
    diag::log(
        "INFO",
        "stage_done",
        &format!(
            "profile={profile_id} files={staged_files} sorted={} raw={}",
            staged_systems.len(),
            raw_copied_systems.len()
        ),
    );
    Ok(StageReportView {
        staging_path: staging_path_display,
        staged_systems,
        raw_copied_systems,
        staged_files,
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn with_license<T>(body: impl FnOnce(&license::Session<license::UreqTransport>) -> T) -> T {
    let _guard = license::session_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let session = license::production_session();
    body(&session)
}

fn log_license(event: &str, status: &license::LicenseStatus) {
    // `log_detail` is a fixed set of reason codes. It never includes the key.
    diag::log("INFO", event, &status.log_detail());
}

#[tauri::command]
async fn license_status() -> license::LicenseView {
    // The Gumroad call is blocking. It runs off the UI thread so a 15s timeout
    // during launch cannot freeze the window. The command still returns a view
    // and never a Tauri error, so a panic payload cannot echo the key.
    match tauri::async_runtime::spawn_blocking(|| {
        with_license(|session| session.status(now_unix()))
    })
    .await
    {
        Ok(status) => {
            log_license("license_status", &status);
            status.to_view(None)
        }
        Err(_) => {
            let status = license::worker_stopped_status();
            log_license("license_status", &status);
            status.to_view(None)
        }
    }
}

#[tauri::command]
async fn activate_license(license_key: String) -> license::LicenseView {
    match tauri::async_runtime::spawn_blocking(move || {
        with_license(|session| session.activate(&license_key, now_unix()))
    })
    .await
    {
        Ok(outcome) => {
            log_license("license_activate", &outcome.stored);
            outcome.to_view()
        }
        Err(_) => {
            let status = license::worker_stopped_status();
            log_license("license_activate", &status);
            status.to_view(None)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PaidFeatureView {
    feature: String,
    open: bool,
    message: String,
}

#[tauri::command]
async fn deactivate_license() -> license::LicenseView {
    match tauri::async_runtime::spawn_blocking(|| with_license(|session| session.deactivate()))
        .await
    {
        Ok(status) => {
            log_license("license_deactivate", &status);
            status.to_view(None)
        }
        Err(_) => {
            let status = license::worker_stopped_status();
            log_license("license_deactivate", &status);
            status.to_view(None)
        }
    }
}

/// Paid features only. Safety names return before a license session is opened,
/// so a license lookup cannot slow backup, probe, identity, or overwrite reporting.
#[tauri::command]
async fn paid_feature(feature: String) -> PaidFeatureView {
    if !license::command_entry(&feature).consults_license() {
        let decision = license::decide_paid(&feature, None);
        return PaidFeatureView {
            feature,
            open: decision.open,
            message: decision.message,
        };
    }
    let feature_name = feature.clone();
    let status = match tauri::async_runtime::spawn_blocking(|| {
        with_license(|session| session.status(now_unix()))
    })
    .await
    {
        Ok(status) => status,
        Err(_) => license::worker_stopped_status(),
    };
    log_license("paid_feature", &status);
    let decision = license::decide_paid(&feature_name, Some(&status));
    PaidFeatureView {
        feature: feature_name,
        open: decision.open,
        message: decision.message,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::panic::set_hook(Box::new(|info| {
        diag::log("ERROR", "panic", &format!("{info}"));
    }));
    diag::log(
        "INFO",
        "app_start",
        &format!("version={}", env!("CARGO_PKG_VERSION")),
    );
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            list_profiles,
            detect_stock_device,
            list_flash_disks,
            list_probe_disks,
            read_disk_identity,
            flash_os,
            backup_stock_card,
            cancel_stock_backup,
            restore_backup,
            cancel_restore,
            probe_card,
            cancel_probe,
            full_check_card,
            cancel_full_check,
            list_volumes,
            firstboot_state,
            prepare_card,
            seed_card,
            plan_roms,
            copy_roms,
            check_profile_feed,
            apply_profile_feed,
            app_version,
            stage_library,
            diagnostics_path,
            needle_status,
            needle_acquire,
            classify_library,
            diagnose_card,
            license_status,
            activate_license,
            deactivate_license,
            paid_feature
        ])
        .build(tauri::generate_context!())
        .expect("error while building the application")
        .run(|_app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // No helper process may outlive the app.
                needle::embed_client::kill_all();
                needle::serve::kill_all();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::read_disk_identity;

    #[test]
    fn read_disk_identity_refuses_disk_zero() {
        let error = read_disk_identity(0).unwrap_err();
        assert!(
            error.contains("disk 0"),
            "disk 0 must be refused before any disk query: {error}"
        );
    }

    #[test]
    fn each_lettered_volume_keeps_its_own_disk_number() {
        let text = r#"[{"DriveLetter":"E","DiskNumber":2},{"DriveLetter":"F","DiskNumber":4}]"#;
        let rows = super::parse_lettered_partitions(text).unwrap();
        assert_eq!(super::disk_number_of("E", &rows), 2);
        assert_eq!(super::disk_number_of("F:", &rows), 4);
        let one = r#"{"DriveLetter":"G","DiskNumber":3}"#;
        let rows = super::parse_lettered_partitions(one).unwrap();
        assert_eq!(super::disk_number_of("g", &rows), 3);
        assert_eq!(super::disk_number_of("Z", &rows), 0);
        let system = r#"[{"DriveLetter":"C","DiskNumber":0},{"DriveLetter":"E","DiskNumber":2}]"#;
        let rows = super::parse_lettered_partitions(system).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(super::disk_number_of("C", &rows), 0);
        assert_eq!(super::disk_number_of("E", &rows), 2);
    }
}
