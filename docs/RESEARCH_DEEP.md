# Zero-Touch CFW Auto-Flasher — Market & Technical Feasibility Research

**Product idea:** Desktop utility where the user selects a CFW (ArkOS / dArkOS, JELOS / ROCKNIX, etc.) → tool downloads the image → flashes SD → generates that CFW’s exact ROM folder structure on the PC → copies the user’s local ROM library — without the usual flash → insert → first-boot expand → pull card → copy ROMs loop.

**Research date:** 2026-09-22 (America/New_York)  
**Method:** WebSearch + WebFetch of primary docs (CFW wikis, Retro Game Corps, Retro Handhelds, XNL, ArchR Flasher, Igir, Reddit threads).  
**Market sizing:** No reliable public TAM/unit numbers found for “CFW flasher” demand; treat as **unknown**. Do not invent figures.

**Important naming note:** As of late 2025 / 2026, christianhaitian’s wiki states **dArkOS has replaced ArkOS** (ArkOS repo kept historical). Community forks (AeolusUX ArkOS-R3XS, ArkOS4Clone, etc.) remain in heavy use on R36-class devices. Product copy should treat **dArkOS + community ArkOS forks + ROCKNIX** as the live targets, with “ArkOS” as the user-facing umbrella term.

---

## 1. MARKET PAIN

### 1.1 Documented multi-step workflow (citations)

Authoritative install flows all describe the same dance:

