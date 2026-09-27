//! Card Doctor (PR 7): facts serialization + deterministic explanations,
//! with an optional engine paraphrase.
//!
//! Display-only, always: the doctor READS what the gates already see and
//! explains it; it never changes a gate outcome. Two layers:
//!
//! 1. **Deterministic templates** for known states and refusals, rendered
//!    from `CardFacts` alone (engine off). Stable strings the UI renders
//!    verbatim and tests pin.
//! 2. **Engine paraphrase** (engine on): one reset-then-complete turn over
//!    the serialized facts, vocabulary-validated into the
//!    `card_doctor.json` tool vocabulary, used as the human-facing text
//!    beside the template, plus the likely-profile suggestion.
//!
//! Garbage reaches neither layer's truths: the templates match on known
//! shapes and degrade to a generic explanation; the engine call is
//! vocabulary-validated, so an unexpected answer is dropped in favor of
//! the template. Tests assert the gates produce byte-identical results
//! with the engine off, on, and garbage-fed.

use serde::{Deserialize, Serialize};

use super::client::ServeClient;
use crate::firstboot::CardSafety;
use crate::volume::VolumeInfo;

/// Everything the gates already see about one card, serialized for the
/// doctor. Paths stay on the box: only labels, decisions, folder names,
/// and gate outcomes cross into the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardFacts {
    /// Volume the user selected in the app.
    pub volume_letter: String,
    pub volume_label: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub is_empty: bool,
    /// The gate decision already rendered for this volume (ready /
    /// needs_format / rejected) plus its reason.
    pub decision: String,
    pub decision_reason: String,
    /// Sibling volumes on the same disk (letter + label), capped.
    pub disk_volumes: Vec<VolumeDisk>,
    /// Top-level folder names on the card root (capped), for family id.
    pub root_folders: Vec<String>,
    /// Gate outcomes in their own words.
    pub firstboot_state: String,
    pub firstboot_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeDisk {
    pub letter: String,
    pub label: String,
}

/// One rendered doctor answer. `engine_text`/`suggested_profile_id` are
/// `None` when the engine is off or its answer fails validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub heading: String,
    pub steps: Vec<String>,
    pub engine_text: Option<String>,
    pub suggested_profile_id: Option<String>,
}

/// The card families the doctor (and the tool vocabulary) knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardFamily {
    ArkosEasyroms,
    RocknixShare,
    R36sClone,
    R35sStock,
    DarkosEasyroms,
    Unknown,
}

impl CardFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            CardFamily::ArkosEasyroms => "arkos_easyroms",
            CardFamily::RocknixShare => "rocknix_share",
            CardFamily::R36sClone => "r36s_clone",
            CardFamily::R35sStock => "r35s_stock",
            CardFamily::DarkosEasyroms => "darkos_easyroms",
            CardFamily::Unknown => "unknown",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(name: &str) -> Self {
        match name {
            "arkos_easyroms" => CardFamily::ArkosEasyroms,
            "rocknix_share" => CardFamily::RocknixShare,
            "r36s_clone" => CardFamily::R36sClone,
            "r35s_stock" => CardFamily::R35sStock,
            "darkos_easyroms" => CardFamily::DarkosEasyroms,
            _ => CardFamily::Unknown,
        }
    }
}

/// Families we may suggest, in vocabulary order. Anything else from the
/// engine is dropped.
pub const KNOWN_PROFILE_IDS: &[&str] = &[
    "arkos-roms-card",
    "rocknix-roms-card",
    "rocknix-rgb10x-os",
    "r36s-clone-card",
    "r35s-stock-card",
    "darkos-rgb10x-os",
];

/// Identifies the card family from gate-visible facts alone. Order
/// matters: specific signatures before label matching.
pub fn identify_family(facts: &CardFacts) -> CardFamily {
    let has = |name: &str| {
        facts
            .root_folders
            .iter()
            .any(|f| f.eq_ignore_ascii_case(name))
    };
    let label = facts.volume_label.to_ascii_lowercase();
    // Stock R35S layout: lowercase roms/* folders (@tf) with mixed-case
    // Easytitles over them; Roms/FC for NES is the fingerprint.
    let lower_folders: Vec<String> = facts
        .root_folders
        .iter()
        .map(|f| f.to_ascii_lowercase())
        .collect();
    if lower_folders
        .iter()
        .any(|f| f == "roms/fc" || f == "roms" && has("Emulators"))
    {
        return CardFamily::R35sStock;
    }
    // Clone-card stock partition: top-level Roms/* short names.
    if lower_folders
        .iter()
        .any(|f| f == "roms/psp" || f == "roms/neogeo")
    {
        return CardFamily::R36sClone;
    }
    if label == "share" || has("roms") && label == "share" {
        return CardFamily::RocknixShare;
    }
    if label == "easyroms" {
        return CardFamily::DarkosEasyroms;
    }
    CardFamily::Unknown
}

