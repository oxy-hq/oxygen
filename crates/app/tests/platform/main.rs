//! What is left of oxy-app's platform tests: nothing. Every module has moved
//! out (the last ones in decompose R-d, 2026-11-25); decompose DEL removes this
//! file.
//!
//! One binary for the whole group; see `tests/authz/main.rs` for why. Add a case
//! as a `mod` here rather than a new `tests/*.rs`. The experiment and simulation
//! modules left with their code for `oxy-api-experiment`'s `tests/integration/`
//! (2026-11-08); `oxy-app` cannot name that crate.
//!
//! The compile cluster (the compile boundary's tests, the compiled readers,
//! the env pins, the feature-flag gates and the repository facts) moved to
//! `oxy-workspace-runtime`'s integration group (decompose R-b, 2026-11-25), and
//! the rest — the staff-scope fences, the run routes and the queued chat run,
//! the Factory and frontline gates, the fleet claim, the master-key checks,
//! migration rollback safety and tolerance, and `data_placement` — with it in
//! decompose R-d (2026-11-25).
//!
//! The source scans that asked "where may a workspace file be read from?" —
//! `compiled_reader_is_not_a_back_door` and the `workspace_path_*` trio — and
//! the partner-table guard moved to `oxy-guards` (decompose GRD, 2026-11-24).
//!
//! It was mixed three ways, and the split is in `.config/nextest.toml`:
//!
//! * the database-backed modules (`admin_staff_scope`, `encryption_key_check`,
//!   `master_key_kms`, until decompose R-d) took their own database each
//!   through [`common::fresh_db`], so they sat in `db-per-test`
//!   (`max-threads = 4`).
//! * (`local_mode_router`, which calls `api_router(..)` and so reaches the
//!   *shared* `OXY_DATABASE_URL`, went up into `oxy-server`'s integration group
//!   with the router in decompose LIFT, 2026-11-23, and its `serial-db` pin
//!   with it.)
//! * everything else is in-process, but NOT ungrouped: that override matches
//!   `binary(=platform)`, so every module here except those exclusions runs
//!   under `db-per-test` (`max-threads = 4`, `retries = 2`) whether or not it
//!   opens a connection.
//!   Deliberate, and argued in the config: a per-binary rule stays true the day
//!   someone adds a harness-backed test to one of those files, where a module
//!   allowlist would silently go stale. `oxy-guards`' group (oxy-app's
//!   `tests/routing/` until decompose GRD) got the other answer — nothing in
//!   it can reach a database, so it is in no group at all.
//!
//! `oxy-guards`' `shared_db_registry` fails the build if that classification drifts.
//!
//! No member mutates process cwd any more: `metric_tree_fit_panel` did until it
//! moved into `oxy-metric-monitoring` (decompose T-small, 2026-11-24), and
//! `workspace_details_fields::storage_key_normalizes_relative_to_absolute` did
//! until it moved to `oxy-api-workspaces` on 2026-11-16. Both carry
//! `#[serial_test::serial]`, whose
//! lock is global to a binary; anything folded in here that touches cwd,
//! environment variables or another process-global needs the same attribute — a
//! guard private to one file excludes nothing once the files share a binary.

// A test that drives an agentic-http route and the notice it sends in one async
// body (`platform_notices::ask_finished`) needs a type-layout depth past the
// default 128 — the same limit, for the same reason, as `oxy-app`'s own lib.
#![recursion_limit = "256"]

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

// `admin_staff_scope`, `analytics_run_thread_access`, `coordinator_any_replica`,
// `data_placement`, `encryption_key_check`, `factory_access`, `fleet_claim`,
// `frontline_app_grant`, `frontline_pin`, `master_key_kms`,
// `migration_rollback_{safety,tolerance}`, `queued_chat_run` and
// `run_routes_workspace_scope` moved to `oxy-workspace-runtime`'s integration
// group (decompose R-d, 2026-11-25).
// `anomaly_notify` moved to `oxy-api-metric-monitoring`'s integration group
// with the insights delivery's executor (2026-11-21).
// `artifact_naming_agrees` and `walker_storage_divergence` moved to `oxy`'s
// (core's) integration group, which depends on `oxy-compile` (decompose T-small).
// `chat_org_standing` moved to `oxy-api-threads`' integration group with the
// chat handlers (2026-11-16).
// `compile` (`oxy compile`'s two ignored smoke tests) moved to `oxy-server`'s
// integration group, the crate that builds the binary (decompose T-small).
// `metric_tree_fit_panel` moved into `oxy-metric-monitoring` as the in-source
// `metric_tree_fit_panel_tests`, beside `splitting_executor` (decompose T-small).
// The OLTP cluster (`oltp_provisioner`, whose fixture the other `oltp_*` files
// and `compile_oltp_promote` share, and those ten) moved to
// `oxy-workspace-runtime`'s integration group (decompose R-c, 2026-11-25).
// `periodic_claim` and `run_retention` moved into `oxy-server` with the drivers
// (in-source `db_tests` beside the code, decompose BL1, 2026-11-23).
// `run` spawned `oxy run`; with `oxyc run` (2026-11-24) its cases moved to
// `sdk/cli/src/commands/run.test.ts`, over the same `oxy_example` fixture files.
// `shared_state_boot` moved to `oxy-server` (a unit-test module of the
// binary) with `shared_state_check` on 2026-11-22 (decompose UP2).
// `usage_report` moved to `oxy-api-billing`'s integration group with the
// usage report's surface (admin split, PR 8b).
// `workspace_details_fields` moved to `oxy-api-workspaces`' integration group
// with the details builders it drives (2026-11-16).
// `compiled_reader_is_not_a_back_door`, `no_dropped_partner_tables` and the
// three `workspace_path_*` scans moved to `oxy-guards`' integration group
// (decompose GRD, 2026-11-24): source scans, no database.
