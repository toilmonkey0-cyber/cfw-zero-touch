# Igir sorting backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optional igir-powered 1G1R + DAT-verified staging of the library before the existing card copy; inert without a DAT folder.

**Architecture:** igir runs once per included system into a local staging folder under the store root; Preview/Copy then treat staging as the library, so every card-side gate and behavior is unchanged. Profile JSON carries an optional per-system `datNamePattern` used as igir's `--dat-name-regex`; pattern-less systems stage by plain copy.

**Tech Stack:** Rust (Tauri 2, serde), igir v5.5 via PATH or `npx --yes igir@latest`, React + TypeScript.

**Spec:** `docs/superpowers/specs/2026-09-25-igir-sorting-design.md`

## Global Constraints

- igir writes only into staging; `clean` is never used; no new crates.
- `CFW_IGIR_TEST=1` gates the only test that runs igir.
- DATs are user-supplied; the app never downloads them.
- Commit style: imperative one-liners matching `git log`.

## Review Focus

1. A system without a `datNamePattern` must still reach the card (raw copy), or the summary must say it was skipped — silence is a data-loss surprise. Test: builder unit test (Task 2).
2. Region list must join into igir's comma form and 1G1R must appear only when a DAT is set. Test: builder unit test (Task 2).
3. Staging must be wiped before each run so a previous library's files cannot leak into a new plan. Test: staging wipe step (Task 4, fixture).
4. Profile pattern changes must ride the feed with a version bump, or seeded stores never see them. Test: drift test with explicit versions (Task 1).
5. Old feeds/profiles without `datNamePattern` must still validate. Test: existing suites stay green (Task 1).

---

### Task 1: `datNamePattern` + profile patterns + feed v2

**Files:** Modify `specs/profile.schema.json`, `src-tauri/src/profiles.rs`, `src-tauri/tests/profiles.rs`, all six `profiles/*.json`, regenerate `profiles-feed.json`.

- [ ] Test first (`src-tauri/tests/profiles.rs`): a profile with `"datNamePattern":"^Nintendo - Game Boy$"` on a system parses and roundtrips through serde with the field intact; a system without it stays `None`. RED (no field), then add to `SystemFolder`:

```rust
#[serde(rename = "datNamePattern", default, skip_serializing_if = "Option::is_none")]
pub dat_name_pattern: Option<String>,
```

- [ ] Schema: add `"datNamePattern": { "type": "string" }` to system `properties`.
- [ ] Add patterns to all six profiles' systems per the spec table; add `"version": 2` to each profile file; regenerate the feed with the Task-7-style node one-liner (it carries `p.version ?? 1`).
- [ ] Drift test passes unchanged (shipped files now parse at explicit version 2, equal to feed).
- [ ] Run `cargo test --test profiles --test feed`, then commit: `Add DAT name patterns and bump profiles to version 2.`

### Task 2: igir invocation builders (pure)

**Files:** Create `src-tauri/src/igir.rs` (+ `pub mod igir;`), test `src-tauri/tests/igir.rs`.

- [ ] Types and function:

```rust
pub struct StagePlan { pub system_folder: String, pub args: Vec<String> }

pub fn stage_plans(
    library: &Path, staging: &Path, systems: &[(String /*folder*/, Option<String> /*pattern*/)],
    dat_folder: Option<&Path>, regions: &[String], single: bool,
) -> Vec<StagePlan>
```

per system: `-i <library>/<folder>/**`, and with DAT: `-d <dat>/** --dat-name-regex <pattern> -R <joined> [-s]`; always `-o <staging>/<folder>/ --overwrite-invalid`; pattern-less systems with a DAT get the same invocation minus the dat flags only if the pattern is None AND dat is None → plain copy; pattern-less with DAT present → plain copy (spec: raw).

Wait — spec says pattern-less systems stage by plain copy always. Implement exactly that: DAT flags only when `dat_folder.is_some() && pattern.is_some()`. Regions/single only with DAT. Output dir always `<staging>/<folder>/`.

- [ ] Tests: with DAT + pattern → dat flags, `-R USA,EUR`, `-s`; without DAT → no dat flags even with regions set; pattern-less → plain copy; empty regions with DAT → omit `-R`. RED first.
- [ ] Commit: `Build igir staging invocations per system.`

### Task 3: `stage_library` command

**Files:** Modify `src-tauri/src/lib.rs`, `src-tauri/tests/igir.rs`.

- [ ] `resolve_igir() -> Result<String, String>`: `igir` on PATH (`where.exe igir` success) else check `npx` (where.exe npx) → return the base command tokens (`["igir"]` or `["cmd","/C","npx","--yes","igir@latest"]`); neither → Err with guidance.
- [ ] `stage_library(app, profile_id, library, dat_folder, regions, single, include) -> StageReportView`:
  - profile_by_id; included systems = profile systems filtered by `include` (empty = all); staging dir = `store::store_root()/staging`; wipe it (`fs::remove_dir_all` best-effort) then create.
  - For each StagePlan: spawn (tokio-free `std::process::Command`), stream lines, emit `stage-progress` `{systemFolder, line}` events, collect tail; non-zero exit → Err with tail.
  - Report: `{ stagedSystems: [folder...], rawCopiedSystems: [folder...], stagedFiles: usize }` (walk staging counting files).
- [ ] Register command. Commit: `Stage the library with igir before preview.`

### Task 4: Frontend — sort section + staged flow

**Files:** Modify `src/App.tsx`.

- [ ] State: `datFolder`, `regions: string[]` (default `["USA"]`), `single: boolean` (default true), `stageSummary`, `staging` flag.
- [ ] In the library step, a "Sort with a DAT (optional)" block: DAT folder picker (existing `open` dialog), path display, three region checkboxes, 1G1R checkbox, explainer line ("DATs come from you — No-Intro's datomatic. Without one, Preview copies as today.").
- [ ] Preview with `datFolder` set: call `stage_library` first (progress line while staging), then `plan_roms` with `library = staging path from the report`? — the backend returns the staging path in the report (`stagingPath: String`); store it and use it for both plan and copy. Without `datFolder`: exactly as today.
- [ ] Commit: `Add the optional DAT sort section to the library step.`

### Task 5: Env-gated integration test

**Files:** Modify `src-tauri/tests/igir.rs`.

- [ ] `CFW_IGIR_TEST=1` test: fixture library with `game (USA).nes`, `game (Europe).nes` (identical bytes), `other (USA).nes`; run igir `dir2dat` over the folder to make a DAT; call the same code path as `stage_library`'s spawn loop (extract a testable `run_stage_plan` helper) with `-R USA -s`; assert staging contains `game (USA).nes` + `other (USA).nes` and no Europe file.
- [ ] Run with the env var set on this machine; commit: `Verify 1G1R staging against a self-generated DAT.`

### Task 6: Live verification + docs

- [ ] Drive the dev app: feed check shows the version-2 profile update → apply → "Updated 6 profiles"; library step on the real library bridge: set no DAT → preview unchanged; set DAT folder (generated fixture) → stage → plan shows the 1G1R-reduced set for the fixture system.
- [ ] BUILD_PLAN Phase 4 igir line done; README note that DATs are user-supplied. Commit.
