use std::fs;
use std::path::{Component, Path};

use crate::volume::VolumeDecision;

pub fn seed_folders(
    root: &Path,
    folders: &[String],
    decision: &VolumeDecision,
) -> Result<Vec<String>, String> {
    match decision {
        VolumeDecision::Ready { .. } => {}
        VolumeDecision::NeedsFormat { reason } | VolumeDecision::Rejected { reason } => {
            return Err(format!("refusing to seed this card: {reason}"));
        }
    }
    if !root.is_dir() {
        return Err(format!("card root {} is not a directory", root.display()));
    }

    let mut created = Vec::new();
    for folder in folders {
        let relative = normalize_folder(folder)?;
        let destination = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        fs::create_dir_all(&destination)
            .map_err(|error| format!("could not create {relative}: {error}"))?;
        created.push(relative);
    }
    Ok(created)
}

fn normalize_folder(folder: &str) -> Result<String, String> {
    let path = Path::new(folder);
    if path.is_absolute() {
        return Err(format!("{folder} escapes the card"));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.is_empty() || part == "." {
                    continue;
                }
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("{folder} escapes the card"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{folder} escapes the card"));
            }
        }
    }
    if parts.is_empty() {
        return Err("folder path is empty".into());
    }
    Ok(parts.join("/"))
}
