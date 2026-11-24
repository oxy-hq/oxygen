//! The `oxy-api-workspaces` integration group — one binary, like every crate's
//! `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a `mod`
//! here rather than a new top-level `tests/*.rs`.
//!
//! It reuses `oxy-app`'s per-test harness through the `#[path]` below — the
//! same file the `oxy-app` groups include — so a test that opens a database
//! gets its own, cloned from a per-run template. That puts `workspace_list` in
//! nextest's `db-per-test` group (`.config/nextest.toml`); the other two open
//! no database.

// `meta_canary` awaits a handler whose future nests deeper than rustc's
// default 128 when its layout is computed ("queries overflow the depth
// limit", depth 130). The future is the handler's own; the limit is only how
// far the compiler will follow it. (It came with the test from `oxy-app`'s
// `routing` group, 2026-11-16.)
#![recursion_limit = "256"]

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

// Rehomed from `oxy-app`'s `routing` group (`fleet_canary`) with the `/meta`
// handler it drives (2026-11-16). No database; serialised on the probe's
// process-global flag.
mod meta_canary;
// Rehomed from `oxy-app`'s `platform` group with the details builders it
// drives (2026-11-16). No database.
mod workspace_details_fields;
// The list half of `oxy-api-tenancy`'s `org_default_workspace`, rehomed with
// `list_workspaces` (2026-11-16) and seeded by direct rows. Its own database.
mod workspace_list;