/// Deterministic templates: one heading + ordered steps per known shape.
/// Corrupted firstboot degrades to the generic explanation before any
/// family heading: when the gates cannot verify first boot, the card
/// is never described as a ready family card.
pub fn explain(facts: &CardFacts) -> DoctorReport {
    // A corrupted/unknown firstboot state takes priority over the
    // family templates: the card is not verifiable, so no ready
    // heading may claim otherwise.
    if facts.firstboot_state != "safe"
        && facts.firstboot_state != "armed"
        && facts.decision != "rejected"
    {
        return DoctorReport {
            heading: "The app cannot verify first boot on this card".into(),
            steps: vec![
                facts.firstboot_reason.clone(),
                "Check that the card's small BOOT partition is visible in Windows, reinsert the card, and try again.".into(),
            ],
            engine_text: None,
            suggested_profile_id: None,
        };
    }
    match (facts.decision.as_str(), facts.firstboot_state.as_str()) {
        ("rejected", _) => DoctorReport {
            heading: format!("This card is not usable as a {} card", facts.volume_label),
            steps: vec![
                if facts.decision_reason.is_empty() {
                    "The volume failed the safety check.".to_string()
                } else {
                    facts.decision_reason.clone()
                },
                "Use a different removable card, or erase and prepare this one from the Prepare step.".into(),
            ],
            engine_text: None,
            suggested_profile_id: None,
        },
        (_, "armed") => DoctorReport {
            heading: "This card has not finished its first boot".into(),
            steps: vec![
                format!(
                    "The handheld would format {} and erase anything copied now. {}",
                    facts.volume_label, facts.firstboot_reason
                ),
                "Boot the handheld once until the game menu appears, shut down from the menu, then put the card back.".into(),
            ],
            engine_text: None,
            suggested_profile_id: None,
        },
        (_, "unknown") => DoctorReport {
            heading: "The app cannot verify first boot on this card".into(),
            steps: vec![
                facts.firstboot_reason.clone(),
                "Check that the card's small BOOT partition is visible in Windows, reinsert the card, and try again.".into(),
            ],
            engine_text: None,
            suggested_profile_id: None,
        },
        _ => match identify_family(facts) {
            CardFamily::Unknown => DoctorReport {
                heading: "This card looks ready, but the family is unfamiliar".into(),
                steps: vec![
                    format!(
                        "Volume {} ({}), {} top-level folders. Copying should work, but double-check the profile matches the handheld.",
                        facts.volume_letter,
                        facts.volume_label,
                        facts.root_folders.len()
                    ),
                ],
                engine_text: None,
                suggested_profile_id: None,
            },
            family => DoctorReport {
                heading: family_heading(family),
                steps: vec![family_steps(family)],
                engine_text: None,
                suggested_profile_id: None,
            },
        },
    }
}

fn family_heading(family: CardFamily) -> String {
    match family {
        CardFamily::ArkosEasyroms => "This is an ArkOS EASYROMS card".into(),
        CardFamily::RocknixShare => "This is a ROCKNIX share card".into(),
        CardFamily::R36sClone => "This is an R36S clone stock card".into(),
        CardFamily::R35sStock => "This is an R35S stock card".into(),
        CardFamily::DarkosEasyroms => "This is a dArkOS EASYROMS card".into(),
        CardFamily::Unknown => "This card looks ready, but the family is unfamiliar".into(),
    }
}

fn family_steps(family: CardFamily) -> String {
    match family {
        CardFamily::ArkosEasyroms => {
            "Copying is safe now that first boot has run: games go to the system folders.".into()
        }
        CardFamily::RocknixShare => {
            "Copying is safe: games go under roms/<system> on the share partition.".into()
        }
        CardFamily::R36sClone => {
            "Use the R36S clone profile: it writes Roms/<NAME> with the stock short names.".into()
        }
        CardFamily::R35sStock => {
            "Use the R35S stock profile: it writes roms/<system> with the lowercase folders this card expects.".into()
        }
        CardFamily::DarkosEasyroms => {
            "Copying is safe now that first boot has run: games go to the system folders.".into()
        }
        CardFamily::Unknown => {
            "The layout is unfamiliar: verify the profile matches the handheld before copying.".into()
        }
    }
}

/// Engine paraphrase: one turn over the serialized facts, vocabulary
/// validated. Any failure or out-of-vocabulary answer falls back to the
/// template — the engine can only ADD a paraphrase, never replace the
/// gate truth.
pub fn paraphrase_with_engine(
    facts: &CardFacts,
    client: &ServeClient,
) -> (Option<String>, Option<String>) {
    let input = match serde_json::to_string(facts) {
        Ok(json) => format!("Explain this card. CardFacts: {json}"),
        Err(_) => return (None, None),
    };
    let Ok(turn) = client.ask(&input) else {
        return (None, None);
    };
    match parse_engine_turn(&turn.call.name, &turn.call.arguments) {
        Some((text, profile)) => (text, profile),
        None => (None, None),
    }
}

