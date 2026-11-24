//! Platform tests that don't belong to a single product domain — the compile
//! boundary and the workspace-source rules it rests on, compiled readers, the
//! CLI `run` command, router shape in local mode, project queries,
//! workspace detail fields, and the partner-table regression guard.
//!
//! One binary for the whole group; see `tests/authz/main.rs` for why. Add a case
//! as a `mod` here rather than a new `tests/*.rs`. The experiment and simulation
//! modules left with their code for `oxy-api-experiment`'s `tests/integration/`
//! (2026-11-08); `oxy-app` cannot name that crate.
//!
//! Three of these ask "where may a workspace file be read from?" rather than
//! "which pod serves this route" — `compiled_reader_is_not_a_back_door`,
//! `workspace_path_backdoor` and `workspace_path_escape_hatch` (the fourth,
//! `walker_storage_divergence`, went to core). That is the line between this group and
//! `tests/routing/`: nothing here reasons about the router. They are source
//! scans and walker-semantics tests, so they open no database and mutate no
//! process-global state.
//!
//! Mixed three ways, and the split is in `.config/nextest.toml`:
//!
//! * `admin_staff_scope`, `compiled_reader_semantic`,
//!   `airway_compile_boundary`, `file_credentials_at_compile`, `revision_miss_not_from_disk`,
//!   `worker_ctx_follows_promote`,
//!   `semantic_scan_from_memory`, `context_documents_boundary`, `eval_tests_boundary`,
//!   `connector_vars_from_workspace_secrets`,
//!   `encryption_key_check` and `master_key_kms` are database-backed
//!   through [`common::fresh_db`] — own database each, so they sit in
//!   `db-per-test` (`max-threads = 4`).
//! * (`local_mode_router`, which calls `api_router(..)` and so reaches the
//!   *shared* `OXY_DATABASE_URL`, went up into `oxy-server`'s integration group
//!   with the router in decompose LIFT, 2026-11-23, and its `serial-db` pin
//!   with it.)
//! * everything else is in-process, but NOT ungrouped: that override matches
//!   `binary(=platform)`, so every module here except those exclusions runs
//!   under `db-per-test` (`max-threads = 4`, `retries = 2`) whether or not it
//!   opens a connection. The three source scans folded in above inherit that.
//!   Deliberate, and argued in the config: a per-binary rule stays true the day
//!   someone adds a harness-backed test to one of those files, where a module
//!   allowlist would silently go stale. `tests/routing/` is the group that got
//!   the other answer — nothing in it can reach a database, so it is in no
//!   group at all.
//!
//! `authz::shared_db_registry` fails the build if that classification drifts.
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

#[path = "../common/mod.rs"]
mod common;

mod admin_staff_scope;
mod airway_compile_boundary;
mod airway_fleet_control;
mod analytics_run_thread_access;
// `anomaly_notify` moved to `oxy-api-metric-monitoring`'s integration group
// with the insights delivery's executor (2026-11-21).
// `artifact_naming_agrees` and `walker_storage_divergence` moved to `oxy`'s
// (core's) integration group, which depends on `oxy-compile` (decompose T-small).
// `chat_org_standing` moved to `oxy-api-threads`' integration group with the
// chat handlers (2026-11-16).
// `compile` (`oxy compile`'s two ignored smoke tests) moved to `oxy-server`'s
// integration group, the crate that builds the binary (decompose T-small).
mod compile_first;
mod compile_from_git;
mod compile_oltp_promote;
mod compile_push;
mod compile_reconcile;
mod compile_request;
mod compile_self_heal;
mod compiled_reader_is_not_a_back_door;
mod compiled_reader_semantic;
mod connector_vars_from_workspace_secrets;
mod context_documents_boundary;
mod coordinator_any_replica;
mod data_placement;
mod encryption_key_check;
mod env_pins;
mod eval_tests_boundary;
mod factory_access;
mod feature_flag_refresh;
mod file_credentials_at_compile;
mod fleet_claim;
mod frontline_app_grant;
mod frontline_pin;
mod master_key_kms;
// `metric_tree_fit_panel` moved into `oxy-metric-monitoring` as the in-source
// `metric_tree_fit_panel_tests`, beside `splitting_executor` (decompose T-small).
mod migration_rollback_safety;
mod migration_rollback_tolerance;
mod no_dropped_partner_tables;
mod oltp_branches;
mod oltp_branches_env;
mod oltp_branches_guards;
mod oltp_branches_local;
mod oltp_branches_teardown;
mod oltp_env_branches;
mod oltp_env_branches_cap;
mod oltp_env_branches_recut;
mod oltp_env_branches_stale;
mod oltp_provisioner;
// `periodic_claim` and `run_retention` moved into `oxy-server` with the drivers
// (in-source `db_tests` beside the code, decompose BL1, 2026-11-23).
mod preagg_promote_nudge;
mod queued_chat_run;
mod revision_miss_not_from_disk;
mod rollout_gates;
// `run` spawned `oxy run`; with `oxyc run` (2026-11-24) its cases moved to
// `sdk/cli/src/commands/run.test.ts`, over the same `oxy_example` fixture files.
mod run_routes_workspace_scope;
mod semantic_scan_from_memory;
// `shared_state_boot` moved to `oxy-server` (a unit-test module of the
// binary) with `shared_state_check` on 2026-11-22 (decompose UP2).
// `usage_report` moved to `oxy-api-billing`'s integration group with the
// usage report's surface (admin split, PR 8b).
mod worker_ctx_follows_promote;
// `workspace_details_fields` moved to `oxy-api-workspaces`' integration group
// with the details builders it drives (2026-11-16).
mod workspace_path_backdoor;
mod workspace_path_escape_hatch;
mod workspace_path_needs_is_dir;
mod workspace_repo_facts;
