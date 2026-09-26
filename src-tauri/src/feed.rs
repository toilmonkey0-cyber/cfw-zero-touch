use std::collections::HashSet;
use std::io::Read;

use crate::profiles::Profile;

pub const FEED_VERSION: u32 = 1;
pub const DEFAULT_FEED_URL: &str =
    "https://raw.githubusercontent.com/toilmonkey0-cyber/cfw-zero-touch/main/profiles-feed.json";

pub fn feed_url() -> String {
    std::env::var("CFW_FEED_URL").unwrap_or_else(|_| DEFAULT_FEED_URL.into())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedApp {
    pub version: String,
    pub release_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileFeed {
    pub feed_version: u32,
    pub app: FeedApp,
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileChange {
    pub id: String,
    pub name: String,
    pub local_version: u32,
    pub remote_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FeedDiff {
    pub updates: Vec<ProfileChange>,
    pub additions: Vec<Profile>,
    pub app_update: Option<FeedApp>,
}

/// Validate a feed body against the profile schema. Any invalid entry,
/// duplicate id, or unknown feedVersion rejects the whole feed so apply can
/// never be partial.
pub fn parse_feed(text: &str, validator: &jsonschema::Validator) -> Result<ProfileFeed, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("feed is not JSON: {error}"))?;
    let feed_version = value.get("feedVersion").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if feed_version != FEED_VERSION {
        return Err(format!(
            "unsupported feed version {feed_version}; this app understands version {FEED_VERSION}"
        ));
    }
    let entries = value
        .get("profiles")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut profiles = Vec::new();
    let mut seen = HashSet::new();
    for entry in &entries {
        let errors = validator.iter_errors(entry).into_errors();
        if !errors.is_empty() {
            return Err(format!(
                "a feed profile does not match the profile schema: {errors}"
            ));
        }
        let profile: Profile = serde_json::from_value(entry.clone())
            .map_err(|error| format!("a feed profile could not be read: {error}"))?;
        if !seen.insert(profile.id.clone()) {
            return Err(format!("feed lists profile {} twice", profile.id));
        }
        profiles.push(profile);
    }
    let app: FeedApp =
        serde_json::from_value(value.get("app").cloned().unwrap_or(serde_json::Value::Null))
            .map_err(|error| format!("feed app section is invalid: {error}"))?;
    // The release URL goes to the OS opener (ShellExecute), which executes
    // files — only https may ever reach it.
    if !app.release_url.starts_with("https://") {
        return Err(format!(
            "feed releaseUrl must be an https URL, got {:?}",
            app.release_url
        ));
    }
    Ok(ProfileFeed {
        feed_version,
        app,
        profiles,
    })
}

pub fn diff_feed(local: &[Profile], feed: &ProfileFeed, running_version: &str) -> FeedDiff {
    let mut updates = Vec::new();
    let mut additions = Vec::new();
    for remote in &feed.profiles {
        match local.iter().find(|current| current.id == remote.id) {
            Some(current) if remote.version > current.version => updates.push(ProfileChange {
                id: remote.id.clone(),
                name: remote.name.clone(),
                local_version: current.version,
                remote_version: remote.version,
            }),
            Some(_) => {}
            None => additions.push(remote.clone()),
        }
    }
    let app_update = if app_version_newer(&feed.app.version, running_version) {
        Some(feed.app.clone())
    } else {
        None
    };
    FeedDiff {
        updates,
        additions,
        app_update,
    }
}

fn version_parts(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Semver-aware on major.minor.patch with a plain string fallback.
pub fn app_version_newer(remote: &str, local: &str) -> bool {
    match (version_parts(remote), version_parts(local)) {
        (Some(remote), Some(local)) => remote > local,
        _ => remote != local && remote > local,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub applied: usize,
    pub skipped: usize,
}

/// Write feed entries that are newer or new into the store. An entry whose
/// id already lives in a file keeps that filename (e.g. an `.example.json`
/// seed), so updates land where the profile is listed from.
pub fn apply_feed(
    store_dir: &std::path::Path,
    feed: &ProfileFeed,
    local: &[Profile],
) -> Result<ApplyReport, String> {
    let mut report = ApplyReport {
        applied: 0,
        skipped: 0,
    };
    for remote in &feed.profiles {
        if let Some(current) = local.iter().find(|current| current.id == remote.id) {
            if remote.version <= current.version {
                report.skipped += 1;
                continue;
            }
        }
        let target = match profile_file_for_id(store_dir, &remote.id)? {
            Some(path) => path,
            None => store_dir.join(format!("{}.json", remote.id)),
        };
        let text = serde_json::to_string_pretty(remote)
            .map_err(|error| format!("could not encode profile {}: {error}", remote.id))?;
        std::fs::write(&target, text)
            .map_err(|error| format!("could not write {}: {error}", target.display()))?;
        report.applied += 1;
    }
    Ok(report)
}

fn profile_file_for_id(
    store_dir: &std::path::Path,
    id: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let entries = std::fs::read_dir(store_dir)
        .map_err(|error| format!("could not read {}: {error}", store_dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(profile) = serde_json::from_str::<Profile>(&text) {
            if profile.id == id {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

/// Fetch the feed body over HTTPS with a hard timeout. The only network
/// touch in this module; commands treat failure as a soft state.
pub fn fetch_text(url: &str) -> Result<String, String> {
    let response = ureq::get(url)
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|error| format!("could not reach the profile feed: {error}"))?;
    let mut body = String::new();
    response
        .into_reader()
        .take(4 * 1024 * 1024)
        .read_to_string(&mut body)
        .map_err(|error| format!("could not read the profile feed: {error}"))?;
    Ok(body)
}
