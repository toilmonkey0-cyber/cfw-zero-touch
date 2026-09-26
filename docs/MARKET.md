# Market research — Zero-Touch CFW Auto-Flasher

**Date:** 2026-09-22  
**Verdict:** **Partial–yes gap.** Pain is real and widely documented. No product fully owns “flash → prepare ROMs partition safely → copy library” for the popular ArkOS-class CFWs. The *naive* product idea (copy ROMs immediately after flash) is **technically wrong** for single-card ArkOS/dArkOS and must be reframed as **PC-side firstboot**.

---

## 1. The pain (documented)

Canonical ArkOS flow (wiki + Retro Game Corps + Retro Handhelds guides):

1. Download the correct device image (often split archives + xz).
2. Flash with Rufus / balenaEtcher / Pi Imager / `dd`.
3. Often: eject/reinsert, run DTB selector or overwrite DTB (clones / R35S/R36S).
4. Insert into handheld, **first boot** expands partitions (can take a long time; device reboots).
5. Pull card back to PC.
6. Copy ROMs/BIOS into CFW-specific folders on `EASYROMS` (or dual-card ROM card).
7. Reinsert and scrape/play.

Sources:
- https://github.com/christianhaitian/arkos/wiki (EASYROMS exFAT; **do not manually expand EASYROMS before first boot** or boot can hang)
- https://retrogamecorps.com/2023/03/27/arkos-starter-guide/
- https://retrohandhelds.gg/installing-arkos4clones-on-the-xf35h-and-xf40h/ (explicit: after OS is up, reinsert card and drag ROMs into Easyroms)
- ArkOS4Clone OTA even tells users to **put the card back in the PC** after device power-off (https://github.com/lcdyk0517/arkos4clone/releases)

Community evidence of confusion (folder schemas differ by CFW; dual vs single card):
- https://www.reddit.com/r/RG353V/comments/198y5cy/arkos_and_jelos_rom_file_structure/
- https://www.reddit.com/r/SBCGaming/comments/185y0tk/need_help_setting_up_r35s_on_single_sd_card/

**Buyer personas**
| Persona | Need | Willingness to pay |
| --- | --- | --- |
| Guide-follower / gift setup | One wizard, don’t brick grandma’s R36S | High for $10–30 one-time or tip-jar |
| Enthusiast / dual-boot tinkerer | Multi-CFW profiles, reproducible cards | Medium; will use free if good |
| Content creators | Fewer support comments under install videos | Medium–high (affiliate / sponsor) |
| Market size | Unknown (no reliable public unit counts here) | SBC handheld boom is real; exact TAM unknown — do not invent |

---

## 2. Competitive landscape

| Tool | Flash | Device/panel smarts | Partition expand on PC | ROM folder schema | ROM library copy | Gap vs idea |
| --- | --- | --- | --- | --- | --- | --- |
| Rufus / Win32DiskImager | Yes | No | No | No | No | Flash only |
| balenaEtcher | Yes | No | No | No | No | Flash only; ArkOS guides often prefer Rufus |
| Raspberry Pi Imager | Yes | Custom OS list possible | No | No | No | Flash only |
| **Arch R Flasher** (Tauri) | Yes | Yes (R36S original/clone/soysauce + panel DTBO) | No | No | No | Closest UX for *one* CFW family; **stops after flash** |
| XNL R36 Linux Partition Sizer | No | R36-focused | Resizes **Linux** partition pre-boot | No | No | Partial expand; not ROM seed |
| Igir | No | No | No | Yes (`{rocknix}` / `{jelos}` / handheld tokens) | Sort/copy **onto an already-ready volume** | Perfect ROM sorter; assumes card already prepared |
| Manual MiniTool / GParted | No | No | Manual | Manual | Manual | Expert escape hatch |

**Arch R Flasher** (https://github.com/archr-linux/archr-flasher): download + SHA256 + flash + inject panel overlay. Explicitly does **not** prepare EASYROMS or copy games. Proves demand for “smarter than Etcher” desktop apps in this niche.

**Igir** (https://igir.io): industry-grade ROM manager; ROCKNIX/JELOS folder tokens exist. Complementary, not competitive — integrate or shell out.

---

## 3. Technical landmine (must drive product design)

ArkOS/dArkOS **firstboot**:
- Grows root / recreates ROM partition
- `mkfs.exfat` labels `EASYROMS`
- Extracts `roms.tar` (folder tree, PortMaster bits, configs)
- Disables `firstboot.service`

Community analysis (SBCGaming): anything copied to EASYROMS **before** first boot is **wiped** when firstboot formats and extracts. Manually expanding without neutralizing firstboot still fails or hangs (official wiki warning).

**Implication:** “Zero-touch” ≠ “copy ROMs after Etcher.”  
Zero-touch = **perform firstboot equivalence on the PC**, then copy ROMs, then boot a card that will **not** re-format the games partition.

Dual-SD / ROMs-only card is an easier wedge: FAT/exFAT games card + CFW folder map + Igir-like copy, while OS card flashes normally. Still valuable; less risky.

---

## 4. Market gap verdict

| Claim | Assessment |
| --- | --- |
| Gap for better flash UX | Partially filled by Arch R Flasher (single ecosystem) |
| Gap for CFW-aware ROM folder creation + library port **without device shuffle** | **Open** — no mainstream tool owns ArkOS-class PC-side firstboot + ROM load |
| Gap for “dropdown any CFW” | Wide open but **maintenance-heavy**; start narrow |
| Naive copy-after-flash | **Anti-feature** for ArkOS single-card |

**Positioning:** “The flasher that finishes the job Etcher starts” / “PC-side first boot for handheld CFW.”

---

## 5. Risks

- **Brick / no-boot:** wrong image, wrong DTB, incomplete firstboot simulation
- **Maintenance:** per-CFW/per-device profiles drift as CFWs fork (JELOS → ROCKNIX / UnofficialOS; ArkOS → dArkOS / ArkOS4Clone)
- **Legal UX:** never ship ROMs/BIOS; user points at their own library; clear ToS
- **Privileges:** raw disk write on Win/macOS/Linux is hard (Arch R Flasher pattern: privileged helper)
- **Support burden:** clones and panel variants dominate Discord pain

---

## Addendum (deep research pass)

Full write-up: `docs/RESEARCH_DEEP.md` · competitor table: `docs/COMPETITORS.md`.

- **“Triple shuffle”** is product framing, not a found community meme.
- Official (d)ArkOS guides often say **do not use balenaEtcher**; Rufus/Win32DiskImager preferred.
- **dArkOS** has replaced upstream ArkOS naming; community ArkOS forks still dominate many R36-class devices — profile naming should cover both.
- **Igir** has `{rocknix}` / `{jelos}` but **no built-in `{arkos}` token** — ArkOS maps are a product differentiator.
- Dual-SD “Switch to SD2 for ROMs” only creates empty trees on-device; still no library copy.
- Safer public promise: **minimize shuffles** / finish setup on the PC — reserve “never boot to expand” for profiles with proven firstboot disarm.

