# Phase 1 hardware checklist

Automated tests do not touch a real SD card. Run this on a spare card before calling the ROMs-card flow done.

## Setup
- Dual-card ArkOS or dArkOS device, OS card already booted once on its own.
- Spare SD card you can erase. Note the drive letter Windows assigns.
- A tiny library folder, for example `gba/test.gba` (a file you own).

## Steps
1. `npm run tauri dev` from the repo root.
2. Pick **ArkOS — ROMs card only**.
3. Confirm fixed disks are absent from the list. Only the spare card appears.
4. If the card is not empty exFAT/FAT, type `FORMAT` and erase it. Confirm the size shown matches the spare card before you do.
5. After prepare, the card root contains `gba`, `snes`, `nes`, `psx`, and `bios`.
6. Point at the tiny library. Preview lists `gba/test.gba` and ignores other extensions.
7. Copy, eject from Windows, insert into the handheld's ROMs slot, boot.
8. The test ROM is visible in the ArkOS GBA list.

## Record the result
Write the device, CFW build, card size, and pass/fail in the commit or PR message. Do not claim hardware success without that note.

## ROCKNIX
The ROCKNIX profile nests folders under `roms/` and labels the volume `SHARE`. Confirm that label and the `roms/<system>` layout against the ROCKNIX build you use before relying on it. This pass does not flash ROCKNIX and does not disarm ArkOS firstboot.
