//! Stem cleaning for embedding and grouping.
//!
//! Port of the spike's `clean_stem` (validated in the ingestion repo's
//! embedding experiment): index entries and queries must embed the SAME
//! cleaned form or cosine similarity collapses — raw messy stems drift
//! into the unrelated band. The cleaner strips folder prefixes,
//! extensions, region/disc/scene tag groups, junk tokens, and pure digit
//! tokens; separators normalize to single spaces; output is lowercase.

use std::path::Path;

/// Tokens that never carry identity: scene junk, disc/track markers,
/// extension stems, region markers, revision noise.
const JUNK_WORDS: &[&str] = &[
    "rip", "proper", "repack", "track", "track1", "track01", "disc", "disk",
    "bin", "cue", "iso", "img", "gdi", "chd", "7z", "zip", "usa", "us",
    "europe", "eu", "japan", "jpn", "world", "ntsc", "pal", "rev", "v1",
];

/// Hard cap matching the embed-client stem cap.
pub const MAX_STEM_BYTES: usize = 4096;

/// Cleans a file name (or folder-prefixed path) into its canonical stem.
pub fn clean_stem(input: &str) -> String {
    // Drop any folder prefix; work on the file name only.
    let name = input.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or("");

    // Drop the extension (last dot segment, short and alphanumeric).
    let mut end = name.len();
    if let Some(dot) = name.rfind('.') {
        let ext = &name[dot + 1..];
        if !ext.is_empty() && ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            end = dot;
        }
    }
    let name = &name[..end];

    // Drop (...) [...] {...} tag groups.
    let mut stripped = String::with_capacity(name.len());
    let mut depth = 0usize;
    for c in name.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => stripped.push(c),
            _ => {}
        }
    }

    // Separators to spaces, lowercase, drop junk and digit-only tokens.
    let normalized = stripped.replace(['_', '.', '-'], " ").to_lowercase();
    let mut tokens: Vec<&str> = normalized.split_whitespace().collect();
    tokens.retain(|token| {
        if JUNK_WORDS.contains(token) || token.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        // disc3 / disk2 / track01 shaped tokens are disc/track tags.
        for prefix in ["disc", "disk", "track"] {
            if let Some(rest) = token.strip_prefix(prefix) {
                if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                    return false;
                }
            }
        }
        true
    });

    let joined = tokens.join(" ");
    if joined.is_empty() {
        // Every token was junk (e.g. "(USA) (Disc 1)"): fall back to the
        // tag-stripped raw name so stems stay non-empty and unique-ish.
        let fallback = stripped.trim().to_lowercase();
        let fallback = if fallback.is_empty() {
            name.trim().to_lowercase()
        } else {
            fallback
        };
        return if fallback.is_empty() {
            "unnamed".to_string()
        } else {
            fallback
        };
    }
    if joined.len() > MAX_STEM_BYTES {
        // Truncate on a char boundary at or below the cap.
        let mut cut = MAX_STEM_BYTES;
        while !joined.is_char_boundary(cut) {
            cut -= 1;
        }
        return joined[..cut].to_string();
    }
    joined
}

/// Convenience: cleans a path's file name directly.
pub fn clean_stem_of(path: &Path) -> String {
    clean_stem(&path.to_string_lossy())
}

// ---------------------------------------------------------------------
// Region / disc tag parsing (PR 3): pure functions, no engine, no I/O.
// The spike's tag vocabulary — regex-grade determinism for names that
// carry tags; the embedding tier exists for the ones that don't.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Region {
    Usa,
    Europe,
    Japan,
    World,
}

impl Region {
    /// The preference-ordered form used by smart-sort region priority
    /// (resolves the "region list fixed to USA/EUR/JPN" item for the
    /// smart path; the DAT path keeps its exact-checkbox semantics).
    pub fn as_str(self) -> &'static str {
        match self {
            Region::Usa => "USA",
            Region::Europe => "EUR",
            Region::Japan => "JPN",
            Region::World => "World",
        }
    }
}

/// Tag vocabulary → region. Keys are lowercase; multi-word keys match
/// whole tag groups ("(North America)").
const REGION_TAGS: &[(&str, Region)] = &[
    ("usa", Region::Usa),
    ("us", Region::Usa),
    ("u", Region::Usa),
    ("ntsc", Region::Usa),
    ("ntsc-u", Region::Usa),
    ("north america", Region::Usa),
    ("europe", Region::Europe),
    ("eur", Region::Europe),
    ("eu", Region::Europe),
    ("e", Region::Europe),
    ("euro", Region::Europe),
    ("pal", Region::Europe),
    ("japan", Region::Japan),
    ("jpn", Region::Japan),
    ("jp", Region::Japan),
    ("j", Region::Japan),
    ("ntsc-j", Region::Japan),
    ("world", Region::World),
    ("w", Region::World),
];

