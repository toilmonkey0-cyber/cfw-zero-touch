---
name: verify
description: Use before opening or updating a pull request, and before claiming that checks passed. Run cargo fmt --check, cargo clippy -D warnings, cargo test, and npm test, paste the output into the PR, and add a Hardware note for disk units.
alwaysApply: true
---

# Verify

Before you open or update a pull request, run `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, and `npm test`. Paste the full output into the pull request description.

From `src-tauri`:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

From the repo root:

```bash
npm test
```

`cargo clippy -D warnings` is passed to the compiler after `--`. CI runs `cargo clippy --all-targets -- -D warnings` in `.github/workflows/ci.yml`. Use that command so the pasted output matches CI.

For a disk unit, add a Hardware note to the pull request. A disk unit changes flashing, formatting, volume detection, firstboot, or `src-tauri/src/bin/`. Name the device, the CFW, the card size, and what you checked on a real card. If you did not run hardware, say that in the Hardware note.

Tests use disk-image fixtures and never real disks.
