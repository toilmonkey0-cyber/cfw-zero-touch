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
