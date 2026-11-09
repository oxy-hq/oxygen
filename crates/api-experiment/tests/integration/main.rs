//! The `oxy-api-experiment` integration group — one binary, like every crate's
//! `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a `mod` here
//! rather than a new top-level `tests/*.rs`.
//!
//! These moved out of `oxy-app`'s `platform` group with the code they test:
//! `oxy-app` cannot depend on this crate, so its test binaries can no longer
//! name `oxy_api_experiment::…`.
//!
//! The database-backed modules reuse `oxy-app`'s per-test harness through the
//! `#[path]` below — the same file the `oxy-app` groups include — so every test
//! gets its own database cloned from a per-run template. That puts this binary
//! in nextest's `db-per-test` group (`.config/nextest.toml`), exactly as the
//! `platform` binary was, with two carve-outs: `route_roles` opens no database,
//! and `simulation_router` builds `api_router(..)`, which runs the unscoped
//! `cleanup_stale_runs` on the ambient `OXY_DATABASE_URL`, so it stays in
//! `serial-db` as it did under `platform`.
//!
//! `experiment_pricing` and `fixture_layer` `set_current_dir` — the fixture's
//! view SQL reads its CSVs by relative path, and cwd is process-global — so
//! every test that does is `#[serial_test::serial]`, whose lock is global to
//! this binary.

// The experiment estimate tests drive the executor and its warehouse reads in
// one async body, which needs a type-layout depth past the default 128 — the
// same limit, for the same reason, as this crate's own lib.
#![recursion_limit = "256"]

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod experiment_pricing;
mod experiment_results;
mod experiment_routes;
mod experiment_runner;
mod fixture_layer;
mod route_roles;
mod simulation_lifecycle;
mod simulation_router;
mod simulation_routes;
