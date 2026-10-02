//! `oxy-api-frontline`'s integration tests — one binary, per the workspace rule
//! (add a case as a `mod` here, never a new top-level `tests/*.rs`).
//!
//! These moved out of `oxy-app`'s `tests/platform/` and `tests/authz/` with the
//! frontline handlers. The DB-backed ones (`frontline_devices`,
//! `frontline_roster`, `frontline_kiosk_signin`, `frontline_kiosk_mode`,
//! `frontline_kiosk_leave_route`, `frontline_kiosk_probe`) keep using
//! `oxy-app`'s harness, included by path rather than copied: it owns the
//! per-run template database and the stray-database sweep, and a second copy would drift from both. Like the
//! oxy-app binaries it serves, it requires nextest's process-per-test mode; the
//! `db-per-test` group in `.config/nextest.toml` covers this binary too.
//!
//! `frontline_device_guards` and `route_roles` open no database.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod frontline_device_guards;
mod frontline_devices;
mod frontline_kiosk_leave_route;
mod frontline_kiosk_mode;
mod frontline_kiosk_probe;
mod frontline_kiosk_signin;
mod frontline_roster;
mod route_roles;
