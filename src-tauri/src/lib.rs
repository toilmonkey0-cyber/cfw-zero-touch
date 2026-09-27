pub mod diag;
pub mod feed;
pub mod firstboot;
pub mod flash;
pub mod folder_map;
pub mod format_gate;
pub mod igir;
pub mod needle;
pub mod prepare;
pub mod profiles;
pub mod romcopy;
pub mod seed;
pub mod store;
pub mod volume;

use std::path::{Path, PathBuf};

use profiles::Profile;
use romcopy::CopyPlan;
use serde::Serialize;
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
struct LetteredPartition {
    letter: char,
    disk: u32,
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
        let disk = row.get("DiskNumber").and_then(|value| value.as_u64()).unwrap_or(0) as u32;
        partitions.push(LetteredPartition { letter, disk });
    }
    Ok(partitions)
}

/// Judge whether ROMs may be copied onto this card for this profile.
///
/// Only flashed single-card ArkOS/dArkOS profiles carry the firstboot wipe;
/// ROMs-only cards have no BOOT partition and are always safe to fill.
fn check_card_safety(profile: &Profile, volume: &VolumeInfo) -> Result<firstboot::CardSafety, String> {
    let gate_applies =
        profile.image.is_some() && profile.rom_schema.layout == "arkos_easyroms_root";
    if !gate_applies {
        return Ok(firstboot::CardSafety::Safe);
    }
    let partitions = lettered_partitions()?;
    let Some(disk) = partitions
        .iter()
        .find(|row| row.letter.eq_ignore_ascii_case(&volume.letter.chars().next().unwrap_or('?')))
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
                .find(|volume| volume.letter.chars().next() == Some(row.letter))
                .map(|volume| (row.letter, volume.label.clone()))
        })
        .collect();
    let boot_root = firstboot::boot_letter(&disk_volumes)
        .map(|letter| PathBuf::from(format!("{letter}:\\")));
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
            diag::log("WARN", "firstboot_refused", &format!("volume={}: {reason}", volume.letter));
            ("armed".into(), reason)
        }
        firstboot::CardSafety::Unknown(reason) => {
            diag::log("WARN", "firstboot_unverified", &format!("volume={}: {reason}", volume.letter));
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
}

fn view_volume(volume: VolumeInfo, expected_label: &str) -> VolumeView {
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
    Ok(volumes
        .into_iter()
        .map(|volume| view_volume(volume, &label))
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
    let file_system = profile
        .rom_schema
        .format_fs
        .as_deref()
        .unwrap_or("exFAT");
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
    Ok(view_volume(refreshed, &label))
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

#[tauri::command]
fn plan_roms(
    app: tauri::AppHandle,
    profile_id: String,
    volume_id: String,
    library: String,
    include: Vec<String>,
) -> Result<PlanView, String> {
    let profile = profile_by_id(&app, &profile_id)?;
    let volume = require_volume(&volume_id)?;
    let plan = build_plan(&profile, &volume, Path::new(&library), &include)?;
    Ok(plan_view(plan))
}

#[tauri::command]
fn copy_roms(
    app: tauri::AppHandle,
    profile_id: String,
    volume_id: String,
    library: String,
    include: Vec<String>,
    dry_run: bool,
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
    let plan = build_plan(&profile, &volume, Path::new(&library), &include)?;
    diag::log(
        "INFO",
        "copy_start",
        &format!(
            "profile={profile_id} volume={} files={} dry_run={dry_run}",
            volume.letter,
            plan.items.len()
        ),
    );
    let total = plan.items.len();
    let root = card_root(&volume);
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

fn prepare_split_7z(app: &tauri::AppHandle, image: &profiles::ImageSource) -> Result<PathBuf, String> {
    let first = image
        .parts
        .first()
        .ok_or("this 7z image has no download parts")?;
    let image_name = flash::extracted_image_name(&flash::image_file_name(&first.url)?)?;
    let directories = image_cache_dirs();
    for directory in &directories {
        let candidate = directory.join(&image_name);
        if candidate.is_file() {
            let hash = flash::sha256_reader(std::fs::File::open(&candidate).map_err(|error| {
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
    let exe = std::env::current_exe().map_err(|error| format!("could not find this app: {error}"))?;
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
    let image = profile
        .image
        .ok_or("this profile has no OS image")?;
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
    let script = format!(
        "& '{tool}' --image='{image}' --sha256='{sha}' --disk={disk_number} --confirm='{confirmation}' *>&1 | Tee-Object -FilePath '{log}'\r\nexit $LASTEXITCODE\r\n",
        tool = tool.display().to_string().replace('\'', "''"),
        image = image_path.display().to_string().replace('\'', "''"),
        sha = image.sha256.replace('\'', ""),
        log = log.display().to_string().replace('\'', "''"),
    );
    std::fs::write(&script_path, script).map_err(|error| format!("could not write the launcher: {error}"))?;
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
    let text = std::fs::read_to_string(&log).unwrap_or_default();
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
    diag::log("INFO", "flash_done", &format!("disk={disk_number} profile={profile_id}"));
    Ok(text)
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
                &format!("status={status} updates={} additions={}",
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
            diag::log("INFO", "feed_check", &format!("status=error reason={reason}"));
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
    Err("igir sorting needs either igir on PATH or Node.js installed so igir can run through npx".into())
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
            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
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
            diag::log("ERROR", "stage_failed", &format!("profile={profile_id}: {error}"));
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
            list_flash_disks,
            flash_os,
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
            needle_acquire
        ])
        .build(tauri::generate_context!())
        .expect("error while building the application")
        .run(|_app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // No helper process may outlive the app.
                needle::embed_client::kill_all();
            }
        });
}