/// Parses the release region from a raw file name. Tag groups are
/// checked first in reading order, then bare separator-delimited tokens
/// (for messes with no brackets at all).
pub fn parse_region(name: &str) -> Option<Region> {
    // Pass 1: whole tag groups, in reading order.
    let mut depth = 0usize;
    let mut group = String::new();
    let mut groups: Vec<String> = Vec::new();
    for c in name.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                if depth == 1 {
                    group.clear();
                }
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    groups.push(group.trim().to_lowercase());
                }
            }
            _ if depth > 0 => group.push(c),
            _ => {}
        }
    }
    for group in &groups {
        if let Some(region) = REGION_TAGS.iter().find(|(tag, _)| *tag == group) {
            return Some(region.1);
        }
    }
    // Pass 2: bare tokens (lowercased, separator-split).
    let lowered = name
        .replace(['_', '.', '-', '(', ')', '[', ']', '{', '}'], " ")
        .to_lowercase();
    for token in lowered.split_whitespace() {
        if let Some(region) = REGION_TAGS.iter().find(|(tag, _)| *tag == token) {
            return Some(region.1);
        }
    }
    None
}

/// A parsed disc tag: number within the set, and the "of N" total when
/// present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscTag {
    pub number: u8,
    pub total: Option<u8>,
}

