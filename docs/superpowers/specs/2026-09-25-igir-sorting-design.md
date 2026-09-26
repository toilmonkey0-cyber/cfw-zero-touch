# Igir sorting backend — design

Date: 2026-09-25
Status: approved in chat ("do what makes sense"); spec on disk for review
Phase: BUILD_PLAN Phase 4 (second sub-project)

## Context

Card Studio's copier moves a library onto a card folder-for-folder, skip-unchanged,
with preview. It has no notion of game identity: full No-Intro sets copy every
region duplicate. igir (v5.5, npm, also on PATH as a single binary) brings game
identity via DAT files: 1G1R selection (`-s`, requires DATs with parent/clone
info), region filtering (`-R USA,...`), checksum verification, and canonical
renaming. The user supplies DATs; the app never downloads games, BIOS, or DATs.

## Goals

- Optional DAT-driven sort before the existing card copy: 1G1R + verification
  when a DAT folder is supplied; zero behavior change when it is not.
- igir never writes to the card and never runs `clean` against card content.
- Preview, skip-unchanged, the armed-firstboot gate, and crash-safe copy are
  unchanged; they operate on the staged set.

## Non-goals

- igir writing directly to the card; any use of `igir clean`.
- Downloading DATs (user supplies a folder, e.g. from No-Intro's datomatic).
- Arcade/FBNeo DAT matching (no shipped pattern; arcade copies as-is).
- CHD/disc handling changes (Optimistic Euclid's domain).
- Cancelling a running stage (v1).

## Approach: staging pre-pass (approved)

1. The library step gains an optional "Sort with a DAT" section: DAT folder
   picker, region checkboxes (USA, EUR, JPN; default USA), and a 1G1R checkbox
   (default on). The section is inert until a DAT folder is chosen.
2. Choosing Preview with a DAT folder set first **stages** the library:
   - For each included system whose folder carries a `datNamePattern`, run
     `igir copy -i "<library>/<folder>/**" -d "<datFolder>/**"
     --dat-name-regex "<pattern>" -R <regions> [-s] -o "<staging>/<folder>/"
     --overwrite-invalid`.
   - Systems without a pattern (arcade, bios) stage by plain copy
     (`igir copy -i ... -o ...`) so everything included still reaches the card.
   - The staging root is `%LOCALAPPDATA%\cfw-card-studio\staging`
     (`CFW_STUDIO_DATA` override via the existing store root). Each staging run
     wipes and rebuilds it; it persists after copy so a re-copy needs no re-stage.
3. Preview and Copy then run against the staging folder exactly as they run
   against a library today (plan_roms/copy_roms take the library path).
4. Without a DAT folder, Preview/Copy behave exactly as today (no igir, no
   staging, no disk doubling).

## igir invocation

- Tool resolution: `igir` on PATH first; otherwise `npx --yes igir@latest`
  spawned via `cmd /C` on Windows. Neither available: a clear error naming the
  two options (install igir, or install Node.js).
- One invocation per system; stdout/stderr lines are emitted as
  `stage-progress` events (raw last line) so the wizard shows movement.
- A non-zero exit fails staging with the captured output tail.
- DAT name patterns are anchored regexes against No-Intro DAT names, one per
  system in the profile (`datNamePattern`), so Game Boy, Color, and Advance do
  not cross-match. Shipped patterns: gba `^Nintendo - Game Boy Advance`,
  gb `^Nintendo - Game Boy$`, gbc `^Nintendo - Game Boy Color`,
  nes `^Nintendo - Entertainment System`, snes `^Nintendo - Super Nintendo`,
  megadrive `^Sega - Mega Drive`, mastersystem `^Sega - Master System`,
  gamegear `^Sega - Game Gear`, pcengine `^NEC - PC Engine`,
  neogeo `^SNK - Neo Geo`, n64 `^Nintendo - Nintendo 64`,
  psx `^Sony - PlayStation`. Systems without a pattern copy raw.

## Profile and feed changes

- `SystemFolder` gains optional `datNamePattern`; the schema documents it.
  Old profiles and old feeds validate unchanged (optional field).
- All six shipped profiles gain patterns for their known systems and bump to
  `version: 2` in `profiles/*.json`; `profiles-feed.json` is regenerated. The
  user's seeded store (version 1) then sees a real update through the feed —
  the first dogfood of the update path.

## Testing

- Offline unit tests: `datNamePattern` serde roundtrip; staged-argument
  construction (with/without DAT, region joining, 1G1R flag, per-system raw
  copy for pattern-less systems); feed regeneration drift test.
- Env-gated integration test (`CFW_IGIR_TEST=1`, this machine has Node):
  build a fixture library containing a USA/EUR duplicate pair, generate a DAT
  with igir's own `dir2dat`, stage with 1G1R + USA, and assert only the USA
  file lands in staging.
- Live verification: drive the wizard with the real library bridge on the
  smallest system, then apply the version-2 profile feed through the UI.

## Safety review

- igir writes only into the staging directory (`-o`); `clean` is never used.
- The card-side gates (volume, firstboot, FORMAT confirmation) are untouched.
- Pattern regexes come from profiles (repo/feed trust, same as image URLs).
- Staging is user-local data under the store root; wiped on each stage run.