/// Vocabulary-validates one engine turn into an optional paraphrase and
/// likely-profile suggestion. Wrong tool name, missing or non-string
/// fields, or a profile outside the shipped vocabulary all reject the
/// turn (returns None instead). Missing family or observation degrade
/// gracefully: a grounded profile id still yields a profile-only
/// suggestion (the template supplies the words), because the suggestion
/// is the part of the answer anchored in shipped vocabulary.
pub fn parse_engine_turn(
    name: &str,
    arguments: &serde_json::Value,
) -> Option<(Option<String>, Option<String>)> {
    if name != "explain_card" {
        return None;
    }
    let observation = arguments.get("observation").and_then(|v| v.as_str());
    let family = arguments
        .get("card_family")
        .and_then(|v| v.as_str())
        .map(CardFamily::from_str);
    // A present-but-foreign profile id rejects the whole turn, even
    // when observation and family are otherwise fine: the suggestion
    // is the only part of the answer anchored in shipped vocabulary,
    // so a foreign id means the engine went off-script.
    if let Some(id) = arguments
        .get("suggested_profile_id")
        .and_then(|v| v.as_str())
    {
        if !KNOWN_PROFILE_IDS.contains(&id) {
            return None;
        }
    }
    let profile = arguments
        .get("suggested_profile_id")
        .and_then(|v| v.as_str())
        .filter(|id| KNOWN_PROFILE_IDS.contains(id))
        .map(str::to_string);
    match (observation, family, profile) {
        (Some(text), Some(_), profile) => Some((Some(text.to_string()), profile)),
        (None, _, Some(profile)) => Some((None, Some(profile))),
        _ => None,
    }
}

/// Full diagnosis: template always; engine paraphrase when a client is
/// provided. `CardSafety` is re-derived from `facts.firstboot_state` for
/// the equivalence tests (the gates themselves are never re-run here).
pub fn diagnose(facts: &CardFacts, client: Option<&ServeClient>) -> DoctorReport {
    let mut report = explain(facts);
    if let Some(client) = client {
        let (text, profile) = paraphrase_with_engine(facts, client);
        if text.is_some() || profile.is_some() {
            report.engine_text = text;
            report.suggested_profile_id = profile;
        }
    }
    report
}

/// Re-derives `CardSafety` purely from the serialized gate outcome, for
/// the equivalence property: the same facts always explain the same way
/// regardless of engine presence.
pub fn safety_of(facts: &CardFacts) -> CardSafety {
    match facts.firstboot_state.as_str() {
        "safe" => CardSafety::Safe,
        "armed" => CardSafety::Armed(facts.firstboot_reason.clone()),
        _ => CardSafety::Unknown(facts.firstboot_reason.clone()),
    }
}

/// Collects `CardFacts` from what the gates already see. All inputs are
/// provided by the caller (the command layer runs the real gates); the
/// root folder listing is capped so a bloated card cannot grow the
/// input unbounded.
pub fn collect_facts(
    volume: &VolumeInfo,
    decision: &str,
    decision_reason: &str,
    disk_volumes: Vec<VolumeDisk>,
    safety: &CardSafety,
    card_root: &std::path::Path,
) -> CardFacts {
    let mut root_folders = Vec::new();
    if let Ok(entries) = std::fs::read_dir(card_root) {
        for entry in entries.flatten().take(64) {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    root_folders.push(name.to_string());
                }
            }
        }
    }
    root_folders.sort();
    let (state, reason) = match safety {
        CardSafety::Safe => ("safe".to_string(), String::new()),
        CardSafety::Armed(reason) => ("armed".to_string(), reason.clone()),
        CardSafety::Unknown(reason) => ("unknown".to_string(), reason.clone()),
    };
    CardFacts {
        volume_letter: volume.letter.clone(),
        volume_label: volume.label.clone(),
        file_system: volume.file_system.clone(),
        total_bytes: volume.total_bytes,
        is_empty: volume.is_empty,
        decision: decision.to_string(),
        decision_reason: decision_reason.to_string(),
        disk_volumes,
        root_folders,
        firstboot_state: state,
        firstboot_reason: reason,
    }
}

/// The boot-marker sibling files the doctor understands. Matches the
/// gate's own vocabulary (`firstboot.rs::WIPE_SCRIPTS`) so summaries
/// stay consistent — duplicated here only as names, never as policy.
pub const WATCHED_MARKERS: &[&str] = &["expandtoexfat.sh", "firstboot.sh"];

/// The empty-reason sentinel for garbage-equivalence tests: an
/// explicitly constructed empty string, distinct from any reason the
/// gates or fixtures would ever produce by mutation.
impl CardFacts {
    pub fn default_reason() -> String {
        String::new()
    }
}

pub fn marker_summary(boot_root: Option<&std::path::Path>) -> Vec<String> {
    let Some(root) = boot_root else {
        return Vec::new();
    };
    WATCHED_MARKERS
        .iter()
        .filter(|name| root.join(name).is_file())
        .map(|name| name.to_string())
        .collect()
}
