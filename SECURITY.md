# Security policy

Card Studio erases SD cards and writes raw OS images to USB disks. It is designed to fail closed, and its safety properties are treated as product features rather than nice-to-haves. This document describes the trust model, the guards that are enforced in code, and the limits you should know about before relying on it.

## Supported versions

Pre-1.0. Only the `main` branch is supported; fixes land there. There are no maintained release branches yet.

## Reporting a vulnerability

Report suspected vulnerabilities through GitHub's private vulnerability reporting on this repository (Security → Report a vulnerability). Please do not open a public issue for a security problem.

Useful reports include the affected screen or command, the profile id, the card or disk state, whether a card was modified, and the relevant lines from `%LOCALAPPDATA%\cfw-card-studio\diagnostics.log`. The log deliberately contains no ROM file names, so it is safe to attach. The in-app **Save support file** button writes a zip of that log plus version, system, the latest card test, and license status. The identity check in that zip includes the card serial and the partition table id. It removes game names, folder paths, account names, the license key, and email addresses before saving. The app does not upload the zip.

Expect an initial response within about a week. This is a part-time project, and there is no bug bounty.

## Trust model

The app performs privileged or network-touching work, and each kind has its own anchor:

| Operation | Anchor |
| --- | --- |
| Flashing an OS image | The published SHA-256 in the profile, checked on the downloaded artifact before any write, and again by reading the card back after the write |
| Profile updates (the feed) | `main` on this repository, fetched over HTTPS from `raw.githubusercontent.com`; the feed's `releaseUrl` must be `https://` or it is rejected |
| Local-AI engine and weights | SHA-256 pins compiled into `src-tauri/src/needle/manifest.rs`, verified on download; only fetched after you consent |
| Optional license check | `POST https://api.gumroad.com/v2/licenses/verify` over HTTPS with certificate validation. The body is the public product id and the license key. No API token is compiled in. Release builds refuse Gumroad test purchases. The call runs off the UI thread. See [`docs/LICENSING.md`](docs/LICENSING.md) |
| Windows installers | A `v*` tag builds them in GitHub Actions and attaches them to a draft release. You publish the draft. Code signing stays off until `SIGNING_ENABLED` is exactly `true`, and a manual dry run stays unsigned even then. The certificate is read only by the sign step, after the installer exists, from secrets on the `release` environment. Use GitHub-hosted runners. See [`docs/RELEASING.md`](docs/RELEASING.md) |

Because `main` is the profile-feed trust anchor, the repository's branch protection and account security are part of the app's security model, not just project hygiene.

Profile data is validated at load time, not trusted: folder and filename patterns are rejected if they are unsafe, destination paths are validated at plan time and again per copy item, and rejected values fail the profile rather than silently sanitizing it.

## Guards enforced in code

- Only removable media is offered as a target. Disk 0 and the system disk are refused, and a removable disk that reports size 0 is skipped.
- A write to an OS image happens only when: the confirmation is exactly `FLASH`, the target is a non-zero USB disk, the displayed size matches `Get-Disk`, and the image SHA-256 matches.
- A card is only reported ready for game copying when the volume checks pass and, for single-card ArkOS/dArkOS layouts, the first-boot scripts are gone. While `expandtoexfat.sh` or `firstboot.sh` remain, copies are refused, because first boot formats the games partition.
- Formatting requires a removable drive, a named expected size, and the exact word `FORMAT`.
- Copies write to a `.cfwpart` temporary name and rename on completion, so an interrupted copy cannot leave a truncated ROM behind.
- Disk writes require elevation; the app launches the flash helper with a UAC prompt rather than assuming it already has rights.
- The diagnostic log stays on this PC and is never uploaded. There is no telemetry and no analytics. An optional license check sends the product id and license key to Gumroad over HTTPS when you activate, and again to re-check a saved activation. It is not required for backup, restore, card tests, or the disk safeguards. Inference for the optional local-AI tier runs on-device and never contacts the network.

## Accepted residual risk

These are known and intentionally out of scope, documented rather than hidden:

- **Same-user malware can defeat the flash result check.** A process running as the same user can write `RESULT=OK` into the helper's log and make an unverified flash look verified. A real fix requires an out-of-process privileged verifier; the app does not attempt to defend against a compromised user session.
- **A profile's `custom` layout values are validated, not sandboxed.** Bounds are enforced on folders and patterns, but a malicious profile is a limited-confusion risk rather than a code-execution vector. Profiles you load yourself are trusted to the same degree as the app.
- **The DAT sorting path shells out to your `igir`.** The app spawns `igir` (from `PATH` or `npx.cmd`) and validates the folders it is given, but the tool itself is a third-party dependency you supply.
- **Nothing prevents you from confirming a write to a card that matters.** The confirmations are deliberately literal, and the app does not try to outsmart a determined user who types `FLASH` on the wrong card. The size and disk checks are the safety net; read them.
- **The license check is a speed bump.** The app is open source. The cache HMAC is computed from a machine id and a label that ships in the source, so a determined person can forge a cache or skip the check. Card-safety commands do not consult it. Details and the three-computer soft limit are in [`docs/LICENSING.md`](docs/LICENSING.md).
- **Installers are unsigned until signing is turned on.** SmartScreen will warn. Check `SHA256SUMS` before you run a download. [`docs/RELEASING.md`](docs/RELEASING.md) is the procedure for adding a certificate later.

## Out of scope

- Physical damage, card wear, or data loss on cards that were already failing.
- Firmware or images produced by third parties; the app verifies images against published checksums, not authorship.
- Unsupported platforms. Flashing is Windows-only, and a non-Windows write path is not implemented.
