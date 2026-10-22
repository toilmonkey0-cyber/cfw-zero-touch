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
- [x] Guided setup (unit 08): one wizard chains backup, quick probe, optional full check, stock-device detection, flash, boot-once, and copy by calling the existing commands. Each destructive step keeps its own typed confirmation. Skipping backup means this setup cannot restore the card. `probe_card` and `full_check_card` do not take a route from the wizard; they still run when this app session has a backup of that exact card, or the card is blank. The wizard reads disk number, serial, size, and partition-table id with the read-only `read_disk_identity` command. USB card readers often report their own serial for every card, so the partition-table id (the MBR signature or the GPT disk GUID from `Get-Disk`) is what stays with the card. That id is read again after flash, because flashing rewrites the partition table, and copy waits until the reinserted card matches the recorded disk number, serial, size, and partition-table id. `list_volumes` returns each volume's disk number from `Get-Partition`, and the games partition is used only when that disk number and serial are the rechecked card. The wizard pins the card on the first disk step. A failure or cancel stops the wizard and reports OVERWRITTEN only when the command said the card was overwritten. Detection pre-selects; the user confirms the handheld.
- [ ] Follow-up: `flash_os` returns only a log string, so a guided flash still says the result did not say whether the card was overwritten. Leave that until `flash_os` itself reports OVERWRITTEN. The wizard must not invent the flag.
- [x] Stock-card device and screen detection (unit 07): read-only fingerprints live in the profile feed. A shipped fingerprint is a real DTB name and sha256 with a cited source (see `docs/FINGERPRINTS.md`). Partition layout is stored only when it was recorded from a real card; otherwise matching uses the DTB name and hash. A profile with no real fingerprint is `manualOnly`. A unique match pre-selects the profile; the user confirms the device and the screen. Unknown or ambiguous cards stay on the manual picker.
- [x] Crash reporting → local diagnostics log (2026-09-25): `diagnostics.log` in the store root records app start, prepare/flash/stage/copy outcomes, firstboot refusals, feed checks and applied updates, and panics — with UTC timestamps and 1 MiB rotation to `diagnostics.old`. Nothing is ever transmitted; the footer's "Show diagnostics log in folder" opens it.
- [x] Licensing (unit 09): optional Gumroad license verify for signed builds and support, an offline cache bound to a machine-id hash, and a 3-computer soft limit that warns and does not lock. Backup, restore, probe, full check, guided setup, and the disk refusals do not consult it. The product id is a build-time placeholder. Tests use recorded Gumroad responses and never call the live API. See `docs/LICENSING.md`.
- [x] Diagnostics support file (unit 11): one click saves a zip with the app version, OS, recent diagnostics notes, the latest card probe, identity, and fake-card result, and license status. The identity file includes the card serial and the partition table id. ROM names, library paths, usernames, the license key, cached tokens, HMAC secrets, and Gumroad bodies are left out. Building the zip does not open a card. The Save support file button works with no license.
- [x] Release pipeline (unit 12): a `v*` tag builds the Windows installer, writes `SHA256SUMS`, and opens a draft GitHub release. You publish the draft. Code signing stays off unless `SIGNING_ENABLED` is `true` on a `v*` tag push. Compile and bundle never see the certificate. Two later jobs check out `scripts/` only, verify the previous job's hashes, and sign the app and the `cfw-flash` helper from the `release` environment. The installer sign job extracts both installers first and checks the inner executables against those hashes and the signer thumbprint. See `docs/RELEASING.md`.
- [x] Feature gating (unit 10): signed builds and support open an Enter key dialog. A valid key unlocks them, a refunded key stays locked with a plain message, and an offline re-check after one activation stays unlocked. The user can view or deactivate the license. ROM-card prep and the safety paths (identity, probe, fake-card result, backup, OVERWRITTEN) run with no key and do not wait on licensing. Gating is in the UI and the `paid_feature` command, not in the disk writers. See `docs/LICENSING.md`.

## Testing strategy
- Unit: profile parse, path mapping, disarm patch apply on fixture tree.
- Integration: loopback/.img fixtures with partitioned disks (Linux CI).
- Hardware matrix spreadsheet: device × CFW × storage mode — manual QA.

## Suggested Grok Build kickoff prompt
See `GROK_BUILD_HANDOFF.md`.

