use std::fs;
use std::path::{Path, PathBuf};

use crate::folder_map::storage_folder;
use crate::profiles::SystemFolder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyAction {
    Copy,
    SkipUnchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyItem {
    pub source: PathBuf,
    pub relative_dest: String,
    pub bytes: u64,
    pub action: CopyAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyPlan {
    pub items: Vec<CopyItem>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyReport {
    pub copied: usize,
    pub skipped: usize,
    pub bytes_copied: u64,
}

pub fn plan_copy(
    library: &Path,
    dest_root: &Path,
    layout: &str,
    systems: &[SystemFolder],
    include: &[String],
    bios_folder: Option<&str>,
) -> Result<CopyPlan, String> {
    if !library.is_dir() {
        return Err(format!("{} is not a library folder", library.display()));
    }

    let mut items = Vec::new();
    for system in systems {
        if !include.is_empty() && !include.iter().any(|id| id.eq_ignore_ascii_case(&system.id)) {
            continue;
        }
        // The folder is a path fragment in the library and on the card; a
        // hostile profile value must be refused whether or not a matching
        // library folder exists.
        ensure_safe_dest(&system.folder)?;
        let Some(source_dir) = find_named_dir(library, &[&system.folder, &system.id]) else {
            continue;
        };
        let dest_prefix = storage_folder(layout, &system.folder)?;
        ensure_safe_dest(&dest_prefix)?;
        collect_files(
            &source_dir,
            &source_dir,
            &system.extensions,
            &dest_prefix,
            dest_root,
            &mut items,
        )?;
    }

    if let Some(bios_folder) = bios_folder {
        if let Some(source_dir) = find_named_dir(library, &["bios"]) {
            let bios_prefix = storage_folder(layout, bios_folder)?;
            ensure_safe_dest(&bios_prefix)?;
            collect_files(
                &source_dir,
                &source_dir,
                &[],
                &bios_prefix,
                dest_root,
                &mut items,
            )?;
        }
    }

    let warning = if items.is_empty() {
        Some("No ROM or BIOS files matched this profile.".into())
    } else {
        None
    };
    Ok(CopyPlan { items, warning })
}

pub fn execute_copy(
    plan: &CopyPlan,
    dest_root: &Path,
    dry_run: bool,
) -> Result<CopyReport, String> {
    let mut report = CopyReport {
        copied: 0,
        skipped: 0,
        bytes_copied: 0,
    };
    for item in &plan.items {
        match item.action {
            CopyAction::SkipUnchanged => report.skipped += 1,
            CopyAction::Copy => {
                report.copied += 1;
                if dry_run {
                    continue;
                }
                let destination = dest_root.join(
                    item.relative_dest
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                if let Err(error) = ensure_safe_dest(&item.relative_dest) {
                    return Err(format!(
                        "refusing to write {}: {error}",
                        destination.display()
                    ));
                }
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!("could not create {}: {error}", parent.display())
                    })?;
                }
                // Write to a temp name and rename, so an interrupted run never
                // leaves a truncated file that looks like a complete ROM.
                let mut temp_name = destination.clone().into_os_string();
                temp_name.push(".cfwpart");
                let temp = PathBuf::from(temp_name);
                fs::copy(&item.source, &temp).map_err(|error| {
                    format!("could not copy {}: {error}", item.source.display())
                })?;
                if destination.exists() {
                    fs::remove_file(&destination).map_err(|error| {
                        format!("could not replace {}: {error}", destination.display())
                    })?;
                }
                fs::rename(&temp, &destination).map_err(|error| {
                    format!("could not finish copying {}: {error}", item.source.display())
                })?;
                report.bytes_copied += item.bytes;
            }
        }
    }
    Ok(report)
}

fn collect_files(
    root: &Path,
    current: &Path,
    extensions: &[String],
    dest_prefix: &str,
    dest_root: &Path,
    items: &mut Vec<CopyItem>,
) -> Result<(), String> {
    let entries = fs::read_dir(current)
        .map_err(|error| format!("could not read {}: {error}", current.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read a library entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, extensions, dest_prefix, dest_root, items)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        if !extensions.is_empty() && !extension_matches(&path, extensions) {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(path.as_path());
        let relative = relative.to_string_lossy().replace('\\', "/");
        let relative_dest = if dest_prefix.is_empty() {
            relative
        } else {
            format!("{dest_prefix}/{relative}")
        };
        let bytes = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        let destination = dest_root.join(relative_dest.replace('/', std::path::MAIN_SEPARATOR_STR));
        let action = if same_size(&destination, bytes) {
            CopyAction::SkipUnchanged
        } else {
            CopyAction::Copy
        };
        items.push(CopyItem {
            source: path,
            relative_dest,
            bytes,
            action,
        });
    }
    Ok(())
}

fn extension_matches(path: &Path, extensions: &[String]) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    extensions.iter().any(|extension| {
        name.len() > extension.len()
            && name[name.len() - extension.len()..].eq_ignore_ascii_case(extension)
    })
}

fn same_size(destination: &Path, bytes: u64) -> bool {
    destination
        .metadata()
        .map(|meta| meta.is_file() && meta.len() == bytes)
        .unwrap_or(false)
}

/// Mapped destination prefixes become path segments under the card root, so
/// a hostile profile value must never be able to climb out of it.
fn ensure_safe_dest(dest: &str) -> Result<(), String> {
    for part in dest.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return Err(format!("unsafe destination folder \"{dest}\""));
        }
    }
    Ok(())
}

fn find_named_dir(library: &Path, names: &[&str]) -> Option<PathBuf> {
    let entries = fs::read_dir(library).ok()?;
    let mut found = vec![None; names.len()];
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        for (index, name) in names.iter().enumerate() {
            if file_name.eq_ignore_ascii_case(name) && found[index].is_none() {
                found[index] = Some(entry.path());
            }
        }
    }
    found.into_iter().flatten().next()
}
