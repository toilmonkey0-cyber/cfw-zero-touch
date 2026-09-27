# Needle local-AI integration — design

Date: 2026-09-26
Status: feature set brainstormed with the user this session; spec drafted for
review (Phase 1 scoped for implementation, later phases directional)
Phase: proposed BUILD_PLAN Phase 5 (all Phase 4 items are closed)

## Context

Card Studio's deterministic pipeline is finished and safe: profile feed,
volume/format gates, armed-firstboot gate, crash-safe copy, igir DAT
sorting, diagnostics log. Two gaps remain that determinism alone cannot
close well:

- **DAT-less sorting is intentionally inert.** The "Sort with a DAT"
  section (spec `2026-09-25-igir-sorting-design.md`) does nothing without a
  user-supplied DAT folder: `romcopy::plan_copy` routes files only by exact
  library-folder-name match (`find_named_dir` matches `system.folder` or
  `system.id`, case-insensitively, at the library root). A messy library —
  mixed folders, `.zip` files that could belong to three systems, renamed
  region variants — copies as-is or not at all, and every region duplicate
  lands on the card.
- **Gate refusals are terse.** "firstboot is still armed (expandtoexfat.sh,
  firstboot.sh)" is precise and correct, but a first-time user does not know
  what to *do* about it.

A local model was validated this session specifically for these jobs. The
Needle 3 spike (sibling Rom Ingestion Engine repo,
`docs/needle-spike-2026-09-26.md`, tooling in `tools/needle/`) proved an
on-device stack: engine under 1.5 MB per platform, base weights 35 MB,
~81 MB peak RAM, p50 ~170 ms/file over local HTTP, ~700 tok/s decode on a
laptop CPU, inference never touching the network. The spike's central
finding is an architecture, not just a model: **split generation from
lookup** — deterministic regex for tags (100% on tagged names), embedding
nearest-neighbor for title identity (full-index k=1 = 92.5% platform; with
cosine abstain threshold 0.985 = 97.1% accuracy at 86.2% coverage, the rest
needing review), and generation (tuned weights) for junk refusal — while
generation can never learn title→platform knowledge (36% on unseen titles;
a 1.8k-example LoRA learns format, not a per-title lookup table).

The Rom Ingestion Engine already carries a proven Rust integration
(`src-tauri/src/classifier/needle.rs`: reset-then-complete client, fast
timeouts, `NoCall` soft-fallthrough, vocabulary-validated parsing) and a
proven managed-download pattern (`src-tauri/src/chdman/downloader.rs`:
per-platform URL + SHA-256 manifest). Per the user's directive, Card Studio
gets its **own integration** — the two apps never share a process, port, or
code path — but reuses the spike's artifacts: the `tools/needle/` pipeline
(corpus generator, eval harness, embedding experiment, trained
`adapter.safetensors`), the weight pins, and the protocol lessons.

## Goals

- Fill the DAT-less sorting gap: messy libraries sort per-profile
  (nes→`Roms/FC` on `r35s-stock-card`, etc.) with no DAT, no Node, no
  network at inference time.
- Explain cards and refusals in plain language (Card Doctor) without
  changing one byte of gate logic.
- Catch renamed duplicate variants before they burn SD space (embedding
  dedupe in preview).
- Every capability works as pure advisory on top of the existing
  deterministic moat; the app behaves identically when the engine is
  absent, not installed, or crashed.
- Distribution follows the project's existing trust model: SHA-256-pinned
  downloads over HTTPS, cached under the store root, telemetry disabled on
  anything spawned.
- Land the long-game differentiator (on-device sorting companion) as a
  measured, gated Phase 5 — not a promise.

## Non-goals

- Shipping, bundling, or downloading ROMs, BIOS, **or DATs** — including a
  prebuilt title index *derived from* DATs (see Key Decisions).
- Needle ever deciding, disposing, or writing: no model output reaches the
  card except through the unchanged `romcopy::execute_copy`, and no model
  string becomes a path component.
- Any cloud/BYOK inference, ever (filenames stay on the machine).
- Replacing `npx igir` for the DAT path; the two sort modes coexist.
- Changing the volume, format, firstboot, or flash gates; changing
  `ensure_safe_dest`, disk-0 refusal, FORMAT/FLASH confirmations, or UAC
  elevation.
- Shipping a Python runtime to users (the pip package is a developer tool
  for the training pipeline only).
- Feed-distributed engine pins in v1 (open question; v1 pins compile into
  the app).
- Stage/copy cancellation (still deferred; smart classification reports
  progress but a started classification runs to completion).
- Making smart sort work on fully ambiguous libraries: a library with no
  recognizable folders and only ambiguous extensions (all zips) has no
  index to query and degrades to review-by-hand or the DAT path — by
  design, surfaced honestly in the UI rather than guessed around.

## Key Decisions

1. **Needle is advisory-only, above the moat.** Gates and the copy
   executor stay Needle-free modules; the model can only *propose* routes,
   explanations, and duplicate groups. Rationale: the pre-publication
   security review's verdict rests on the deterministic stack; nothing in
   this design may weaken it, and every PR is reviewable against that line.
2. **Phase 1 uses embeddings only, not generation.** The spike proved tags
   are best parsed deterministically (regex 100% on tagged names) and
   identity is best solved by nearest-neighbor lookup; generation adds
   nothing to routing that the tiers below don't already do more safely.
   The serve sidecar (and the 63 MB tuned weights) waits for Phase 2, so
   Phase 1's new runtime surface is a single unelevated helper process
   speaking stdio — no HTTP, no port management.
3. **Embeddings run through a small helper binary (`cfw-embed`) that
   statically links the C API at build time** (`needle_embed` in
   `needle.h`, `libneedle.a`) and loads the *weights* at runtime — not
   FFI in the app process and not serve mode (which exposes no `/embed`
   route). A static archive can only be linked at compile time, so the
   engine code ships inside the installer-borne `cfw-embed.exe` (vendored
   from the pinned `.a` in CI; `CFW_NEEDLE_DIR` supplies it for local
   builds), and only the 35 MB weights are a runtime download. Rationale:
   a crash in native code cannot take the app down; the helper mirrors
   the existing `bin/cfw-flash.rs` second-binary precedent; it runs
   unelevated and never touches the network; and Phase 1 downloads zero
   executables — the only acquired artifact is data. The
   MSVC-linkability of the shipped `libneedle.a` is a build-time entry
   criterion PR 2a must validate (fallbacks in Risks/Open Questions).
4. **Weights download on first use; engine code ships in the installer.**
   The 35 MB `needle3.cact` enters the store root only after explicit
   consent, verified against a SHA-256 pin compiled into the app
   (`flash::ensure_image` pattern: `.partial`, hash check, rename;
   mismatch discards). The ~1.5 MB engine code is *not* a download: it
   is statically linked into `cfw-embed.exe`, which ships beside the app
   exactly like `cfw-flash.exe` (Phase 2's `needle.exe` serve binary is
   the one runtime-executable download, also pinned). Rationale: keeps
   the installer +~1.5 MB instead of +36 MB, keeps the no-engine path
   the default, and the only thing a compromised network can swap is
   pinned data — stricter than the existing igir path, which today
   executes `npx --yes igir@latest` (unpinned).
