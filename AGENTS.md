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

## Disk safety
1. Removable disks only. Never offer or write disk 0, the system disk, or a removable disk that reports size 0.
2. A wipe or flash requires the exact typed confirmation (`FORMAT` or `FLASH`) and a size check against the disk before any write.
3. Tests use disk-image fixtures and never real disks. A test or CI job must not open, partition, format, or write a physical drive.
4. Never edit `src-tauri/src/flash.rs`, `src-tauri/src/bin/cfw-flash.rs`, or `src-tauri/src/volume.rs` unless the pull request has the `disk-safety` label.

## Stack
- Default: **Tauri 2 + Rust backend + typed frontend** (aligns with Arch R Flasher precedents).
- Profiles are data (`profiles/*.json`), not hard-coded UI branches.
- Privileged disk ops isolated in a small helper with clear OS-specific backends.

## Verification
- Unit tests for profile mapping and disarm patches on fixture trees.
- Tests use disk-image fixtures and never real disks.
- Do not claim hardware success without a real SD test note in the PR/commit message.
- Before a pull request, follow `.grok/skills/verify/SKILL.md`.

## Git
- Small commits; no force-push to main; never commit secrets (tokens, private keys, passwords, or `.env` files).
- One unit per pull request, on a branch named `unit/NN-name` (for example `unit/00-repo-guardrails`). Squash-merge into `main`.
- Never edit `src-tauri/src/flash.rs`, `src-tauri/src/bin/cfw-flash.rs`, or `src-tauri/src/volume.rs` without the `disk-safety` label.

## Cursor Cloud specific instructions
- Login shells provide Node.js 24 (`node -v` should report 24.x) and Rust stable. Frontend checks from the repo root: `npm test` (`tsc --noEmit` and ESLint) and `npm run test:unit` (Vitest). Rust checks from `src-tauri`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- The desktop app is `npm run tauri dev`. Vite is fixed to port 1420. That command needs a display and WebKitGTK 4.1 (`libwebkit2gtk-4.1-dev` and the rest of Tauri's Debian prerequisite list). `npm run dev` by itself has no Tauri backend, so the profile list stays empty.
- Volume listing, format, and flash use Windows disk APIs. On Linux, choosing a layout reaches the card step and `list_volumes` reports that removable-drive listing is Windows-only; no drive is offered. `cargo test` uses disk-image fixtures and must not open a physical drive.
