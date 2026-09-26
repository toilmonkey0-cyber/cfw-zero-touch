# Diagnostics log — design

Date: 2026-09-25
Status: approved in chat
Phase: BUILD_PLAN Phase 4 (third sub-project; replaces cloud crash reporting)

## Goal

A local, user-inspectable record of what Card Studio did — especially around
the dangerous moments (flash, format, copy, stage) and every failure — so
problems can be understood without a debugger and without any telemetry.

## Behavior

- File: `%LOCALAPPDATA%\cfw-card-studio\diagnostics.log` (store root via
  `store::store_root()`, so `CFW_STUDIO_DATA` overrides it for tests).
- One line per event: `YYYY-MM-DDTHH:MM:SSZ LEVEL event detail`. Timestamps
  are UTC, hand-rolled (civil-from-days algorithm), no new dependencies.
- Rotation: when the file reaches 1 MiB it is renamed `diagnostics.old` and a
  fresh log starts. One generation is kept.
- Writes are best-effort and guarded by a process-wide mutex; a logging
  failure never fails the operation being logged.
- Logged events (INFO unless noted):
  - `app_start` with the running version
  - `prepare_start` / `prepare_done` / `prepare_failed` (volume letter, label, size)
  - `flash_start` / `flash_done` / `flash_failed` (disk number, profile)
  - `stage_start` / `stage_done` / `stage_failed` (profile, systems, file count)
  - `copy_start` / `copy_done` / `copy_failed` (file count, bytes — never file names)
  - `feed_check` (status or offline reason) and `feed_applied` (applied/skipped)
  - `firstboot_refused` (WARN — the armed-firstboot gate fired) with reason
  - `panic` (ERROR) via a panic hook installed at startup
- No ROM or BIOS file names are ever logged; only counts, sizes, letters.

## UI

- Footer shows the log path and an "Open diagnostics log" button that opens
  the file in the default editor (opener plugin, `openPath`; capability
  `opener:allow-open-path` added).

## Testing

- Unit tests (offline, `CFW_STUDIO_DATA` override): timestamp formatting
  against known epochs (epoch zero, a leap day, a 10-digit epoch), append of
  two lines, rotation creating `diagnostics.old`.
- Live: drive prepare → preview on the EASYROMS test card and read the log
  back; footer button opens the file.

## Non-goals

- Any network transmission; log levels/config UI; log search/filtering.