5. **Engine and weights are software, not content — argued explicitly.**
   The engine is a program like 7-Zip (already an external dependency at
   `C:\Program Files\7-Zip\7z.exe`), and `needle3.cact` is model
   parameters, the same trust class as the SHA-256-pinned OS images
   already downloaded from GitHub releases. Neither is a ROM, a BIOS, or
   a DAT (game content or a game catalog). The Needle code repo is
   Apache-2.0; the HF weights repo's license tag must be confirmed to
   match before go-live (Open Questions). The rule's edge case — a
   *shipped* title index distilled from DATs — is excluded: Phase 1
   indexes are built only from the user's own filenames, on their machine.
6. **The library index is built from the user's library at runtime**, not
   shipped: every canonical stem already sitting in a recognized
   system folder becomes a labeled index entry; ambiguous files are
   queries against it. Rationale: sidesteps the DAT-derivative question
   entirely and makes the index self-consistent with what the user
   actually owns.
7. **Card Studio owns its integration; the ingestion repo owns the
   pipeline.** We mirror the proven client/parser defenses (reset-then-
   complete, vocabulary validation, `NoCall` fallthrough) and reuse
   `tools/needle/` artifacts (tools.json schema conventions, corpus, eval
   harness, adapter), but no shared process, port, crate, or release
   coupling. User directive; also the only shape that survives either app
   being absent.
8. **Spawn hygiene matching the security review:** direct binaries only
   (no `cmd /C`, no shell strings), `NEEDLE_TELEMETRY=0` +
   `DO_NOT_TRACK=1` on every spawn, localhost only, health-check before
   the tier enables, idle shutdown, kill-on-exit.
9. **Feature-detect, don't require.** `needle_status()` reports
   absent/installing/ready/failed; smart sections render inert with a
   one-click install button; every existing flow is byte-identical when
   the status is anything but ready.
10. **Smart sorting plans directly from the library, no staging copy.**
    igir stages because it is an external tool that must write somewhere;
    needle routing happens in-process, so plans reference library sources
    directly — no disk doubling (a property the igir spec itself brags
    about for the no-DAT path).
11. **Model output is enum-validated, never free-form paths.** A route is
    a `system_id` that must exist in the loaded profile; the destination
    is then computed by the existing `folder_map::storage_folder` +
    `profiles::is_safe_folder` chain. A hostile or buggy model can pick a
    wrong *system*, never a wrong *path shape*.

## Approach

### Architecture: advisory tier above the deterministic moat

```
┌─ React/TS wizard (src/App.tsx) ── status chips, review UI, plan view ─┐
│  CSP: default-src 'self'; connect-src ipc: http://ipc.localhost …    │
│  (renderer never talks HTTP to any engine; Rust owns all of it)      │
├─ Tauri commands (lib.rs) ── the only new IPC surface ────────────────┤
│  needle_status · needle_acquire · classify_library                    │
│  plan_roms/copy_roms (+ optional smart payload) · diagnose_card (P2)  │
├─ needle/ module (new, pure — no tauri::Emitter) ─────────────────────┤
│  manifest.rs acquire.rs embed_client.rs index.rs sort.rs serve.rs(P2) │
│  client.rs (P2) — Card Studio's own, mirroring ingestion needle.rs   │
├─ bin/cfw-embed.rs (new helper) — C API host, stdio JSONL, unelevated  │
├───────────────────────── THE MOAT (unchanged) ───────────────────────┤
│  volume.rs decide/allow_copy · format_gate.rs authorize_format ·     │
│  prepare.rs · firstboot.rs card_safety · romcopy.rs execute_copy +   │
│  ensure_safe_dest · flash.rs disk-0/FLASH/UAC · feed.rs https-only   │
└───────────────────────────────────────────────────────────────────────┘
```

The moat list is exactly today's code. Needle modules import nothing from
it except `Profile`/`SystemFolder` types and `folder_map`; nothing in the
moat imports anything from `needle/`. A PR that adds a `needle::` use to
`volume.rs`, `format_gate.rs`, `firstboot.rs`, `flash.rs`, or
`romcopy.rs` is wrong by construction.

### What Needle is (validated facts this design builds on)

From the spike record (`docs/needle-spike-2026-09-26.md`,
`tools/needle/README.md` in the ingestion repo; engine + weights fetched
from HF repo `Cactus-Compute/needle3`, paths verified on Windows:
`windows-x86_64/{needle.exe, libneedle.a, needle.h}` + `needle3.cact`):

- Serve mode: `needle.exe --model X.cact --tools tools.json --serve`
  exposes `POST /reset` and `POST /complete
  {"input":"..."}` **only** — there is no `/embed` HTTP route. The engine
  owns one process-global conversation: every independent request needs
  reset-then-complete, or answers leak across inputs. (A `--port` flag is
  implied by the CLI but was never exercised in the spike, which used the
  default :8080 — verify on the first Phase 2 smoke test.)
- Input must be bare data text (folder/filename); verbs leak into grounded
  fields (the word "Classify" set `is_multidisc: true`). Instructions live
  in the tool description.
- `function_calls` hold grounded answers; `suppressed_calls` hold withheld
  ones (arguments present, usable at reduced confidence); `confidence` is
  `null` on locally LoRA-tuned weights (calibration head ships only with
  base/platform builds) — the parser defaults it to 0.75 and keeps
  deterministic gates deciding.
- Embeddings: 3072-dim, ~5 ms/title, base weights, via the Python API or
  the C API `needle_embed`. k=1 nearest-neighbor beats k=5 voting (voting
  overrode correct nearest matches). Stems must be cleaned before
  embedding or accuracy collapses. Index math is trivial in Rust
  (10k × 3072 f32 ≈ 123 MB, ~ms per query).
- Generation is good at tag parsing on canonical names but cannot learn
  title→platform knowledge (36% unseen) — never proposed for identity
  lookup.
- Tooling pin: `cactus-needle==3.0.1` (3.0.5's engine-wheel download
  404s). This pin binds *developers rebuilding weights*, not users — the
  app downloads engine/weights over HTTPS directly with its own pins.
- Training (only if we ever re-tune): WSL required (Windows MAX_PATH kills
  flax/orbax); `XLA_PYTHON_CLIENT_PREALLOCATE=false`,
  `--max-len 512 --batch-size 8 --workers 2`; ~3.5 h CPU for 1.8k
  examples. The trained `adapter.safetensors` is committed in
  `tools/needle/` and rebuilds `romclass.cact` in ~2 min.

### Engine distribution, trust, and licensing

- **Manifest** (`needle/manifest.rs`): a compiled-in table, one row per
  artifact — `needle3.cact` (runtime download, Phase 1), `needle.exe`
  (runtime download, Phase 2), and `libneedle.a` + `needle.h`
  (**build-time inputs only**, vendored by CI / `CFW_NEEDLE_DIR` when
  compiling `cfw-embed`; never acquired on a user machine). Each row:
  URL (huggingface.co resolve URLs), expected SHA-256, expected byte
  size. Same shape as the ingestion
  `chdman/downloader.rs::PlatformManifest` and the same guarantee class
  as profile image pins.
