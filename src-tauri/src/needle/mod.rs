//! Needle engine artifacts: compiled-in pins and verified acquisition.
//!
//! PR 1 of the Needle integration design
//! (`docs/superpowers/specs/2026-09-26-needle-integration-design.md`):
//! a pinned manifest of everything Needle the app can acquire, and an
//! acquire path that reuses the flash pipeline's `.partial`/verify/rename
//! discipline. The engine archive and header are build-time inputs for the
//! `cfw-embed` helper (PR 2); only the weights and the Phase 2 serve engine
//! are runtime downloads. Nothing here runs native code or touches the
//! network on its own.

pub mod acquire;
pub mod client;
pub mod doctor;
pub mod embed_client;
pub mod index;
pub mod manifest;
pub mod serve;
pub mod sort;
pub mod tags;
