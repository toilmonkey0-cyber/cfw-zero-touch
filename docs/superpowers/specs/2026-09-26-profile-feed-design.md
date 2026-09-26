# Profile pack feed + update checker — design

Date: 2026-09-26
Status: approved in chat; pending spec review
Phase: BUILD_PLAN Phase 4 (first sub-project)

## Context

Card Studio ships profiles as JSON files inside the repo. Each profile pins an
OS image URL and SHA-256, so every upstream dArkOS/ROCKNIX release makes the
shipped profiles stale until the app itself is rebuilt. The repo
(toilmonkey0-cyber/cfw-zero-touch, currently private, to be made public)
already hosts the profile files, and the flash pipeline already ships an HTTP
client (ureq). Profile loading today reads a compile-time repo path, which
breaks for installed builds and leaves no writable home for updates.

## Goals

- Profiles update without an app release, via a feed the app fetches itself.
- The app can tell the user when a newer build of itself exists.
- Updating is always an explicit user action with a visible result.
- A missing network changes nothing: local profiles always load.

## Non-goals

- Binary auto-update or the Tauri updater plugin.
- Feed signing (trust model is the HTTPS origin plus the repo).
- Settings UI beyond the buttons described here.
- igir backend, DTB/panel step, diagnostics log (deferred Phase 4 items).

## Feed format

One file, `profiles-feed.json`, at the repo root. Default URL:
`https://raw.githubusercontent.com/toilmonkey0-cyber/cfw-zero-touch/main/profiles-feed.json`,
overridable with the `CFW_FEED_URL` environment variable (tests, local
servers).

```json
{
  "feedVersion": 1,
  "app": {
    "version": "0.1.0",
    "releaseUrl": "https://github.com/toilmonkey0-cyber/cfw-zero-touch/releases"
  },
  "profiles": [
    { "id": "darkos-rgb10x-os", "version": 1, "...": "full existing profile schema" }
  ]
}
```

- `feedVersion` must be `1`; any other value is an unsupported feed and is
  rejected whole.
- Each entry in `profiles` uses the existing profile schema plus an optional
  integer `version` (default `1`) added to `specs/profile.schema.json`.
- An update is: same `id` with remote `version` greater than local, or an `id`
  the local store does not have (an addition).
- The feed's `app.version` is compared with the running version
  (`CARGO_PKG_VERSION`); newer means "a build is available", linking to
  `app.releaseUrl` via the opener plugin.

## Runtime profile store

- Location: `%LOCALAPPDATA%\cfw-card-studio\profiles` (created on demand).
  `CFW_STUDIO_DATA` overrides the store root for tests.
- Seeding: when the runtime store is missing a profile the seed source has,
  the seed file is copied in. Seed source is the repo `profiles/` directory in
  dev builds and the Tauri bundle resources (`profiles/*`, `specs/*` added to
  `bundle.resources`) in packaged builds; `CFW_STUDIO_ROOT` overrides the seed
  source for tests. Seeding never overwrites an existing runtime file.
- All loading (`list_profiles`, `profile_by_id`) reads the runtime store.
- Apply writes entries into the runtime store as `<id>.json`.

## Trust and safety

- Feed is fetched over HTTPS from the same repo a user would clone; the feed
  inherits the repo's trust.
- Every feed entry is validated against `specs/profile.schema.json` before
  anything is written. An invalid entry or unsupported `feedVersion` aborts
  the apply with no writes (write-then-swap per file is unnecessary because
  apply is all-or-nothing).
- A hostile feed cannot make the app write unverified bytes to a card:
  flashing verifies each image's SHA-256 (parts and unpacked image), requires
  the FLASH confirmation, and refuses disk 0; ROM copies remain behind the
  armed-firstboot gate and the volume gates.
- No user data is sent anywhere; the feed fetch is a plain GET.

## Backend components

- `src-tauri/src/feed.rs`:
  - `ProfileFeed`, `FeedApp` structs (`Deserialize`, `feedVersion` checked).
  - `parse_feed(text, validator) -> ProfileFeed` — validates each profile
    entry against the schema; rejects unknown `feedVersion`.
  - `diff_feed(local: &[Profile], feed: &ProfileFeed) -> FeedDiff` — lists
    updates (id, local/remote versions), additions, and `app_update: Option`.
  - `apply_feed(store_dir, feed, local) -> ApplyReport` — writes only entries
    that are newer or new; existing equal-or-older entries are untouched.
  - Version comparison for `app.version` is semver-aware on `major.minor.patch`
    with a plain string fallback.
- Fetch goes through an injectable `fetch` closure (ureq with a timeout in
  production), the same seam `flash::ensure_image` uses, so tests stay offline.
- `lib.rs` commands:
  - `check_profile_feed() -> FeedCheckView` — fetch, parse, diff; soft-fails
    with a reason string when offline or the feed is invalid.
  - `apply_profile_feed() -> ApplyReportView` — fetch, parse, diff, apply;
    hard-fails with the reason.

## UI

- Profile screen gains a status block:
  - idle: "Checking for profile updates…" during the startup check.
  - up to date: "Profiles are up to date." with a "Check again" button.
  - updates: "N profile updates available." with an "Update profiles" button;
    after apply, "Updated N profiles." and the profile list re-renders from
    the runtime store.
  - offline/invalid: "Could not check for updates: <reason>." with "Check
    again".
- Footer: "Card Studio <version>". When the feed's `app.version` is newer, a
  line "Card Studio <new> is available" with an "Open the releases page"
  button (opener plugin).
- Startup performs one silent check that never blocks profile rendering.

## Initial feed and anti-drift

- `profiles-feed.json` is generated from the six shipped profiles at
  `version: 1`, so a fresh install checks as "up to date".
- A Rust test asserts the checked-in feed matches the repo `profiles/`
  directory (same ids, same content, versions >= 1), so the feed cannot rot
  when profiles change. CI (existing workflow) runs it.

## Testing

- Unit/integration on fixtures (no network):
  - feed parse: valid, unknown feedVersion rejected, schema-invalid entry
    rejected, malformed JSON rejected.
  - diff: newer remote = update, equal = none, older = none, unknown id =
    addition, app version newer/equal/older.
  - apply on a temp store: writes updates and additions, leaves others, is
    all-or-nothing on an invalid feed.
  - seeding: empty runtime store fills from seed source; existing files are
    not overwritten.
  - drift: generated-from-profiles property test against the checked-in feed.
- UI: drive the dev app with `CFW_FEED_URL` pointed at a local fixture server
  (up-to-date, update-available, offline), verifying banners, buttons, and a
  real apply re-rendering the profile list.

## Open items

- The user flips the repo to public; then the default URL is exercised for
  real (expect HTTP 200 and "up to date" on first run).
- Commit author names/emails become public with the repo; accepted.
- `main` currently lacks the phase-0-1 work; the feed URL targets `main`, so
  the branch must be merged (or the URL pinned to a ref that has the feed)
  before the default URL serves anything.