/// Parses the first disc/disk/cd tag from a raw file name. Word-boundary
/// + digit-anchored, so "Discworld", "CD-i", or "Track 1" never match.
pub fn parse_disc(name: &str) -> Option<DiscTag> {
    let lowered = name.to_lowercase();
    let bytes = lowered.as_bytes();
    for word in ["disc", "disk", "cd"] {
        let mut from = 0usize;
        while let Some(found) = lowered[from..].find(word) {
            let start = from + found;
            from = start + word.len();
            // Must start at a word boundary.
            if start > 0 && is_word_byte(bytes[start - 1]) {
                continue;
            }
            let mut rest = &lowered[start + word.len()..];
            // Optional separators between word and number.
            rest = rest.trim_start_matches(['_', '-', ' ', '.']);
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() || digits.len() > 2 {
                continue;
            }
            let number: u8 = digits.parse().ok()?;
            if !(1..=99).contains(&number) {
                continue;
            }
            // Must end at a word boundary (digit followed by non-word or end).
            let after = &rest[digits.len()..];
            if after.starts_with(|c: char| c.is_ascii_alphanumeric()) {
                continue;
            }
            // Optional "of N" total.
            let mut total = None;
            let trimmed = after.trim_start_matches(['_', '-', ' ', '.', ')', ']', '}']);
            if let Some(of_rest) = trimmed.strip_prefix("of ") {
                let total_digits: String =
                    of_rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
                if !total_digits.is_empty() && total_digits.len() <= 2 {
                    if let Ok(value) = total_digits.parse::<u8>() {
                        if (1..=99).contains(&value) {
                            total = Some(value);
                        }
                    }
                }
            }
            return Some(DiscTag { number, total });
        }
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

/// The `.gdi` rule from the spike: a GD-ROM descriptor can only be a
/// Dreamcast image, so the extension is a deterministic system hint.
/// No shipped profile has a dreamcast system yet; the rule ports anyway.
pub fn gdi_system_hint(file_name: &str) -> Option<&'static str> {
    let lowered = file_name.to_lowercase();
    lowered.ends_with(".gdi").then_some("dreamcast")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_junk_and_separators() {
        assert_eq!(clean_stem("Final Fantasy VII (USA) (Disc 2).cue"), "final fantasy vii");
        assert_eq!(clean_stem("Sonic CD [J]"), "sonic cd");
        assert_eq!(clean_stem("Metal Gear Solid (USA) (Disc 1)"), "metal gear solid");
        assert_eq!(clean_stem("gran_turismo_scene_rip_psx_track1.bin"), "gran turismo scene psx");
        assert_eq!(clean_stem("Panzer Dragoon Saga (USA) Disc 3 of 4.cue"), "panzer dragoon saga of");
        assert_eq!(clean_stem("Nights.into Dreams (Europe).bin"), "nights into dreams");
        assert_eq!(clean_stem("Game (Rev A) [!] (v1.1)"), "game");
        assert_eq!(clean_stem("R-Type Delta (USA)"), "r type delta");
    }

    #[test]
    fn folder_prefixes_are_dropped() {
        assert_eq!(clean_stem("roms/gba/Advance Wars (U).gba"), "advance wars");
        assert_eq!(clean_stem("C:\\library\\nes\\Zelda.zip"), "zelda");
    }

    #[test]
    fn keeps_identity_when_everything_strips() {
        // Never return an empty stem: fall back to the tag-stripped name.
        assert_eq!(clean_stem("(USA) (Disc 1) [!]"), "(usa) (disc 1) [!]");
        assert!(!clean_stem("(1)").is_empty());
        assert!(!clean_stem("...").is_empty());
    }

    #[test]
    fn caps_output_length() {
        let long = "x".repeat(MAX_STEM_BYTES + 100);
        assert!(clean_stem(&long).len() <= MAX_STEM_BYTES);
    }
}

#[cfg(test)]
mod tag_parsing_tests {
    use super::*;

    #[test]
    fn region_vocabulary_round_paren_square_and_bare() {
        // The spike generator's full tag vocabulary.
        for (name, expected) in [
            ("Game (USA).cue", Region::Usa),
            ("Game [U].gba", Region::Usa),
            ("Game US.zip", Region::Usa),
            ("Game (NTSC).iso", Region::Usa),
            ("Game (NTSC-U).cue", Region::Usa),
            ("Game (North America).cue", Region::Usa),
            ("Game (Europe).cue", Region::Europe),
            ("Game [E].gba", Region::Europe),
            ("Game (EU).zip", Region::Europe),
            ("Game (PAL).cue", Region::Europe),
            ("Game Euro.bin", Region::Europe),
            ("Game (Japan).cue", Region::Japan),
            ("Game [J].gba", Region::Japan),
            ("Game (JPN).zip", Region::Japan),
            ("Game (NTSC-J).cue", Region::Japan),
            ("Game Japan.nes", Region::Japan),
            ("Game (World).cue", Region::World),
            ("Game [W].gba", Region::World),
        ] {
            assert_eq!(parse_region(name), Some(expected), "case: {name}");
        }
    }

    #[test]
    fn region_reads_first_tag_then_bare_token() {
        assert_eq!(parse_region("Game (USA) (Japan).cue"), Some(Region::Usa));
        assert_eq!(parse_region("game_japan_v1.bin"), Some(Region::Japan));
    }

    #[test]
    fn region_negatives_do_not_match_substrings() {
        assert_eq!(parse_region("Disgaea (En,Fr,De).cue"), None, "language tag is not a region");
        assert_eq!(parse_region("Europa Universalis.cue"), None, "title word must not match");
        assert_eq!(parse_region("Metal Gear Solid.bin"), None);
    }

    #[test]
    fn disc_tags_parse_with_variants_and_totals() {
        for (name, number, total) in [
            ("Game (Disc 1).cue", 1, None),
            ("Game Disc 2.cue", 2, None),
            ("Game (Disk 1).bin", 1, None),
            ("game_disc3.img", 3, None),
            ("Game (Disc 03).cue", 3, None),
            ("Game (Disc 3 of 4).cue", 3, Some(4)),
            ("Game disc_2.7z", 2, None),
            ("Game (CD 2).cue", 2, None),
        ] {
            let parsed = parse_disc(name).unwrap_or_else(|| panic!("case: {name}"));
            assert_eq!(parsed.number, number, "case: {name}");
            assert_eq!(parsed.total, total, "case: {name}");
        }
        assert_eq!(parse_disc("Metal Gear Solid.bin"), None);
        assert_eq!(parse_disc("Game (Disc 0).cue"), None, "zero is not a disc number");
    }

    #[test]
    fn disc_words_do_not_match_inside_words_or_cd_i() {
        assert_eq!(parse_disc("Discworld (USA).cue"), None, "no digits after disc");
        assert_eq!(parse_disc("The Misadventures of Pip (CD-i).cue"), None, "CD-i is not a disc tag");
        assert_eq!(parse_disc("Track 1 audio.bin"), None, "track is not disc");
        assert_eq!(parse_disc("Discmania (USA).cue"), None);
    }

    #[test]
    fn gdi_hint_is_dreamcast_only() {
        assert_eq!(gdi_system_hint("Shenmue (USA).gdi"), Some("dreamcast"));
        assert_eq!(gdi_system_hint("GAME.GDI"), Some("dreamcast"));
        assert_eq!(gdi_system_hint("Game.cue"), None);
        assert_eq!(gdi_system_hint("gdi-batch-readme.txt"), None, "must end with the extension");
    }
}
