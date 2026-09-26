# CFW Zero-Touch Card Studio (working title)

Desktop utility to flash handheld custom firmware **and** prepare the games partition on your PC — skipping the usual flash → boot → pull card → copy ROMs dance.

> **Status:** Phase 1 ROMs-card wizard is in progress on `phase-0-1`. Flashing and PC-side firstboot are not implemented.

## Why this exists
Etcher/Rufus only flash. Arch R Flasher adds smart download/panel flashing for one CFW family. Igir sorts ROMs onto a card that’s already ready. **Nothing mainstream does safe PC-side firstboot + ROM load for ArkOS-class single-card setups.**

Naive “copy ROMs right after flash” **deletes your games** when ArkOS firstboot reformats `EASYROMS`. This project treats that as a hard design constraint.

## Docs
- [Market research](docs/MARKET.md)
- [Product definition](docs/PRODUCT.md)
- [Build plan](docs/BUILD_PLAN.md)
- [Grok Build handoff](GROK_BUILD_HANDOFF.md)

## Legal
This tool will never ship ROMs or BIOS files. You point it at files you already own.
DAT files for the optional igir sort (1G1R, region filter, checksum verification)
are also yours to supply — for example from No-Intro's datomatic. igir itself runs
from your PATH or through npx (Node.js).
The optional local-AI tier (Needle) is software, not content: its engine and
weights are Apache-2.0/open artifacts downloaded only with your consent and
verified against compiled-in SHA-256 pins — see `THIRD_PARTY_NOTICES.md`.
A local diagnostics log (`%LOCALAPPDATA%\cfw-card-studio\diagnostics.log`) records
what the app does, including errors; it never leaves your PC.
On launch the app checks the profile feed at `raw.githubusercontent.com` (and
after that, downloads OS images only when you ask it to flash). `CFW_FEED_URL`
overrides the feed URL for development.

## Run the ROMs-card wizard

Requires Node 24, Rust stable, and the Visual Studio C++ build tools (for Tauri on Windows).

```bash
npm install
npm test
npm run tauri dev
```

Phase 1 prepares a **ROMs card only**. It lists removable drives, can format one after you type `FORMAT`, creates the profile's folders, and copies a library you already have. It does not flash firmware and it does not disarm ArkOS firstboot. Copying ROMs onto a freshly flashed single-card ArkOS image is still unsafe — see `docs/MARKET.md`.

Rust tests: `cargo test` in `src-tauri`. Hardware checks: `docs/QA-PHASE1.md`.

## Quick start for Grok Build
See `GROK_BUILD_HANDOFF.md`.
