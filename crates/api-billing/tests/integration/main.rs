//! `oxy-api-billing`'s integration tests — one binary, per the workspace rule
//! (a new top-level `tests/*.rs` is another full link). Add a case as a `mod`.
//!
//! The first cases moved here from `oxy-app`'s `platform` group with the usage
//! report's surface (admin split, PR 8b). They keep using `oxy-app`'s harness,
//! included by path rather than copied (it owns the per-run template database
//! and the stray-database sweep), so every test gets its own database cloned
//! from a per-run template and `.config/nextest.toml` puts this binary in
//! `db-per-test`. No module here reaches the shared `OXY_DATABASE_URL`;
//! `oxy-app`'s `authz::shared_db_registry` fails the build the day one does
//! without a `serial-db` pin.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod usage_report;
