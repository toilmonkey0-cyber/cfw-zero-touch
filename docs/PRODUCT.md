# Product definition — Zero-Touch CFW Card Studio (working title)

## One-liner
A desktop wizard that flashes handheld CFW **and** prepares the games partition on the PC so the user never does the flash → boot → pull → copy → reinsert shuffle.

## What we build (v1)

### Must have
1. **Device + CFW profile picker** (start with a *narrow* matrix — see below).
2. **Image acquire:** download from profile URLs (GitHub releases) with checksum verify, or pick local `.img` / `.img.xz` / `.img.gz`.
3. **Safe disk target selection** (removable-only, size confirm, type-to-confirm wipe).
4. **Flash** via privileged writer (reuse patterns from Arch R Flasher / Raspberry Pi Imager).
5. **PC-side firstboot (ArkOS-class single-card):**
   - Grow/create ROM partition to fill card (respect optional Linux size reserve).
   - Format exFAT labeled `EASYROMS` (or profile label).
   - Seed folder tree from profile manifest **and/or** extract `roms.tar` from the flashed rootfs (preferred when present).
   - **Neutralize firstboot** so the handheld will not reformat (disable service / replace `firstboot.sh` with no-op per profile recipe).
6. **ROM library port:** user selects local library root; map systems via profile folder schema; copy with progress; optional Igir integration later.
7. **BIOS folder** handling (empty scaffold + copy if present in library).
8. **Post-flight checklist:** “Safe to eject → insert once → play.”

### Explicit non-goals (v1)
- Shipping or linking pirated ROMs/BIOS.
- Wi‑Fi scraping artwork on-device automation.
- Supporting every CFW/device on day one.
- Full EmulationStation theme management.
- Replacing PortMaster.

### v1 device/CFW matrix (recommended)
| Priority | Profile | Why |
| --- | --- | --- |
| P0 | ArkOS / dArkOS single-card (one popular device family, e.g. RG353M or R36S path) | Clearest pain + well-documented firstboot |
| P0 | **ROMs-only card** prep for ArkOS *and* ROCKNIX/JELOS-layout | Lower risk wedge; dual-card users |
| P1 | ArkOS4Clone + DTB selector step | Huge clone market; Arch R Flasher already educated users |
| P2 | ROCKNIX single-card (if ext4 ROM access is solvable cross-platform) | Defer if Windows ext4 is too painful |
| Later | UnofficialOS, Batocera, Knulli, etc. | Profile plugins |

## What “zero-touch” actually means

```
[Select profile] → [Get image] → [Flash] → [Simulate firstboot on PC]
  → [Copy ROMs/BIOS] → [Eject] → [One boot on device]
```

Not:

```
[Flash] → [Copy ROMs onto unexpanded/wrong partition] → [Device firstboot wipes them]
```

## UX outline
1. Welcome + legal (your ROMs only).
2. Profile: CFW + device (+ panel/DTB if required).
3. Storage mode: Single-card OS+ROMs **or** Prepare ROMs card only **or** Flash OS only.
4. Image source: Download / Local file.
5. Select SD target (scary confirm).
6. Optional: point at ROM library + include/exclude systems.
7. Run pipeline with live log (flash → prepare → copy).
8. Done screen.

## Architecture recommendation
- **Tauri 2 + Rust** (matches Arch R Flasher stack; good for privileged disk ops + small footprint).
- Or **Electron** if team velocity > native — acceptable but heavier.
- Core crates/modules:
  - `profiles/` — JSON/YAML manifests (folder maps, image URLs, firstboot recipe)
  - `imager` — decompress + write + verify
  - `partition` — GPT/MBR surgery, exFAT format (platform-specific)
  - `seed` — extract roms.tar / apply folder manifest
  - `firstboot_disarm` — profile-specific file patches on BOOT/root
  - `romcopy` — parallel copy + skip unchanged
  - `ui` — wizard

## Success metrics
- Time from blank card → playable (user library present) under ~N minutes vs guide baseline.
- Zero support tickets of “my ROMs disappeared after first boot” for cards prepared by the app.
- Profile coverage growth without rewriting the wizard.

