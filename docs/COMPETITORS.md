# Competitors & Adjacent Tools — Quick Table

Research date: 2026-09-22. Focus: tools a user might reach for when installing CFW + ROMs on retro handhelds.

| Tool | Platform | Flash CFW image | Download / catalog CFW | DTB / panel inject | Expand / resize partitions | Generate CFW ROM folders | Copy local ROM library | Notes / gap vs zero-touch product |
|------|----------|-----------------|------------------------|--------------------|----------------------------|--------------------------|------------------------|-----------------------------------|
| [Rufus](https://rufus.ie/) | Windows | Yes (DD mode) | No | No | No | No | No | Often recommended for (d)ArkOS; flash-only |
| [balenaEtcher](https://etcher.balena.io/) | Win / Mac / Linux | Yes (+ verify) | No | No | No | No | No | Popular UX; **officially discouraged** for (d)ArkOS |
| [Raspberry Pi Imager](https://www.raspberrypi.com/software/) | Win / Mac / Linux | Yes (“Use custom”) | Pi OS catalog only | No | Pi-specific customization only | No | No | Safe drive picker; no handheld CFW logic |
| [Win32 Disk Imager](https://sourceforge.net/projects/win32diskimager/) | Windows | Yes | No | No | No | No | No | Also **reads** card → `.img` backup |
| USB Image Tool / ApplePi-Baker / `dd` | Win / Mac / Linux | Yes | No | No | No | No | No | Same flash-only class |
| [XNL R36 Linux Partition Sizer](https://www.teamxnl.com/product/r36-linux-partition-sizer/) | Windows, Linux | No | No | No | **Pre-first-boot** edits ArkOS/dArkOS scripts to enlarge **Linux** (shrink future EASYROMS) | No | No | Must run after flash, before first boot; does **not** enable early ROM copy |
| [Arch R Flasher](https://github.com/archr-linux/archr-flasher) | Win / Mac / Linux (Tauri 2) | Yes | Yes (Arch R GitHub + SHA256) | Yes (43 panels / DTBO) | No | No | No | **Real** project; Arch R only; best UX precedent for flash+panel; **no ROM pipeline** |
| [ArkOS4Clone](https://github.com/lcdyk0517/arkos4clone) + DTB tools | Web / desktop helpers | No (user still flashes) | Clone image guidance | Yes (DTB analysis / selector) | No | No | No | Solves clone/panel confusion; not an installer |
| AeolusUX / ArkOS-R3XS / community images | N/A (images) | N/A | Releases pages | Bundled / manual DTB | First-boot on device | On-device after expand | Manual | Image projects, not desktop apps |
| [Igir](https://igir.io/) | Win / Mac / Linux (CLI Node) | No | No | No | No | Yes (`{rocknix}` / `{jelos}` alias, Onion, Spruce, …; **no built-in `{arkos}`**) | Yes (DAT-driven) | Best ROM-layout engine; requires already-mounted ROM volume |
| [Skyscraper](https://github.com/muldjord/skyscraper) | Primarily Linux CLI | No | No | No | No | No (assumes layout) | No (scrapes media) | Artwork / `gamelist.xml` only |
| PortMaster / ThemeMaster | On-device | No | No | No | No | No | No | After CFW is running |
| Samba / SSH / FileBrowser (in CFW) | Network | No | No | No | No | Folders exist after first boot | Manual upload | Avoids second physical shuffle **after** Wi‑Fi works; still needs first boot |

## Gap summary

| Capability | Covered by existing tools? |
|------------|----------------------------|
| Flash alone | Yes — many |
| Flash + panel/DTB | Partial — Arch R Flasher (one CFW family); ArkOS4Clone (manual) |
| Pre-boot Linux size tweak | Partial — XNL (R36 ArkOS/dArkOS) |
| CFW-accurate ROM folder generation | Partial — Igir (not ArkOS-native token) |
| Local ROM library copy into CFW schema | Partial — Igir / manual drag-drop |
| **Flash + DTB + folder gen + ROM copy in one desktop UX** | **No product found** |
| **True single-card (d)ArkOS zero-touch (skip device first boot)** | **No — conflicts with official expand/format firstboot** |

## Closest analogues (for product design)

1. **Arch R Flasher** → architecture & privilege-escalation UX.  
2. **Igir** → ROM schema data model (extend with ArkOS/dArkOS maps).  
3. **XNL Partition Sizer** → proof that pre-first-boot script edits are viable (and limited).
