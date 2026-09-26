use std::fmt;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug)]
pub struct ProfileError {
    message: String,
}

impl ProfileError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProfileError {}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    #[serde(rename = "version", default = "default_version")]
    pub version: u32,
    pub name: String,
    pub cfw: String,
    #[serde(
        rename = "deviceFamily",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub device_family: Option<String>,
    #[serde(rename = "storageModes")]
    pub storage_modes: Vec<String>,
    #[serde(rename = "romSchema")]
    pub rom_schema: RomSchema,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageSource>,
}

fn default_version() -> u32 {
    1
}

/// System folders become path segments on the card and in staging, so they
/// must be single, relative, alphanumeric components. Enforced at load time
/// so every consumer (copy, seed, staging) can trust profile values.
pub fn is_safe_folder(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['\\', '/', ':'])
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
}

/// datNamePattern reaches igir as a raw argv element; quotes and control
/// characters could break argument boundaries.
pub fn is_safe_pattern(pattern: &str) -> bool {
    !pattern
        .chars()
        .any(|c| c == '"' || c == '\'' || c.is_control())
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct ImageSource {
    pub url: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compressed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub parts: Vec<ImagePart>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct ImagePart {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct RomSchema {
    pub layout: String,
    #[serde(rename = "formatFs", default, skip_serializing_if = "Option::is_none")]
    pub format_fs: Option<String>,
    #[serde(
        rename = "volumeLabel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub volume_label: Option<String>,
    #[serde(default)]
    pub systems: Vec<SystemFolder>,
    #[serde(rename = "biosFolder", default, skip_serializing_if = "Option::is_none")]
    pub bios_folder: Option<String>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct SystemFolder {
    pub id: String,
    pub folder: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(
        rename = "datNamePattern",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dat_name_pattern: Option<String>,
}

pub fn schema_validator(schema_path: &Path) -> Result<jsonschema::Validator, ProfileError> {
    let schema_text = fs::read_to_string(schema_path).map_err(|error| {
        ProfileError::new(format!(
            "could not read schema {}: {error}",
            schema_path.display()
        ))
    })?;
    let schema: Value = serde_json::from_str(&schema_text)
        .map_err(|error| ProfileError::new(format!("schema is not JSON: {error}")))?;
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .map_err(|error| ProfileError::new(format!("schema failed to compile: {error}")))
}

pub fn load_profiles(dir: &Path, schema_path: &Path) -> Result<Vec<Profile>, ProfileError> {
    let validator = schema_validator(schema_path)?;

    let mut entries = fs::read_dir(dir)
        .map_err(|error| {
            ProfileError::new(format!(
                "could not read profiles {}: {error}",
                dir.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ProfileError::new(format!("could not list profiles: {error}")))?;
    entries.sort_by_key(|entry| entry.file_name());

    let mut profiles = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("profile.json");
        let text = fs::read_to_string(&path)
            .map_err(|error| ProfileError::new(format!("could not read {file_name}: {error}")))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| ProfileError::new(format!("{file_name} is not JSON: {error}")))?;
        let errors = validator.iter_errors(&value).into_errors();
        if !errors.is_empty() {
            return Err(ProfileError::new(format!(
                "{file_name} does not match the profile schema: {errors}"
            )));
        }
        let profile: Profile = serde_json::from_value(value).map_err(|error| {
            ProfileError::new(format!(
                "{file_name} could not be read as a profile: {error}"
            ))
        })?;
        for system in &profile.rom_schema.systems {
            if !is_safe_folder(&system.folder) {
                return Err(ProfileError::new(format!(
                    "{file_name} has an unsafe system folder: {}",
                    system.folder
                )));
            }
            if let Some(pattern) = &system.dat_name_pattern {
                if !is_safe_pattern(pattern) {
                    return Err(ProfileError::new(format!(
                        "{file_name} has an unsafe datNamePattern: {pattern}"
                    )));
                }
            }
        }
        if let Some(bios) = &profile.rom_schema.bios_folder {
            if !is_safe_folder(bios) {
                return Err(ProfileError::new(format!(
                    "{file_name} has an unsafe bios folder: {bios}"
                )));
            }
        }
        profiles.push(profile);
    }

    profiles.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(profiles)
}
