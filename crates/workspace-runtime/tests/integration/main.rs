//! `oxy-workspace-runtime`'s integration group: the database-backed tests that
//! need the product graph, consolidated here as `oxy-app` is deleted
//! (decompose R, 2026-11-24). This crate depends on nearly everything those
//! tests touch, and nothing it takes as a dev-dependency depends back on it.
//!
//! One binary for the whole group. Add a case as a `mod` here rather than a new
//! `tests/*.rs`: a new top-level test file adds a whole link to every full run.
//!
//! What is here so far, and where it came from:
//!
//! * The Airhouse tests (decompose R-a), `oxy-app`'s `tests/airhouse` binary:
//!   the credential broker, workspace lifecycle, the provisioner, and the
//!   connector pool. The broker, lifecycle and provisioner tests need the
//!   airhouse tables on top of the central schema,
//!   [`common::Schema::CentralAirhouse`]; the pool tests take a database of
//!   their own the same way.
//! * `sentry_surface_gate` (decompose R-a), the two tests out of `oxy-app`'s
//!   `routing::sentry_hub_scan` that drive the real custom-app gate
//!   (`enter_semantic_boundary`). In-process; they open no database.
//!
//! Every database-backed test here gets its own database from a per-run
//! template ([`common::fresh_db`]), so this binary sits in the `db-per-test`
//! group (`max-threads = 4`) in `.config/nextest.toml`, not the
//! fully-serialized `serial-db` group. A module that reaches the *shared*
//! `OXY_DATABASE_URL` instead (`establish_connection()`, `api_router(..)`)
//! needs a `serial-db` pin; `oxy-app`'s `authz::shared_db_registry` finds this
//! group through its harness include and fails the build if one is missing.
//!
//! Anything folded in here that touches cwd, environment variables or another
//! process-global needs `#[serial_test::serial]`: a guard private to one file
//! excludes nothing once the files share a binary.

// A test that drives an agentic-http route and the notice it sends in one async
// body needs a type-layout depth past the default 128, as `oxy-app`'s
// `platform` binary did; kept from that group's header for the tests that
// follow it here.
#![recursion_limit = "256"]

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

mod airhouse_broker;
mod airhouse_broker_preview;
mod airhouse_lifecycle;
mod airhouse_pool_ceiling;
mod airhouse_pool_expiry;
mod airhouse_pool_poison;
mod airhouse_pool_reuse;
mod airhouse_provisioner;
mod sentry_surface_gate;