- **Acquire** (`needle/acquire.rs`): cache search order
  `CFW_NEEDLE_DIR` → `store_root()/needle/` (mirrors the
  `CFW_IMAGE_DIR` → `%LOCALAPPDATA%\cfw-card-studio\images` order in
  `lib.rs::image_cache_dirs`). Download to `<name>.partial`, verify
  SHA-256 (reuse `flash::sha256_reader`), discard on mismatch, rename on
  match — exactly `flash::ensure_image`'s contract. Fetch goes through an
  injectable closure so unit tests stay offline (the seam
  `flash::ensure_image` already uses, and the same idea as
  `prepare::Shell`/`ShellLog` for elevation).
- **Consent**: nothing downloads until the user clicks the install button
  in the smart-sort section. Size is shown up front (~35 MB for Phase 1 —
  weights only; the engine code already ships inside `cfw-embed.exe`).
  Progress emits through a `needle-progress` event from `lib.rs`
  (Emitter-touching code stays in `lib.rs` per the Windows test-binary
  caveat).
- **Download-or-bundle tradeoff**: bundling the *weights* into the
  installer was rejected for v1 — 35 MB for a feature that must stay
  optional, coupled to every app release. The *engine* (~1.5 MB) is
  bundled instead, statically linked inside `cfw-embed.exe`, because an
  archive cannot be linked after install and because code shipped in the
  installer is the stronger integrity story (Phase 1 then downloads no
  executables at all). Weight downloads keep the trust story identical
  to OS images and let the cache be deleted by the user. Revisit if
  field data shows install friction (open question).
- **Trust argument (explicit)**: the profile feed already trusts
  `raw.githubusercontent.com/.../main` (HTTPS origin + the `main`
  branch; branch protection is planned as part of go-live, not yet
  enabled) to deliver URLs that the app then downloads and
  hash-verifies. Needle artifacts get a *stronger* anchor: their pins
  compile into the app binary, so a compromised feed cannot swap engine
  or weight bytes. In Phase 1 the only download is `needle3.cact` —
  data, never executed. Phase 2's `needle.exe` is executable code — but
  so is the 7-Zip the app already shells out to, and unlike today's
  `npx --yes igir@latest` (which fetches and runs unpinned code at
  runtime), it is byte-pinned. Inference never touches the network;
  every spawn sets `NEEDLE_TELEMETRY=0` and `DO_NOT_TRACK=1`.