| Step | What happens | Sources |
|------|----------------|---------|
| 1 | Download device-specific `.img` / `.xz` / split `.7z` | [ArkOS wiki](https://github.com/christianhaitian/arkos/wiki), [dArkOS wiki](https://github.com/christianhaitian/dArkOS/wiki), [RGC ArkOS Starter Guide](https://retrogamecorps.com/2023/03/27/arkos-starter-guide/) |
| 2 | Extract with 7-Zip / Keka | Same |
| 3 | Flash with Rufus / Win32DiskImager / ApplePiBaker / `dd` / Pi Imager — **not** balenaEtcher for (d)ArkOS | [ArkOS wiki](https://github.com/christianhaitian/arkos/wiki) (“DO NOT USE BALENA ETCHER”), [dArkOS wiki](https://github.com/christianhaitian/dArkOS/wiki), [RGC](https://retrogamecorps.com/2023/03/27/arkos-starter-guide/) |
| 4 | Cancel Windows “Format disk?” prompts | [ArkOS wiki](https://github.com/christianhaitian/arkos/wiki) |
| 5 | Optional: swap DTB / boot files for clones / panels | [RGC R35S/R36S box](https://retrogamecorps.com/2023/03/27/arkos-starter-guide/), [Retro Handhelds R36S guide](https://retrohandhelds.gg/r36s-setup-guide/), [ArkOS4Clone](https://github.com/lcdyk0517/arkos4clone) |
| 6 | Insert OS card in TF1 only; **first boot expands partitions** (often ~minutes, may reboot twice) | [ArkOS wiki](https://github.com/christianhaitian/arkos/wiki), [dArkOS wiki](https://github.com/christianhaitian/dArkOS/wiki), [RGC](https://retrogamecorps.com/2023/03/27/arkos-starter-guide/) |
| 7 | Dual-SD: insert blank TF2 → **Options → Advanced → Switch to SD2 for ROMs** (creates folder tree; does **not** copy your library) | [RGC](https://retrogamecorps.com/2023/03/27/arkos-starter-guide/), [ArkOS issue #1214](https://github.com/christianhaitian/arkos/issues/1214) |
| 8 | Proper shutdown → **pull card** → PC → copy ROMs/BIOS into EASYROMS (or SD2) system folders → reinsert | [ArkOS wiki](https://github.com/christianhaitian/arkos/wiki), [RGC “Add ROM files”](https://retrogamecorps.com/2023/03/27/arkos-starter-guide/), [RGC children setup](https://retrogamecorps.com/2024/04/28/setting-up-a-handheld-for-children-or-adult-children/), [ROCKNIX Add Games](https://rocknix.org/play/add-games/) |

**Retro Game Corps** literally sequences: flash → boot/resize → (optional Switch to SD2) → shutdown → eject → add ROMs on PC → reinsert ([guide](https://retrogamecorps.com/2023/03/27/arkos-starter-guide/)).

**Retro Handhelds** similarly: flash CFW → first boot unpack → then “Setting Up ROMs and BIOS” as a separate phase, including format second card / drag-drop / reinsert ([R36S setup](https://retrohandhelds.gg/r36s-setup-guide/)).

**ROCKNIX** dual-card path is also multi-pass: format TF2 → boot once so OS creates `roms/` tree → shutdown → pull TF2 → copy on PC → reinsert ([wiki](https://rocknix.org/play/add-games/)).

### 1.2 How common is the “triple shuffle” complaint?

**Exact phrase “triple shuffle”:** Not found as a widely used community meme/brand in search results. Treat it as a **product framing**, not a documented catchphrase.

**What *is* common (qualitative, not quantified):**

- Endless “how do I install / which image / which DTB / when do I copy ROMs?” threads on r/R36S, r/SBCGaming, device subs ([example: R35S single-card setup confusion](https://www.reddit.com/r/SBCGaming/comments/185y0tk/need_help_setting_up_r35s_on_single_sd_card/), [gift clone confusion](https://www.reddit.com/r/R36S/comments/1nsldiq/got_a_gift_is_it_a_clone_what_to_do_to_it/), [fresh MicroSD install asks](https://www.reddit.com/r/R36S/comments/1cmc7nn/any_guide_on_how_to_install_arkos_in_a_new_microsd/)).
- Guides exist *because* the flow is multi-tool and error-prone (RGC video + written guide; Retro Handhelds; DROIX; XNL “pro” tutorials; YouTube “Ultimate Dual SD” / “Starter Guide” videos).
- First-boot landmines are repeatedly rediscovered: copy-before-expand gets wiped; wrong DTB → black screen; Etcher weirdness; Windows format prompts; Paragon corruption ([ArkOS FAQ](https://github.com/christianhaitian/arkos/wiki/Frequently-Asked-Questions---RG351P)).

**Prevalence metric:** **Unknown.** No survey, upvote census, or support-ticket stats available. Safe claim: pain is **structurally baked into every official guide** and **frequently visible in community Q&A**, especially for gift buyers and R36S clone owners.

### 1.3 Pain summary for product messaging

Users must juggle: correct image × flasher tool × DTB/panel × first-boot rules × CFW-specific folder schema × legal ROM/BIOS sourcing. The SD “shuffle” (PC ↔ handheld ↔ PC) is the most tangible friction for people who already have a ROM library on the PC.

---

## 2. COMPETITORS / ADJACENT TOOLS

See also short table: [`COMPETITORS.md`](./COMPETITORS.md).

### 2.1 Generic SD imagers (flash only)

| Tool | Does | Does **not** |
|------|------|----------------|
| **[Rufus](https://rufus.ie/)** | Windows; DD-mode raw `.img` write; often recommended for (d)ArkOS | No CFW catalog, no DTB picker, no ROM folders, no ROM copy |
| **[balenaEtcher](https://etcher.balena.io/)** | Cross-plat flash + verify; popular UX | Explicitly **discouraged** for (d)ArkOS ([wiki](https://github.com/christianhaitian/arkos/wiki)); no CFW/ROM features |
| **[Raspberry Pi Imager](https://www.raspberrypi.com/software/)** | Cross-plat; “Use custom” OS; safe drive picker | No handheld CFW awareness; no ROM schema/copy |
| **[Win32DiskImager](https://sourceforge.net/projects/win32diskimager/)** | Windows raw write **and** read-back backup | No CFW/ROM features; Windows-only |
| **USB Image Tool / ApplePiBaker / `dd`** | Platform-specific raw write | Same gap |

**Gap vs product idea:** 100% of these stop at “bits on card.”

### 2.2 CFW-/device-adjacent helpers (partial automation)

| Tool | Does | Does **not** |
|------|------|----------------|
| **[XNL R36 Linux Partition Sizer](https://www.teamxnl.com/product/r36-linux-partition-sizer/)** | Post-flash, **pre-first-boot**, edits ArkOS/dArkOS expand scripts to enlarge Linux root by shrinking future EASYROMS; Win + Linux builds | Does **not** expand EASYROMS for early ROM copy; does **not** create ROM trees or copy ROMs; R36S/H focused; not a flasher |
| **[Arch R Flasher](https://github.com/archr-linux/archr-flasher)** (**real**, Tauri 2, GPL-2) | Download Arch R image + SHA256; flash Win/Mac/Linux with privilege escalation; inject panel DTBO for Original/Clone/Soysauce (43 panels); overlay-only tab | **Arch R only** (ROCKNIX-based), **not** ArkOS/dArkOS/JELOS; **no** ROM folder generation; **no** ROM library copy |
| **[ArkOS4Clone](https://github.com/lcdyk0517/arkos4clone) + DTB Analysis Tool** | Clone/panel DTB selection, boot file packs, customization tools | Not an integrated flasher+ROM pipeline; user still uses Rufus/etc. |
| **AeolusUX / ArkOS-R3XS community images** | Device-specific ArkOS builds for R33/R35/R36 | Image project, not a desktop installer |

### 2.3 ROM organization / scraping (ROM side only)

| Tool | Does | Does **not** |
|------|------|----------------|
| **[Igir](https://igir.io/)** | CLI: DAT-driven copy/extract/clean; tokens `{rocknix}` / `{jelos}` (alias), `{onion}`, `{spruce}`, etc. ([tokens](https://igir.io/output/tokens/), [ROCKNIX guide](https://igir.io/usage/handheld/rocknix/)); custom `--output-console-tokens` JSON | **No built-in `{arkos}` token** in published token list; does not flash SD; user must already have a writable ROM partition/mount |
| **[Skyscraper](https://github.com/muldjord/skyscraper)** (Lars Muldjord) | Scrapes artwork/metadata → EmulationStation `gamelist.xml` | No flashing; no CFW install; assumes ROMs already laid out |
| **PortMaster / ThemeMaster** | On-device ports/themes | After CFW is running |

### 2.4 Competitive conclusion

**No shipping tool found** that combines: CFW image download + flash + (safe) partition prep + CFW-accurate empty folder tree + local ROM library copy in one desktop UX.

Closest “shape” precedents:

1. **Arch R Flasher** = flash + hardware overlay UX (architecture reference).  
2. **Igir** = ROM schema mapping (data/model reference).  
3. **XNL Partition Sizer** = pre-first-boot script surgery on ArkOS images (danger + opportunity reference).

---

## 3. TECHNICAL FEASIBILITY / LANDMINES

### 3.1 ArkOS / dArkOS warning: do NOT manually expand EASYROMS before first boot

**Verified on both wikis (identical wording):**

> “DO NOT MANUALLY EXPAND THE EASYROM PARTITION AS THIS WILL BE DONE AT FIRST BOOT OF THIS IMAGE. Manually expanding the partition prior to the first boot of this distro will cause the distro to hang and not complete the boot up process.”  
> — [ArkOS wiki](https://github.com/christianhaitian/arkos/wiki), [dArkOS wiki](https://github.com/christianhaitian/dArkOS/wiki)

**What first boot actually does (community reverse-engineering + reset scripts):**

- Image ships with a small **NTFS** (or similar) games partition labeled for expansion.  
- `firstboot` / `expandtoexfat`-class scripts: grow partition → **mkfs exFAT** → extract bundled `roms.tar` (folder tree + PortMaster bits / configs) → update fstab → disable firstboot ([r/SBCGaming technical comment](https://www.reddit.com/r/SBCGaming/comments/185y0tk/need_help_setting_up_r35s_on_single_sd_card/); ArkOS `reset_firstboot` / dArkOS-family expand scripts on GitHub).  
- **Anything copied onto the games partition before that format is destroyed.**

**Implications for “zero-touch”:**

| Naive idea | Result |
|------------|--------|
| Flash → copy ROMs on PC → first boot | **ROMs wiped** when partition is reformatted |
| Flash → manually expand EASYROMS on PC → first boot | **Boot hang** per official warning |
| Flash → XNL-size Linux → first boot → then copy | **Works**, but still requires first boot + second PC session (not zero-touch) |

### 3.2 Can folders be created on EASYROMS after flash but before first boot if expansion is done on PC (XNL-style)?

**Clarify what XNL does:** XNL **does not** finish EASYROMS expansion for you. It **adjusts the planned Linux vs ROM split** by editing first-boot scripts **before** first boot; the handheld still runs expand/format/tar extract ([XNL product page](https://www.teamxnl.com/product/r36-linux-partition-sizer/)).

**True pre-boot ROM staging would require the tool to *replace* firstboot, not skip it:**

1. Grow/create games partition to fill card.  
2. Format exFAT labeled `EASYROMS`.  
3. Extract the image’s `roms.tar` (or equivalent) so dependency folders (PSP, NDS, etc.) exist — wiki: do not delete those folders ([ArkOS wiki](https://github.com/christianhaitian/arkos/wiki)).  
4. Disable `firstboot.service` / remove hooks so device does not re-wipe.  
5. Then copy user ROMs.

That is **technically feasible on a Linux host** (loop-mount image partitions, `sfdisk`/`parted`, `mkfs.exfat`, tar extract) and **harder on Windows/macOS** (exFAT OK; ext4/btrfs rootfs surgery needs WSL/Linux VM or careful userspace drivers — Paragon is known-bad for ArkOS).  

**Risk:** firstboot scripts evolve (ArkOS → dArkOS → community forks). One wrong disable = boot loop or missing PortMaster scaffolding. This is the core **engineering landmine** for literal zero-touch on **single-card (d)ArkOS**.

### 3.3 Dual-SD setups — is zero-touch easier?

**Yes, for the ROM half; partial for the OS half.**

**ArkOS / dArkOS dual-SD**

- OS card: still needs first boot expansion (and often **no TF2 inserted during first boot** — [rk2023 changelog note](https://github.com/christianhaitian/arkos/blob/main/changelogs/rk2023-changelog)).  
- ROM card: after “Switch to SD2 for ROMs,” structure is created on-device; christianhaitian notes the switch **does not copy** your library ([issue #1214](https://github.com/christianhaitian/arkos/issues/1214)).  
- **PC-side opportunity:** Format TF2 exFAT/FAT32, create ArkOS-style **system folders at card root** (`snes/`, `psx/`, `bios/`, … — not under `/roms`), copy library **without waiting**, then still run Switch-to-SD2 (or configure mounts) so the OS points at SD2. Folder names must match [ArkOS emulators wiki](https://github.com/christianhaitian/arkos/wiki/ArkOS-Emulators-and-Ports-information).

**ROCKNIX / JELOS dual-SD**

- Wiki: insert formatted TF2 → boot → OS creates `roms/` → pull → copy ([Add Games](https://rocknix.org/play/add-games/)).  
- Igir documents writing ROCKNIX layouts from PC ([guide](https://igir.io/usage/handheld/rocknix/)). Pre-creating `roms/<system>/` on TF2 before first insert is **likely workable** (schema is public); confirm per release that empty tree isn’t clobbered.  
- Single-card ROCKNIX often uses **ext4** `games-internal` — painful on Windows/macOS without network transfer ([Igir filesystem note](https://igir.io/usage/handheld/rocknix/)). Dual-SD is the friendly path for a desktop flasher.

**Verdict:** Dual-SD is the **best v1 product path** for “flash OS + prep ROMs card in one session” without fighting firstboot wipe. True single-card zero-touch needs firstboot replication.

### 3.4 CFW-specific folder schemas

| CFW | Typical ROM layout | Notes |
|-----|--------------------|-------|
| **ArkOS / dArkOS** | EASYROMS partition: **system folders at root** (`gb/`, `snes/`, `bios/`, …) | Dual SD2: same at **root of SD2**, not `/roms` ([emulators wiki](https://github.com/christianhaitian/arkos/wiki/ArkOS-Emulators-and-Ports-information); [Reddit path mismatch](https://www.reddit.com/r/RGB30/comments/1de6sw9/is_it_possible_to_share_the_same_roms_folder/)) |
| **JELOS (legacy) / ROCKNIX** | Prefer **`/roms/<system>/`** nesting | `{jelos}` token aliased to `{rocknix}` in Igir ([tokens](https://igir.io/output/tokens/)) |
| **AmberELEC / similar** | Often closer to ArkOS-style trees | Verify per device; don’t assume |
| **muOS / Onion / Spruce / CrossMix** | Different again; Igir has some tokens, not all | Out of scope unless v2 |

**Maintenance burden:** Folder maps drift (new systems, ports, naming). Product needs versioned JSON maps + update channel, ideally generated from each CFW’s `es_systems.cfg` / wiki scrape with human review.

### 3.5 DTB selection for clones (ArkOS4Clone / panels)

R36S-class hardware is **not** one DTB:

- Stock panels vs clone types; wrong DTB → black screen / no boot (common RGC comments / Retro Handhelds screen section).  
- [ArkOS4Clone](https://github.com/lcdyk0517/arkos4clone): DTB Analysis Tool + selectors; community panel packs.  
- [Arch R Flasher](https://github.com/archr-linux/archr-flasher): shows the **right UX pattern** — console family → panel list → inject after flash.  
- Retro Handhelds: identify screen **before** install ([guide](https://retrohandhelds.gg/r36s-setup-guide/)).

Any “zero-touch” ArkOS path for R36 **must** include panel/clone selection or it will brick UX (soft-brick: blank screen).

### 3.6 Cross-platform SD writing privileges

| OS | Privileges / pitfalls |
|----|------------------------|
| **Windows** | Admin / UAC; Win10 1703+ for multi-partition drive letters; Disk Management may need manual letter for EASYROMS ([wiki](https://github.com/christianhaitian/arkos/wiki)); avoid Paragon; cancel format prompts; Arch R Flasher uses elevated startup + PowerShell Storage APIs ([README](https://raw.githubusercontent.com/archr-linux/archr-flasher/main/README.md)) |
| **macOS** | Admin via `osascript`; ApplePiBaker or `dd` to `/dev/rdiskN`; Finder may create `._` junk files (ArkOS has “Remove ._ Files”) |
| **Linux** | `pkexec` / sudo; best platform for partition surgery + ext4; `dd`/`gnome-disks` |

Writing raw images is a solved problem (Etcher/Rufus/Pi Imager/ArchR). **Post-flash multi-partition mount + exFAT + optional ext4 surgery** is the harder cross-plat problem.

---

## 4. MARKET GAP VERDICT

### Verdict: **PARTIAL YES** (strong gap for “guided CFW install + ROM layout”; weak/expensive for literal single-card zero-touch)

**Yes — clear gap exists for:**

- One desktop app that: picks device + CFW + panel → downloads correct image → flashes with the *recommended* backend (not Etcher for dArkOS) → injects DTB → optionally prepares a **second SD** with correct empty folders + copies user ROMs (Igir-like).  
- Gift-buyer / YouTube-follower UX that collapses 4–5 tools and tribal knowledge.

**No / deferred — literal “never insert until ROMs already on single card” for (d)ArkOS:**

- Conflicts with official firstboot expand/format.  
- Requires maintaining firstboot clones per image family.  
- High brick/support cost.

**Buyer personas (qualitative):**

| Persona | Need | Willingness |
|---------|------|-------------|
| **Enthusiast / tinkerer** | Already uses Rufus + Igir; wants polish, multi-CFW maps, DTB UX | Medium — may prefer CLI |
| **Gift buyer / parent** (“set this up for me”) | Maximum hand-holding; dual-SD prep; avoid black screens | **Highest** for a polished GUI ([RGC children guide](https://retrogamecorps.com/2024/04/28/setting-up-a-handheld-for-children-or-adult-children/) exists because this audience is real) |
| **YouTube guide follower** | Does steps once; fails on DTB/Etcher/format prompt | High for “do it for me” app; churn once set up |

**Market size:** **Unknown** (no credible public numbers located for addressable “desktop CFW flasher” users). Handheld volume itself is large on AliExpress/Amazon, but conversion to paid utility is speculative.

---

## 5. RECOMMENDED PRODUCT SCOPE v1

### 5.1 Must-have (v1)

1. **Device + CFW picker** with curated download URLs / checksums (start: **dArkOS official devices** + **AeolusUX / ArkOS-R3XS** + **ROCKNIX** for overlapping Anbernic/Powkiddy/R36 class).  
2. **Flash pipeline** using Rufus-like DD write (or embed `dd`/platform APIs); **default away from Etcher for (d)ArkOS**.  
3. **Panel/DTB injector** for R36 Original vs Clone (reuse ArchR Flasher UX patterns; integrate ArkOS4Clone panel lists where licensed/allowed).  
4. **ROM card wizard (dual-SD first):** format/prep TF2 → generate CFW-accurate folder tree → copy from user-selected library root (optionally shell out to or reimplement Igir-style mapping).  
5. **Explicit legal UX:** “Only copy files you own / have rights to”; no ROM download; no torrent; no “fullset” links.  
6. **Post-flash checklist:** “Boot TF1 alone once → then insert TF2 / Switch to SD2” when firstboot cannot be skipped.  
7. **SHA256/MD5 verify** against published sums.

### 5.2 Later (v1.5+)

- Single-card **firstboot emulation** on Linux (experimental flag).  
- AmberELEC, muOS, Onion, Spruce, Batocera, community forks.  
- Skyscraper / ScreenScraper artwork pass.  
- XNL-style Linux partition size slider.  
- In-app updates of folder-map packs.  
- Save/sync helpers (Syncthing docs only, not required).

### 5.3 Suggested architecture

| Option | Pros | Cons |
|--------|------|------|
| **Tauri 2 (recommended)** | Arch R Flasher already proves flash + privilege escalation + disk UX; small footprint; Rust for FAT/DTB | WebView deps (Win WebView2) |
| **Electron** | Fast UI iteration | Heavier; still need native flash helper |
| **CLI + thin GUI** | Igir-compatible; scriptable for enthusiasts | Gift buyers bounce |

**Recommendation:** **Tauri 2 desktop** + optional **CLI** sharing the same Rust core (flash, maps, copy). Keep CFW folder maps as versioned JSON (Igir custom tokens format is a good starting point).

### 5.4 Honest product positioning

Market as **“CFW Install Studio”** or **“Dual-SD Setup Wizard”**, not absolute “zero-touch single-card never boot.” Promise: *minimize* shuffles; eliminate wrong-image/wrong-DTB; prepare ROMs while OS card first-boots (parallelism) or fully prep ROM card offline.

---

## 6. RISKS

| Risk | Detail | Mitigation |
|------|--------|------------|
| **Legal / ROM UX** | Tool must not facilitate piracy; christianhaitian disclaimer rejects preloaded ROM packs ([dArkOS wiki](https://github.com/christianhaitian/dArkOS/wiki)) | Local-path copy only; ToS; no bundled ROMs/BIOS; optional hash verify against user DATs |
| **Bricking / no-boot** | Wrong image, wrong DTB, interrupted write, cheap SD | Checksums; panel wizard; recommend SanDisk/Samsung; verify write; backup stock via Win32DiskImager read |
| **Wrong image** | RG351P vs MP; clone vs original | Strict device matrix; block known-incompatible combos |
| **First-boot conflicts** | Pre-expanded partitions; TF2 present during first boot; leftover firstboot after PC surgery | Follow wiki rules by default; document experimental modes |
| **Etcher reputation** | Official “do not use” | Don’t use Etcher backend for (d)ArkOS |
| **Maintenance burden** | CFW folder maps, download URLs, DTB lists, firstboot scripts change (ArkOS→dArkOS already) | Map packs as data updates; pin tested image versions; community PR pipeline |
| **Windows partition visibility** | EASYROMS missing drive letter | Detect + guide Disk Management assign |
| **Cross-FS** | ext4 single-card ROCKNIX | Prefer dual-SD; or Samba after first boot |
| **Support load** | Gift buyers generate tickets | Excellent in-app wizard + “what screen do I have?” flow |
| **Competing free stack** | Rufus + Igir + wiki is “good enough” for experts | Win on integration + DTB + dual-SD ROM prep UX |

---

## 7. EVIDENCE-BACKED CONCLUSIONS

1. The painful multi-step CFW setup is **real and universally documented**; the nickname “triple shuffle” is **not** a measured community standard phrase.  
2. **No competitor** currently ships the full product vision; **Arch R Flasher** is the closest architectural cousin (flash + panel), **Igir** the closest ROM-layout cousin.  
3. Literal zero-touch **single-card (d)ArkOS** fights firstboot (expand + **format wipe**); treat as advanced/optional.  
4. **Dual-SD + guided flash + DTB + ROM tree/copy** is the feasible, high-value v1 gap.  
5. Buyer with most pull: **non-expert / gift / guide-follower**, not the terminal-comfortable enthusiast.  
6. Market dollar size: **unknown**.

---

## Primary URL index

- https://github.com/christianhaitian/arkos/wiki  
- https://github.com/christianhaitian/dArkOS/wiki  
- https://retrogamecorps.com/2023/03/27/arkos-starter-guide/  
- https://retrogamecorps.com/2024/04/28/setting-up-a-handheld-for-children-or-adult-children/  
- https://retrohandhelds.gg/r36s-setup-guide/  
- https://rocknix.org/play/add-games/  
- https://igir.io/usage/handheld/rocknix/  
- https://igir.io/output/tokens/  
- https://www.teamxnl.com/product/r36-linux-partition-sizer/  
- https://github.com/archr-linux/archr-flasher  
- https://github.com/lcdyk0517/arkos4clone  
- https://github.com/christianhaitian/arkos/issues/1214  
- https://www.reddit.com/r/SBCGaming/comments/185y0tk/need_help_setting_up_r35s_on_single_sd_card/  
- https://www.reddit.com/r/RGB30/comments/1de6sw9/is_it_possible_to_share_the_same_roms_folder/  
- https://etcher.balena.io/  
