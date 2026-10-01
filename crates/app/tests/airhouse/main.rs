//! Airhouse tests — the credential broker, workspace lifecycle, the
//! provisioner, and the connector pool.
//!
//! One binary for the whole domain; see `tests/authz/main.rs` for why. Add a
//! case as a `mod` here rather than a new `tests/*.rs`.
//!
//! Every test here is database-backed and needs the airhouse tables on top of
//! the central schema — [`common::Schema::CentralAirhouse`]. Each gets its own
//! database from a per-run template, so this binary sits in the `db-per-test`
//! group (`max-threads = 4`) in `.config/nextest.toml`, not the fully-serialized
//! `serial-db` group.

#[path = "../common/mod.rs"]
mod common;

mod airhouse_broker;
mod airhouse_broker_preview;
mod airhouse_lifecycle;
mod airhouse_pool_poison;
mod airhouse_provisioner;