- **Licensing**: Needle is Apache-2.0 (github.com/cactus-compute/needle).
  The first PR that adds the manifest lands `THIRD_PARTY_NOTICES.md`
  (Apache-2.0 attribution for the engine and header; the weights line
  is drafted pending Open Question 9, and confirming that license tag is
  on PR 1's checklist since PR 1 lands the file) plus a README
  licensing note — required before the repo goes public.

### Sidecar and helper lifecycle

Two processes, phased in — never both needed for one feature:

- **`cfw-embed.exe` (Phase 1)**: built from `src-tauri/src/bin/
  cfw-embed.rs` in this repo, shipped beside the app like `cfw-flash.exe`.
  It statically links the engine at build time (`build.rs` vendors the
  pinned `libneedle.a` + `needle.h` from `CFW_NEEDLE_DIR`; CI fetches
  them from the pinned URLs, and a local build without them fails with
  instructions rather than silently degrading) and loads the *acquired*
  `needle3.cact` weights at runtime. Protocol:
  long-running stdio JSONL — app sends `{"op":"ping"}` /
  `{"op":"embed","stem":"..."}`, helper answers
  `{"stem":"...","vec":[...]}`; batch op `embed_many` for index builds.
  Spawned unelevated, direct binary (no shell), telemetry env vars set,
  stdout/stderr piped. Determinism: `embed` is a pure function of (text,
  weights) — no sampling — which is what makes copy-time re-classification
  safe (below).
- **`needle.exe --serve` (Phase 2+)**: spawned from
  `needle/serve.rs` when Card Doctor (or a later NL surface) first needs
  it. Port picked by binding `127.0.0.1:0` in Rust to find a free port,
  then passed via `--port` (flag to-verify on first smoke test; the
  close-then-spawn race is accepted with a retry on bind failure —
  harmless on localhost); health = one `POST /reset` round-trip within a
  2 s connect / 20 s request timeout; failure ⇒ feature disabled, app
  unchanged. The client is **synchronous ureq on a dedicated worker
  thread** — Card Studio has no tokio, so PR 6 ports the ingestion
  client's timeouts, retries, and parse defenses, not its async
  runtime.
- **Shared lifecycle policy**: lazy spawn on first need; last-use
  timestamp; idle > 5 min ⇒ kill (RAM goes back); respawn on next
  request; `RunEvent::Exit` handler in `lib.rs::run()` kills children so
  no orphan survives the app. Peak cost: ~81 MB RSS per engine process;
  the two processes are never both resident in Phase 1.

### Data flow: what text reaches the model

- **Filenames and folder names only** — cleaned stems for embeddings;
  folder-prefixed bare filenames for serve-mode tools (the ingestion
  contract: bare data text, instructions in the tool description).
- **Card summaries only** (Phase 2): volume labels, file systems, sizes,
  partition layout, top-level folder names, which firstboot marker files
  exist. No file contents. Ever. Not ROM bytes, not config bytes, not log
  bodies (Phase 4 diagnostics triage takes a pasted log by explicit user
  action and processes it locally).
- Nothing leaves the machine: inference is in-process or localhost; the
  only network in the whole design is the one-time pinned download.
- The diagnostics log keeps its existing rule — no ROM file names in
  `diag::log` — so needle events carry counts and statuses only.

### Phase 1 — DAT-less smart sorting (implementable detail)

**New modules**

```
src-tauri/src/needle/mod.rs          feature status, wiring
src-tauri/src/needle/manifest.rs     pinned artifacts table
src-tauri/src/needle/acquire.rs      verified download/cache
src-tauri/src/needle/embed_client.rs spawn cfw-embed, stdio JSONL
src-tauri/src/needle/index.rs        LibraryIndex build/persist/query
src-tauri/src/needle/sort.rs         routing tiers + variant collapse
src-tauri/src/needle/tags.rs         stem cleaning + region/disc tags (regex)
src-tauri/src/bin/cfw-embed.rs       C-API embed host
src-tauri/tests/needle.rs            offline + env-gated tests
```

**Routing tiers (per included file, in order — first hit wins)**

1. **Folder match (deterministic, today's behavior)**: file sits under a
   library folder matching `system.folder` or `system.id`
   (`find_named_dir` semantics). Unchanged from `romcopy::plan_copy`.
2. **Unique-extension match (deterministic, new)**: build an
   extension→systems map from the included profile systems; if the file's
   extension is claimed by exactly one system, route there. `.gba`,
   `.sfc`, `.nes`, `.chd` are unique today; `.zip` is claimed by
   gba/snes/nes in the arkos/r36s profiles — exactly the ambiguity tier 3
   exists for. `.gdi` implies dreamcast (spike rule; no shipped profile
   has dreamcast yet, the rule ports anyway).
3. **Embedding nearest-neighbor (Needle)**: `tags::clean_stem` the
   filename, embed, k=1 cosine against the library index. Similarity ≥
   `AUTO_ROUTE_SIM` (0.985, the spike's precision point: 97.1% accuracy
   at 86% coverage) ⇒ auto-route to the index entry's system. Below ⇒
   **needs_review** with the nearest title and similarity shown; the user
   picks a system or skips. (Threshold is a named constant; 0.975 =
   94.7% @ 95% coverage is the documented coverage-first alternative.)
4. **needs_review**: surfaced by `classify_library`, resolved by explicit
   user choice in the UI before any plan exists.

**The library index** (`needle/index.rs`)

- Build: walk the library; every file that tiers 1–2 route deterministically
  is an index entry `(system_id, canonical_stem, vector)`. Files that
  *don't* route deterministically (the future queries) are excluded from
  the index. Cost ~5 ms/title (spike): a 5k-title library ≈ 30 s once,
  behind `classify-progress` events.
- Persist: **one index per profile** at
  `store_root()/needle/index/<profile_id>.bin` — tiers 1–2 are
  profile-dependent (`.bin` is psx-unique in `r35s-stock-card` but
  claimed by psx *and* megadrive in `r36s-clone-card`), so a shared
  cache would reuse the wrong index across profiles. Header (magic,
  format version, dim=3072, count, fingerprints) + f32 matrix + key
  table. Written `.tmp`-then-rename (the `.cfwpart` discipline). Three
  fingerprints gate reuse, and any mismatch rebuilds: the library
  (path + file count + size/mtime hash), the profile *routing* (a hash
  of the `(system_id, folder, extensions)` tuples actually used, so an
  unrelated profile version bump does not force a rebuild), and the
  weights (the manifest tag of the `.cact` the vectors came from —
  embeddings are a function of (text, weights), so a pin bump must not
  mix vector generations). Loaded indexes are bounds-checked and
  dimension-capped (a corrupt or hostile cache is an error, never
  memory unsafety).
- Query: cosine k=1 in plain Rust (no BLAS needed at this scale).

**Variant collapse (1G1R-lite, deterministic given the routing)**

- Group routed files by `(system_id, clean_stem)` — the cleaner strips
  region tags `(U)/(USA)/(Europe)/[E]/PAL…`, disc tags, scene junk, and
  separators (the spike's `clean_stem`, ported into `tags.rs` with its
  fixture tests).
- Within a group, the existing region checkboxes (USA/EUR/JPN, default
  USA) become an ordered preference: keep the highest-priority region
  present, all discs of it; other regions' variants become
  `skip (variant)` actions. Ties break deterministically (fewest junk
  tokens, then larger file, then lexicographic). This also resolves the
  deferred "region list fixed to USA/EUR/JPN" item *for the smart path*
  (the DAT path keeps its exact-checkbox semantics) without shipping a
  wider region vocabulary.

**Command integration (gates untouched)**

- New `classify_library(profile_id, library, include, regions)` →
  `{ routes, needs_review, groups, index_stats }` — the review payload.
- `plan_roms`/`copy_roms` gain an optional `smart` payload carrying the
  user's review choices, keyed by an opaque `review_id` so no free-form
  path crosses IPC. `classify_library` assigns ids sequentially over a
  deterministically sorted classification. The sort key is
  `(parent_path, file_name, size)` — the parent's **path relative to
  the library root**, plus the full file name — because weaker keys are
  not total: two files in one folder can share a stem and size with
  different extensions (`Game (USA).zip` beside `Game (USA).7z`), and a
  folder *name* collides across nesting (`nes/Game.nes` at the library
  root and `roms/nes/Game.nes` both key as `("nes", …)`). A file's
  relative path is unique within a library, so the order is total, ids
  are stable across the preview and copy-time classifications, and
  collisions are impossible by construction. The components are hashed,
  never used as paths, so the no-free-form-path property is preserved.
  With `smart` set, planning routes via `needle::sort`
  instead of `find_named_dir`; destinations still come from
  `folder_map::storage_folder` + profile values, still validated by
  `ensure_safe_dest` at plan and per-item time.
- `copy_roms` **re-classifies at copy time** (embeddings are
  deterministic) and refuses with "the library changed since Preview —
  preview again" if routes disagree with what the review approved.
  `romcopy::execute_copy`, `.cfwpart` rename, `volume::allow_copy`, and
  the `check_card_safety` armed-firstboot gate are untouched — smart
  copies pass through the identical moat.

**UI**

- The library step's sort section becomes a three-way choice: *No sorting
  (copy as-is)* / **Smart sort (no DAT needed)** / *Sort with a DAT
  (igir)*. The DAT path is exactly today's code.
- Smart mode shows an engine chip: Not installed (button: "Download the
  smart-sort engine · ~35 MB") → Installing… → Ready → Failed (reason +
  retry). Absent engine = the option renders with the chip, never blocks
  anything else.
- Preview with smart mode runs `classify_library`: if needs_review is
  non-empty, a review table (file, suggested system, nearest title,
  similarity) defaults to the suggestion and takes one click to accept
  all; then the familiar plan view renders with routed destinations and
  `skip (variant)` rows visible and overridable.
- **Cold start is an expected degradation, not a bug**: the index holds
  only files that tiers 1–2 route deterministically, so a library that
  is *entirely* unorganized zips (no system folders, no unique
  extensions — think a raw arcade or PSX dump folder) yields a near-
  empty index and every file lands in needs_review. When
  `index_stats.titles` is below a small threshold (~25), the smart
  option says so up front ("too few organized titles to auto-route —
  sort files into system folders first, or use a DAT") instead of
  promising a review wall. Note for live verification: the Tiny Best Set
  bridge (junctions into system folders) indexes fine, so the hardware
  run does *not* cover the cold-start case — that needs a synthetic
  fixture test.

### Phase 2 — Card Doctor

- `diagnose_card(volume_id)` serializes exactly what the gates already
  see into `CardFacts`: lettered volumes of the disk (from
  `lib.rs::lettered_partitions`, moved behind a module boundary for
  reuse), labels/file systems/sizes (`volume::list_volumes`), partition
  count, top-level folder names of the card root (capped), and the
  firstboot gate outcome (`firstboot::card_safety` + `boot_letter` +
  marker-file presence — the same `WIPE_SCRIPTS` the gate checks).
- Needle (serve mode, base weights, a `card_doctor` tools.json shipped as
  an `include_str!` asset so app and any future training share one
  schema — the spike's open item 3) extracts typed
  `{card_family, firstboot_state, safe_to_touch, likely_profile,
  explanation}` from that bare-facts text.
- **Deterministic overlay, in the model's favor**: family/firstboot are
  *also* computed by rules; where a rule exists, the rule wins and Needle
  supplies only the plain-language rendering ("expandtoexfat.sh remains
  on BOOT" → "boot the handheld once to the game menu, shut down, then
  reinsert — copying now would be erased by first boot"). Known refusal
  strings get deterministic templates first; Needle paraphrases the
  uncovered combinations. `likely_profile` is a suggestion that links to
  the profile screen. No doctor output is executable: it never calls
  prepare/copy/flash.
- Tests assert the gates return byte-identical results with the engine
  on, off, and returning garbage.

### Phase 3 — embedding dedupe in copy preview

- `romcopy` today skips only same-destination same-size files
  (`same_size` ⇒ `SkipUnchanged`); "Advance Wars (U).gba" and
  "Advance Wars (USA) [!].gba" both copy (and the real-library run showed
  458 skipped exact-name duplicates — the renamed kind is invisible).
- After planning, embed the on-card stems in the affected destination
  folders plus the plan's copy-item stems; group within a system at
  cosine ≥ 0.985 (the own-title variant band; mangled variants of the
  same title sit ~0.99, unrelated titles ~0.92–0.97). Preview shows
  "these N files look like games already on the card" with per-group
  keep/skip — default skip for incoming, never touching what's already
  on the card. Dedupe only removes items from the plan; nothing deletes
  from the card, and `execute_copy` is unchanged.

### Phase 4 — natural-language surface, profile matcher, diagnostics triage

- **NL command surface**: existing Tauri commands exposed as serve-mode
  tools (`check_card`, `prepare_card`, `plan_copy`, `execute_copy`,
  `list_flash_disks`). Needle proposes a tool call; the app renders it as
  a normal button/step with the existing confirmations — the user still
  types FORMAT/FLASH, the gates still dispose. A tool call can never
  shortcut a confirmation because confirmations are read from typed
  input, not from model output.
- **Profile matcher**: plain-words device description → suggest a
  profile with confidence. Reuses the embed helper: index the profile
  descriptions/docs/field notes (all repo content), NN the query. No
  fine-tune needed; the calibrated-confidence question is an open item.
- **Diagnostics triage**: paste a `diagnostics.log` excerpt (explicit
  local action) → typed `{phase, gate_refused, error_code,
  suggested_fix}` or a prefilled issue body.

### Phase 5 (long game) — on-device sorting companion

Needle's stated purpose is tiny ARM devices; the RGB10X is RK3326 with
1 GB RAM, plausibly running a 2–4-layer sliced subnet. `needle build
--layers N` exists in the pinned CLI (2..20 rungs) but was **not
exercised in the spike** — the ~8 MB size guess and every RK3326 number
are the first measurement spike's job, not facts. The delivery
precedent is real but belongs to the stock cards this app replaces:
those *contain* `tools/PortMaster` and `Scan_for_new_games.*` markers —
evidence that handheld-side scan helpers are an expected card pattern —
while `seed.rs::seed_folders` today creates only `folder_map` system
and bios folders, so Card Studio would first need a new `tools/`
seeding recipe before it could host a helper + sliced weights during
prepare. Then the handheld organizes dropped ROMs at boot with no PC in
the loop. **Gated on measurement first**: a sliced model must be timed
on real RK3326 hardware before any of this is promised (open question;
deliberately last).

### Feature-detect: the app works identically without the engine

- `needle_status()` → `absent | installing | ready | failed(<reason>)`.
- Absent/failed: the wizard is exactly today's app; smart sections render
  as inert options with install/retry chips; `classify_library` returns a
  typed "engine not available" the UI maps to the chip. No spawn, no
  download, no behavior change in any other step.
- Crashed helper/sidecar mid-run: the operation fails with a plain
  reason, the tier disables for the session, and the user can fall back
  to no-sorting or the DAT path. The moat never depended on it.

## Testing

- **Offline unit tests** (`src-tauri/tests/needle.rs` + module tests):
  - manifest/acquire: pin-table parse; verify-then-rename with fixture
    files and an injectable fetch (the `flash::ensure_image` seam);
    mismatch discards; cache reuse; `CFW_NEEDLE_DIR` order.
  - `tags.rs`: stem cleaning + region/disc tag fixtures (port the spike's
    generator vocabulary: `(U)/[E]/PAL/Disc 1/of N/cd1`, mixed case and
    separators); `.gdi` ⇒ dreamcast.
  - `index.rs`: cosine/k=1/threshold math against synthetic vectors via
    an `EmbedFn` trait seam (tests inject deterministic vectors; the real
    FFI is env-gated); corrupt/truncated/dim-mismatched cache rejected;
    fingerprint rebuild logic; **two profiles over one fixture library**
    build two correct indexes (the `.bin`-psx-unique-in-`r35s-stock-card`
    vs psx+megadrive-in-`r36s-clone-card` case) and neither is reused
    for the other.
  - `sort.rs`: tier ordering (folder beats extension beats NN), unique-
    extension map from real profile fixtures, variant collapse with
    region priority and deterministic tie-breaks, needs_review set,
    enum validation (model output outside the profile's system ids ⇒
    needs_review, never a path), and the cold-start case (empty/tiny
    index ⇒ everything needs_review plus the small-index messaging
    state).
  - copy-time guard: plan/copy determinism given the same index;
    changed-library refusal; review-id stability across two
    classifications, including a same-stem-different-extension pair in
    one folder (`Game (USA).zip` beside `Game (USA).7z`) and the same
    file name under same-named folders at different depths
    (`nes/Game.nes` and `roms/nes/Game.nes`), proving the total sort
    key.
- **Env-gated integration** (`CFW_NEEDLE_TEST=1`, the `CFW_IGIR_TEST=1`
  pattern; the built `cfw-embed` helper against real weights resolved
  from `CFW_NEEDLE_DIR`):
  - real `cfw-embed` against real weights: embed determinism (same stem ⇒
    identical vector), latency budget per title;
  - a messy fixture library (text dummies, names only — QA-dummy style,
    never real ROM content) routed end to end, asserting destinations.
  - Phase 2 adds the mock-server protocol test for the serve client
    (reset-then-complete ordering, suppressed-call discount, `NoCall`
    fallthrough) — the ingestion `needle_test.rs` pattern.
- **Live verification** (real hardware, notes in the commit per AGENTS.md):
  the user's Tiny Best Set GO bridge as the messy library, smart-sorted
  onto the R36S clone card and the expanded EASYROMS card; on-card counts
  compared against the known-good igir DAT run where DATs exist; Card
  Doctor pointed at a post-flash armed card (fixture-tested only —
  re-arming a real card risks a wipe, per the Phase 3 testing precedent).
- **Windows caveat honored**: all `tauri::Emitter` use stays in `lib.rs`;
  the `needle/` module and helper stay pure so bare cargo test binaries
  load (STATUS_ENTRYPOINT_NOT_FOUND otherwise).

## Safety review

- **Needle never disposes.** Routes, explanations, and duplicate groups
  are proposals rendered in Preview; every write still passes
  `volume::allow_copy`, `check_card_safety` (armed-firstboot), per-item
  `ensure_safe_dest`, `.cfwpart` rename, and — for flash — disk-0
  refusal, SHA-256-verified image, typed FLASH, UAC. One line of gate
  code changes in no PR of this design.
- **No model string becomes a path component.** A route is a `system_id`
  validated against the loaded profile; destinations are computed by
  `folder_map::storage_folder` from `profiles::is_safe_folder`-checked
  values. Serve-mode answers are grammar-constrained *and* vocabulary-
  validated in the parser (out-of-vocab ⇒ discard/needs_review + 0.25×
  confidence, mirroring the ingestion parser's defenses).
- **Spawn hygiene** (post-security-review bar): direct binaries, no
  `cmd /C`, no shell string interpolation into spawns; unelevated
  children only; `NEEDLE_TELEMETRY=0` + `DO_NOT_TRACK=1` on every spawn;
  localhost bind only; children killed on app exit.
- **Downloads**: HTTPS + compiled-in SHA-256 pins + `.partial`/rename;
  mismatched bytes are discarded (`flash::ensure_image` contract).
  Pins in the app binary cannot be swapped by a compromised feed.
- **No content**: engine + weights are software artifacts (engine
  Apache-2.0; weights license pending Open Question 9); the app still
  never ships/downloads ROMs, BIOS, or DATs; Phase 1 indexes are built
  from the user's own filenames and stay on their machine.
- **Untrusted-input hardening**: the persisted index is bounds-checked
  and dimension-capped; helper stdio frames are length-capped; a hostile
  cache or a wedged helper is an error + disabled tier, never UB.
- **Residual risk accepted**: same-user malware could tamper with cached
  artifacts after download — in Phase 1 only the weights (data, whose
  worst case is bad vectors, bounded by index validation and enum-
  validated routing), in Phase 2 also the `needle.exe` executable (same
  class as the accepted flash-log spoofing residual). The pins catch
  network tampering, not local tampering; documented in the threat
  model note that ships with the licensing file.

## Observability (diag.rs event vocabulary)

New events extend the existing `diag::log(level, event, detail)` lines —
counts and statuses only, never ROM file names (existing rule):

- `needle_download_start` / `needle_download_done` / `needle_download_failed`
  (artifact + byte counts)
- `needle_engine_start` / `needle_engine_stop` / `needle_engine_failed`
  (which helper/sidecar, exit reason)
- `needle_index_built` (`titles=N ms=M rebuilt=bool`)
- `needle_classify_done` (`routed=A review=B unmatched=C`)
- `needle_copy_smart` (copy via smart plan: `files= variants_skipped=N`)
- Phase 2: `doctor_run` (`outcome=…`), `doctor_failed`
- Phase 3: `dedupe_preview` (`groups=N items=N`)

## Risks

- **`libneedle.a` may not link with the Windows toolchain** (High — PR 2a
  entry criterion, a build-time/CI concern, never a user-machine one):
  the shipped archive's object format is unverified against MSVC's
  `link.exe`. Mitigations in order: validate `link.exe` against the
  archive; build `cfw-embed` in CI with the matching (GNU) toolchain
  and distribute *our* helper as a pinned manifest download instead of
  bundling it; contribute/follow an upstream `/embed` route or `embed`
  CLI subcommand in `needle.exe` (Apache-2.0, we can patch); worst case
  Phase 1 ships deterministic tiers 1–2 only (still a real improvement,
  and PR 2b's index work stays valuable behind the `EmbedFn` seam) with
  NN behind the fix.
- **Upstream version drift** (High — already cost real time: 3.0.5's
  wheel 404s): pins compile into the app; cache reuse means an offline
  machine keeps working; error copy names the exact pinned URL for
  manual download into `CFW_NEEDLE_DIR`.
- **Silent mis-route files a ROM under the wrong system** (Medium):
  precision-first 0.985 auto-route; everything is visible in Preview and
  overridable; mis-route costs a wrong folder, never data loss; spike
  numbers bound the rate (≤3% of covered queries).
- **Resource cost on low-end PCs** (Medium): ~81 MB per engine process +
  up to ~123 MB index; lazy spawn, 5-min idle kill, index caps; the app
  is fully usable with none of it resident.
- **AV/SmartScreen flags an unsigned engine binary** (Medium, Phase 2
  only — Phase 1 downloads no executables): pinned hashes + clear error
  copy; long-term fix is code signing the app (pre-existing project
  question, not Needle-specific).
- **Supply chain on weights** (Medium): byte-pinned, HTTPS, data-only
  `.cact` never executed; the executable engine is pinned too.
- **Index staleness after library edits** (Low): fingerprint rebuild +
  copy-time re-classification guard.
- **Scope creep toward gate logic** (High, process): the "needle/ imports
  nothing from the moat, moat imports nothing from needle/" rule is the
  review checklist item for every PR here.
- **Phase 5 on-device feasibility unproven** (Low until attempted):
  explicitly gated on RK3326 measurement; nothing in Phases 1–4 depends
  on it.

## Alternatives considered

- **No-AI baseline** (ship deterministic tiers 1–2 only): folder +
  unique-extension routing alone handles most clean-ish libraries at zero
  new surface. Rejected as the *whole* answer (`.zip` ambiguity, misfiled
  games, and variant collapse are exactly the user's pain), but retained
  as the built-in fallback if the PR 2a link criterion fails — the design
  degrades to this gracefully.
- **Deterministic fuzzy folder matching** ("GBA Games", "nintendo_gba"
  → gba, as a tier 1.5 before embeddings): cheap and engine-free, but
  normalization rules are an unbounded guessing game ("roms", "games",
  "old stuff"?) and it does nothing for the actual hard cases —
  ambiguous extensions, misfiled games, variant collapse — which
  embeddings cover with measured accuracy. Not in v1; add as tier 1.5
  later if field data shows folder-name noise dominating (it composes
  cleanly in front of tier 3).
- **Cloud/BYOK inference** (the ingestion engine's Jev tier pattern):
  rejected — filenames stay local by product principle, the tool is
  offline-first (SD prep happens in weird places), per-call cost, and the
  post-security-review posture demands zero telemetry; a local engine is
  the only way to guarantee it.
- **Shim into the ingestion engine's sidecar** (share its process/port,
  or depend on that app being installed): rejected by explicit user
  directive — the integrations stay separated; also coupling two release
  cadences and requiring a second app for a first-run feature. We reuse
  its *artifacts and lessons* (`tools/needle/`, client/parser patterns,
  weight pins), not its runtime.
- **Ship DATs (or a DAT-derived index) to power sorting**: violates the
  never-download-DATs rule; a distilled embedding index is arguably a
  derived catalog, so it is excluded too (Key Decision 6).
- **Ship a Python runtime and use `cactus-needle` directly**: rejected —
  heavyweight install, fragile pins (the 3.0.5 trap), and a large new
  supply-chain surface; Python remains a developer-only training tool.
- **Bundle the weights in the installer**: rejected for v1 (Key
  Decision 4) — 35 MB for an optional tier, no user-deletable cache, and
  installer bytes spent on a feature many users never enable. The
  *engine* half of the old bundle objection no longer applies: the
  engine is bundled (a static archive cannot be linked after install,
  and installer-borne code is the stronger integrity story), and
  release-coupled bumps are inherent to compiled-in pins either way.
  Revisit weights bundling on field data.
- **Generation for identity lookup** (fine-tune harder): disproved by the
  spike (36% unseen-title platform accuracy; LoRA learns format, not
  lookup tables). Never proposed here.

## Open questions

1. Does `link.exe` accept the shipped `libneedle.a` objects (PR 2a
   build-time entry criterion), or do we build/pin our own helper /
   upstream an `/embed` route?
2. Prebuilt index shipping: is a DAT-derived embedding index ever
   acceptable under the no-DAT rule, or is user-library-built forever?
   (v1 assumes forever.)
3. Threshold tuning on real data: 0.985 vs 0.975 measured on the Tiny
   Best Set bridge before Phase 1 ships.
4. Feed-distributed needle pins (let the profile feed bump engines)
   vs compiled-in pins — compiled-in for v1; feed bumps gain update
   agility but make the feed a trust anchor for native binaries.
5. Calibrated confidence for Card Doctor: local LoRA drops the
   confidence head (`null` ⇒ parser default 0.75); a one-off hosted
   platform fine-tune keeps it. Needed only if doctor suggestions must
   carry real confidences.
6. Classification cancellation: a started `classify_library` runs to
   completion (Tauri commands aren't cancellable today); progress events
   only. Fold into the deferred stage-cancel item?
7. Phase 5 feasibility: measured latency/RAM of a sliced 2–4-layer
   subnet on RK3326 (1 GB) before any on-device companion work is
   scheduled.
8. Non-Windows engines: the spike verified `windows-x86_64` only, and
   Card Studio's volume layer is Windows-only today; manifest rows for
   other platforms land when the app does.
9. Does the HF weights repo (`Cactus-Compute/needle3`) carry the same
   Apache-2.0 license as the code repo for `needle3.cact` specifically?
   Confirm before go-live; the licensing docs must state it accurately.
10. Does `needle.exe --serve` accept `--port`? (The spike used the
    default :8080; verify on the first Phase 2 smoke test.)

## References

- Card Studio specs: `docs/superpowers/specs/2026-09-25-igir-sorting-design.md`,
  `docs/superpowers/specs/2026-09-26-profile-feed-design.md`,
  `docs/superpowers/specs/2026-09-25-diagnostics-log-design.md`
- Card Studio modules cited: `src-tauri/src/{lib,volume,format_gate,
  prepare,romcopy,folder_map,profiles,feed,firstboot,flash,diag,igir,
  store,seed}.rs`, `src-tauri/src/bin/cfw-flash.rs`, `src/App.tsx`
- Rom Ingestion Engine (sibling repo, kept separate by directive;
  `C:\Users\aaron\Documents\antigravity\optimistic-euclid`):
  `docs/needle-spike-2026-09-26.md` (full spike record),
  `tools/needle/README.md` (pipeline: `tools.json`, `gen_data.py`,
  `data/*.jsonl`, `adapter.safetensors`, `eval_sidecar.py`,
  `embed_experiment.py`, WSL scripts), `src-tauri/src/classifier/needle.rs`
  (client + parser defenses this design mirrors),
  `src-tauri/src/chdman/downloader.rs` (pinned-manifest download pattern)
- Needle: github.com/cactus-compute/needle (Apache-2.0), HF repo
  `Cactus-Compute/needle3`; pip pin `cactus-needle==3.0.1` (tooling only)
- Project memory: `cfw-zero-touch.md` topic (phases, gates, profiles,
  hardware matrix, security-review history)

## PR Plan

Ordered; each PR is independently reviewable and leaves the tree green.
Capacity assumption: one developer plus subagent review; Phase 1 is
targeted next, Phases 2+ are unscheduled until Phase 1 is verified
live. Sizes are rough (S ≤ ~200 LOC + tests, M ~200–600, L > 600 or new
native surface) with the main review risk named. Phases 2+ PRs are
sketched at the same granularity but land only after Phase 1 is
verified live.

**PR 1 — needle: pinned engine manifest, verified acquire, store layout, licensing**
- Size/risk: S · low — plumbing only, no native surface.
- Files: `src-tauri/src/needle/{mod,manifest,acquire}.rs`,
  `src-tauri/src/lib.rs` (`needle_status`, `needle_acquire` commands,
  registration, `needle-progress` emit), `src-tauri/tests/needle.rs`,
  `THIRD_PARTY_NOTICES.md`, `README.md` (licensing note).
- Dependencies: none.
- Description: compiled-in pin table (`needle3.cact` and `needle.exe`
  as runtime downloads; `libneedle.a`/`needle.h` as build-time inputs):
  URL + SHA-256 + size each; acquire with `.partial`/verify/rename and
  injectable fetch; cache order `CFW_NEEDLE_DIR` →
  `store_root()/needle`; diag download events; offline tests over
  fixture files; Apache-2.0 attribution (engine + header; the weights
  line drafted pending Open Question 9, whose license-tag confirmation
  is on this PR's checklist). No UI.

**PR 2a — needle: embed-link spike + cfw-embed helper protocol**
- Size/risk: M · HIGH — the Windows-toolchain link question; if this
  fails, fall back per Risks before 2b proceeds.
- Files: `src-tauri/src/bin/cfw-embed.rs`, `src-tauri/build.rs` (vendor
  the pinned `libneedle.a`/`needle.h` from `CFW_NEEDLE_DIR`; clear
  build error when absent), `src-tauri/src/needle/embed_client.rs`,
  `src-tauri/tests/needle.rs`.
- Dependencies: PR 1.
- Description: entry criterion — prove the shipped `libneedle.a` links
  under the Windows toolchain (fallbacks per Risks: CI-built
  GNU-toolchain helper distributed as a pinned download, or an upstream
  embed route). Long-running stdio JSONL protocol (`ping`/`embed`/
  `embed_many`); telemetry env vars; idle-kill/exit-kill lifecycle;
  `CFW_NEEDLE_TEST=1` real-weights test (embed determinism, latency
  budget).

**PR 2b — needle: library index build/persist/query**
- Size/risk: M · medium — binary format + fingerprint rules; valuable
  even if 2a's link fails, behind the `EmbedFn` seam.
- Files: `src-tauri/src/needle/index.rs`, `src-tauri/tests/needle.rs`.
- Dependencies: PR 2a for the real engine; none for the offline tests
  (synthetic vectors through `EmbedFn`).
- Description: per-profile persistence
  `store_root()/needle/index/<profile_id>.bin`; library +
  profile-routing + weights fingerprints; `.tmp`-then-rename;
  bounds/dim checks; cosine k=1 query. Offline tests including the
  two-profiles-one-fixture-library case (`.bin` unique to psx in
  `r35s-stock-card`, psx+megadrive in `r36s-clone-card`).

**PR 3 — needle: deterministic filename tags (parallel track)**
- Size/risk: S · low — pure functions, no engine, no I/O.
- Files: `src-tauri/src/needle/tags.rs`, tests.
- Dependencies: none (merges before PR 4).
- Description: port the spike's `clean_stem` + region/disc tag parsing
  (regex, 100% on tagged names) with the spike's tag-vocabulary
  fixtures; `.gdi` ⇒ dreamcast rule. Pure functions, no engine.

**PR 4 — needle: smart-sort planner + command integration**
- Size/risk: L · medium — the planner is Phase 1's core logic; all
  behind seams, moat untouched.
- Files: `src-tauri/src/needle/sort.rs`, `src-tauri/src/lib.rs`
  (`classify_library` command; `plan_roms`/`copy_roms` optional `smart`
  payload with opaque `review_id`s; `classify-progress` emit),
  `src-tauri/tests/needle.rs`.
- Dependencies: PR 2b, PR 3.
- Description: routing tiers 1–3 + needs_review; variant collapse with
  region-priority and deterministic tie-breaks; enum-validated routes
  (model picks only profile system ids); opaque review ids assigned
  over a deterministically sorted classification; copy-time
  re-classification guard; `needle_classify_done` diag event. Offline
  plan tests with injected embeddings plus the cold-start case; moat
  modules untouched (diff shows zero changes under them).

**PR 5 — needle: smart-sort UI + engine install flow**
- Size/risk: M · low backend risk, UX-iteration risk.
- Files: `src/App.tsx` (sort-section mode toggle, engine chip + consent
  download, review table, variant groups in plan view, cold-start
  messaging, badges), `src/App.css`.
- Dependencies: PR 1, PR 4.
- Description: three-way sort choice (none / smart / DAT — DAT path
  untouched); engine status chip with install/retry; review UX with
  accept-suggestions default; small-index messaging ("too few organized
  titles to auto-route — sort files into system folders first, or use a
  DAT"); plan view shows routed destinations and `skip (variant)` rows.
  Live hardware verification noted in the commit (Tiny Best Set bridge
  → R36S clone card, counts vs the igir run); a synthetic cold-start
  fixture covers the all-zips case the live run cannot.

**PR 6 — needle: serve sidecar lifecycle + client (Card Doctor backend)**
- Size/risk: M · medium — first runtime-executable download plus a
  protocol client; mock-tested throughout.
- Files: `src-tauri/src/needle/{serve,client}.rs`,
  `src-tauri/src/needle/tools/card_doctor.json` (`include_str!`),
  `src-tauri/tests/needle.rs`.
- Dependencies: PR 1.
- Description: spawn `needle.exe --serve` (pinned download at first
  doctor use) on a picked free port — `--port` verified on the first
  smoke test; close-then-spawn race accepted with a retry on bind
  failure; direct binary + telemetry env vars + health check +
  idle/exit kill. The client is **synchronous ureq on a dedicated
  worker thread** (this app has no tokio), porting the ingestion
  client's 2 s/20 s timeouts, retry, `NoCall` fallthrough, and
  vocabulary-validated parser into Card Studio-owned code; mock-server
  protocol tests.

**PR 7 — needle: Card Doctor (facts, templates, UI)**
- Size/risk: M · low — read-only surface, display-only output.
- Files: `src-tauri/src/needle/doctor.rs`, `src-tauri/src/lib.rs`
  (`diagnose_card`; move `lettered_partitions` behind a reusable
  boundary), `src/App.tsx` (Explain-this-card panel), tests.
- Dependencies: PR 6.
- Description: `CardFacts` serializer over what the gates already see;
  deterministic explanation templates for known refusals first, Needle
  paraphrase/`likely_profile` second; doctor output display-only. Tests
  assert byte-identical gate results with the engine on/off/garbage.

**PR 8 — needle: embedding dedupe in copy preview**
- Size/risk: S/M · low — reuses the 2b index and 5's UI machinery.
- Files: `src-tauri/src/needle/dedupe.rs`, `src-tauri/src/lib.rs`
  (duplicate groups in `PlanView`), `src/App.tsx` (group confirm UI),
  tests.
- Dependencies: PR 2b, PR 5.
- Description: embed on-card + plan stems, group at ≥ 0.985 within a
  system, preview "N files look like games already on the card" with
  keep/skip per group (default skip incoming, never touch card
  residents); dedupe only removes plan items; `execute_copy` unchanged.

**PR 9+ — Tier 2 surface (after Phases 1–3 verified in the field)**
- NL command surface (serve tools over existing commands; confirmations
  still typed by the user), profile matcher (embed profile docs via the
  existing helper), diagnostics triage (pasted-log extraction). Each its
  own PR against PR 6's client; no new trust surface.

**PR 10+ — Phase 5 on-device companion (measurement-gated)**
- First PR is a measurement spike only: exercise `needle build --layers
  N` (untested in the spike), size the sliced artifact, and time it on
  real RK3326 hardware; any card-writing helper work is a follow-up
  that goes through the full gate review (it writes to cards *outside*
  the PC moat, so it earns its own safety section).

## Implementation amendment � PR 2a (2026-09-26)

The PR 2a entry criterion (does the shipped `libneedle.a` link under the
Windows toolchain?) was tested live. Outcomes, all verified against the
pinned artifacts at revision `b274efcb`:

1. **MSVC `link.exe` rejects the archive** � `LNK1143: no symbol for COMDAT
   section` on `needle.cpp.obj`. The archive's COMDAT form is GCC/LLVM-style;
   this is an object-format incompatibility, not a missing-library problem.
2. **Rust `x86_64-pc-windows-gnu` cannot link it cleanly either** � the
   engine needs LLVM libc++ (`std::__1` symbols) and the vendor targets
   UCRT (needle.exe imports `api-ms-win-crt-*` only), while Rust's gnu
   target links its own msvcrt-based `crt2.o`; the two CRTs collide.
3. **llvm-mingw clang++ links it exactly as the vendor did** � this is the
   PR 2a recipe: `tools/cfw-embed/cfw-embed.cpp`, a small C++ helper built
   by `tools/cfw-embed/build.ps1` with a pinned llvm-mingw toolchain
   (`-static`, static libc++/libunwind, no extra DLLs). Measured against
   the real weights: dimension 3072, byte-identical determinism for
   repeated embeds, ~4.4 ms per embed (~6 ms over pipes), helper survives
   malformed ops, clean exit on stdin close.

Deviations from the original PR 2a sketch, recorded here as the plan of
record for 2b:

- `cfw-embed` is C++ built by the llvm-mingw script (or a future CI job),
  not a `src-tauri/src/bin/*.rs` cargo binary and not vendored through
  `build.rs`. Consequently the design's "local build without them fails
  with instructions" becomes: the helper is built out-of-tree; the app
  resolves it at runtime (`CFW_EMBED_EXE`, beside the exe, store cache).
- The app?helper wire format is the length-prefixed line protocol
  documented in `src-tauri/src/needle/embed_client.rs`
  (`PING`/`EMBED <len>`), not JSONL: a foreign-toolchain helper gets no
  hand-rolled JSON parser. App?frontend stays JSON via Tauri.
- A manifest row pinning a CI-built `cfw-embed.exe` download lands with
  the CI job that first publishes it (follow-up to this PR); until then
  the helper is developer-built via the script.
- `embed_client.rs` ships in PR 2a as designed (spawn/protocol/lifecycle,
  idle-kill after 5 min, `kill_all` wired to `RunEvent::Exit`), with
  offline codec tests plus the `CFW_NEEDLE_TEST=1` real-helper test
  (determinism, dimension, latency budget) mirroring the `CFW_IGIR_TEST`
  pattern.
