# Grok Build handoff

## Prerequisites
1. Install Grok Build CLI: https://docs.x.ai/build/overview  
   ```bash
   curl -fsSL https://x.ai/cli/install.sh | bash
   # Windows: irm https://x.ai/cli/install.ps1 | iex
   ```
2. Copy this folder into a git repo (or `git init` here).
3. From the repo root:
   ```bash
   grok inspect    # should list AGENTS.md + docs
   grok
   ```
4. Enter **Plan Mode** (`/plan` or Shift+Tab) before large builds.

## Kickoff prompt (paste into Grok Build Plan Mode)

```text
Read AGENTS.md, docs/MARKET.md, docs/PRODUCT.md, docs/BUILD_PLAN.md, and specs/profile.schema.json.

Goal: scaffold and implement Phase 0 + Phase 1 of CFW Zero-Touch Card Studio.

Phase 0: Tauri 2 app skeleton, CI stubs, README disclaimer, load profiles from /profiles.
Phase 1: "ROMs card only" wizard — select volume, create CFW folder schema from profile, copy user ROM library with progress. No flashing yet.

Constraints:
- Do not implement single-card ArkOS firstboot yet (Phase 3).
- Do not download or ship ROMs/BIOS.
- Follow profile.schema.json; include the example profile.
- Prefer small vertical slice with tests for folder mapping.

Produce a plan for approval, then implement after I approve.
```

## Follow-up prompt (Phase 2 — after Phase 1 ships)

```text
Implement Phase 2 flash pipeline for one profile's image download + decompress + removable-disk flash + verify. Study public Arch R Flasher behavior as inspiration (download/verify/flash) but do not vendor their code without license review. Keep ROMs-card flow working.
```

## Follow-up prompt (Phase 3 — the moat)

```text
Implement PC-side firstboot for one ArkOS/dArkOS profile per docs/PRODUCT.md and docs/MARKET.md landmine section. Extract roms.tar, format EASYROMS, disarm firstboot so the device will not wipe the partition. Add fixture-based tests and a hardware QA checklist. Use /plan first.
```

## How this repo is meant to be used
| You | Grok Build |
| --- | --- |
| Approve plans, run hardware QA on spare SD | Scaffold, code, tests, refactors |
| Decide profile priority / rename product | Implement against docs |

## Deliverables checklist for “ready to build”
- [x] Market gap research (`docs/MARKET.md`)
- [x] Exact product scope (`docs/PRODUCT.md`)
- [x] Phased build plan (`docs/BUILD_PLAN.md`)
- [x] Profile schema + example
- [x] AGENTS.md for agent conventions
- [x] Kickoff prompts (this file)
