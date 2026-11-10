//! `oxy-api-notifications`' integration tests — one binary, per the workspace
//! rule (add a case as a `mod` here, never a new top-level `tests/*.rs`).
//!
//! These moved out of `oxy-app`'s `tests/platform/` (and `slack_notify_dm` out of
//! `tests/slack/`) with the notifications surface. The DB-backed ones keep using
//! `oxy-app`'s harness, included by path rather than copied: it owns the per-run
//! template database and the stray-database sweep, and a second copy would drift
//! from both. Like the oxy-app binaries it serves, it requires nextest's
//! process-per-test mode; the `db-per-test` group in `.config/nextest.toml`
//! covers this binary, and `slack_notify_dm` (the shared database) sits in
//! `serial-db`.
//!
//! The two tests that need this surface *and* the custom-app one
//! (`console_test_notification`, `notification_function_action`) are
//! `oxy-server`'s: a surface crate may not depend on another, not even for its
//! tests.

// A test that drives an agentic-http route and the notice it sends in one async
// body (`platform_notices::ask_finished`) needs a type-layout depth past the
// default 128 — the same limit, for the same reason, as `oxy-app`'s own lib.
#![recursion_limit = "256"]

#[path = "../../../app/tests/common/mod.rs"]
mod common;
#[path = "../../../app/tests/common/oltp_fx.rs"]
mod oltp_fx;

mod notification_categories;
mod notification_devices;
mod notifications_send;
mod platform_notices;
mod slack_notify_dm;
mod test_notices;
