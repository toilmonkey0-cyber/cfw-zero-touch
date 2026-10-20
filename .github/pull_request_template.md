## Unit

One unit per pull request. Link the unit issue. Its acceptance criteria are the bar for this pull request:

- Unit issue:

## Verification

Paste the full output of `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, and `npm test` (see `.grok/skills/verify/SKILL.md`).

```text

```

## Hardware note

Required for a disk unit (flash, format, volume, firstboot, or `src-tauri/src/bin/`). For any other unit, write "Not a disk unit."

- Device and CFW:
- Card size:
- What was checked on a real card:

If this pull request edits `src-tauri/src/flash.rs`, `src-tauri/src/bin/cfw-flash.rs`, or `src-tauri/src/volume.rs`, add the `disk-safety` label.
