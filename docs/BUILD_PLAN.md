# Grok Build — implementation plan (approve in Plan Mode)

## Phase 0 — Repo skeleton (day 1)
- [ ] Init git repo `cfw-zero-touch` (or rename to product name).
- [ ] Tauri 2 + React/Svelte frontend scaffold.
- [ ] CI: `cargo test`, `pnpm test`, format/lint.
- [ ] `AGENTS.md`, `.grok/skills/`, profile schema in `specs/profile.schema.json`.
- [ ] README with legal disclaimer.

## Phase 1 — Profiles + ROM card path (safest wedge)
- [ ] Profile schema: id, cfw, device, image URLs, checksums, `romFolders[]`, `biosFolder`, `storageModes[]`.
- [ ] Ship 2 profiles: `arkos-easyroms-roms-card`, `rocknix-roms-card`.
- [ ] UI: format/select empty FAT/exFAT volume → create folders → copy ROMs from library.
- [ ] Tests: folder map fixtures; dry-run copy.

**Ship checkpoint A:** useful without touching firstboot. Dual-card users get value immediately.

## Phase 2 — Flash pipeline
- [ ] Download + xz/gz decompress + SHA256.
- [ ] Removable disk enumeration (Win/macOS/Linux).
- [ ] Privileged write + read-back verify (study Arch R Flasher approach).
- [ ] “Flash OS only” mode.

**Ship checkpoint B:** smarter Etcher for selected profiles.

## Phase 3 — Single-card ArkOS firstboot safety (the moat)
- [x] Reverse/document ArkOS firstboot scripts per target image (`expandtoexfat`, `roms.tar`, `firstboot.service`).
- [x] Prepare recipe, adapted: the handheld performs the expansion itself. After a dArkOS flash the wizard routes through a boot-once step (boot to the game menu, shut down, reinsert), and `firstboot_state` refuses to plan or copy while `expandtoexfat.sh`/`firstboot.sh` remain on the card's BOOT partition. PC-side resize/mkfs/extraction was dropped deliberately: this card reader must not grow or format EASYROMS, and games copied before the handheld finishes first boot are erased.
- [x] Golden-path integration test: fixture-tree tests cover armed BOOT (refuses), disarmed BOOT (allows), and missing BOOT (unknown); the spare-SD golden path ran for real on the RGB10X dArkOS card (flash → first boot → copy into expanded EASYROMS).
- [x] Failure recovery: an interrupted copy writes to a `.cfwpart` temp name and renames, so the card never holds a truncated ROM that looks complete; a failed flash tells the user to re-run it (a card is only usable after a read-back match); the armed-card refusal carries boot-once instructions.

**Ship checkpoint C:** the single-card dArkOS path is safe end to end: flash → boot once → gate → copy into expanded EASYROMS.

## Phase 4 — Clone/DTB + polish
- [x] Profile pack feed + update checker (2026-09-26): `profiles-feed.json` at the repo root, fetched over HTTPS, schema-validated entry-by-entry, applied to the writable runtime store (`%LOCALAPPDATA%\cfw-card-studio\profiles`, seeded from bundled profiles) only on an explicit Update click. The feed carries the latest app version for a "new build available" banner. An anti-drift test keeps the feed matching `profiles/`.
- [x] Igir optional backend for sorting (2026-09-25): an optional "Sort with a DAT" section stages the library through igir (1G1R, region filter, checksum verification) before Preview; igir writes only to a local staging folder and never touches the card. Inert without a user-supplied DAT folder; per-system `datNamePattern`s ship in the profiles (feed v2).
- [ ] DTB/panel step: dropped (2026-09-26) — the R36S clone was solved with its own games-card profile and no current device needs a DTB swap.
- [x] Crash reporting → local diagnostics log (2026-09-25): `diagnostics.log` in the store root records app start, prepare/flash/stage/copy outcomes, firstboot refusals, feed checks and applied updates, and panics — with UTC timestamps and 1 MiB rotation to `diagnostics.old`. Nothing is ever transmitted; the footer's "Show diagnostics log in folder" opens it.

## Testing strategy
- Unit: profile parse, path mapping, disarm patch apply on fixture tree.
- Integration: loopback/.img fixtures with partitioned disks (Linux CI).
- Hardware matrix spreadsheet: device × CFW × storage mode — manual QA.

## Suggested Grok Build kickoff prompt
See `GROK_BUILD_HANDOFF.md`.

