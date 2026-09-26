# AGENTS.md — CFW Zero-Touch Card Studio

## Mission
Build a desktop app that flashes handheld CFW **and** prepares the games partition on the PC so users skip the flash → boot → pull card → copy ROMs shuffle.

## Read first
- `docs/MARKET.md` — gap, competitors, landmines
- `docs/PRODUCT.md` — scope / non-goals / v1 matrix
- `docs/BUILD_PLAN.md` — phased delivery
- `specs/profile.schema.json` — profile contract
- `GROK_BUILD_HANDOFF.md` — kickoff prompts

## Hard rules
1. **Never** copy ROMs onto an ArkOS-class card and call it done without PC-side firstboot **or** a dual-card ROMs-only mode. Firstboot reformats EASYROMS and wipes user data.
2. **Never** ship, bundle, or download copyrighted ROMs/BIOS. User supplies paths.
3. Prefer **narrow profiles** over generic “all CFWs” until the pipeline is proven.
4. Removable disks only; require explicit typed confirmation before wipe/flash.
5. Use Plan Mode (`/plan`) for architecture and Phase 3 firstboot work; get approval before editing.
6. After substantive changes: run tests/lints; do not leave the tree broken.
7. Cite real CFW docs/scripts when implementing firstboot recipes; do not invent partition layouts.

## Stack
- Default: **Tauri 2 + Rust backend + typed frontend** (aligns with Arch R Flasher precedents).
- Profiles are data (`profiles/*.json`), not hard-coded UI branches.
- Privileged disk ops isolated in a small helper with clear OS-specific backends.

## Verification
- Unit tests for profile mapping and disarm patches on fixture trees.
- Do not claim hardware success without a real SD test note in the PR/commit message.

## Git
- Small commits; no force-push to main; no secrets in repo.
