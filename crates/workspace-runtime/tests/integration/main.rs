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
//! * The compile cluster (decompose R-b), out of `oxy-app`'s `platform`
//!   binary: compiling from git (`compile_from_git`, `compile_first`,
//!   `compile_push`, `compile_reconcile`, `compile_request`,
//!   `compile_self_heal`), the env pins (`env_pins`), what a compile admits
//!   (`airway_compile_boundary`, `file_credentials_at_compile`,
//!   `eval_tests_boundary`, `context_documents_boundary`) and reads only from
//!   the compiled revision (`revision_miss_not_from_disk`,
//!   `worker_ctx_follows_promote`, `compiled_reader_semantic`,
//!   `semantic_scan_from_memory`, `connector_vars_from_workspace_secrets`),
//!   the Airway fleet routes over it (`airway_fleet_control`), the post-promote
//!   pre-aggregation nudge (`preagg_promote_nudge`), the feature-flag cache and
//!   rollout gates (`feature_flag_refresh`, `rollout_gates`) and the
//!   repository facts (`workspace_repo_facts`). All take their own database
//!   through the harness.
//! * `sentry_surface_gate` (decompose R-a), the two tests out of `oxy-app`'s
//!   `routing::sentry_hub_scan` that drive the real custom-app gate
//!   (`enter_semantic_boundary`). In-process; they open no database.
//! * The OLTP tests (decompose R-c), out of `oxy-app`'s `platform` binary:
//!   `oltp_provisioner` and the `oltp_branches*` / `oltp_env_branches*` files
//!   that share its fixture (`with_fx`, `seed_org`, the harness's `oltp_fx.rs`),
//!   and `compile_oltp_promote` (`oxy_compile_runtime::oltp`). Control plane on
//!   a per-test database ([`common::Schema::CentralOltp`]); the tenant plane is
//!   real databases on the same cluster through `common::admin_url`.
//! * The rest of `oxy-app`'s `platform` binary that is not the compile or OLTP
//!   cluster (decompose R-d): the staff-scope fences, the run routes'
//!   workspace scope and the coordinator on any replica, the queued chat run,
//!   the Factory and frontline gates, the fleet claim, the master-key checks,
//!   migration rollback safety and tolerance, and `data_placement`.
//! * What was left of `oxy-app`'s `authz` binary (decompose R-d):
//!   `authz_loader_differential` and `audit_append_only`.
//!
//! Every database-backed test here but two gets its own database from a
//! per-run template ([`common::fresh_db`]), so this binary sits in the
//! `db-per-test` group (`max-threads = 4`) in `.config/nextest.toml`, not the
//! fully-serialized `serial-db` group. The two are `authz_loader_differential`
//! and `audit_append_only`: they call `establish_connection()` against the raw
//! shared `OXY_DATABASE_URL` with no per-test database, so the config carves
//! them out of this binary's `db-per-test` override and pins them into
//! `serial-db`. They skip when the var is unset, which is why a laptop run looks
//! clean and only CI would show the race. A module that reaches the *shared*
//! database (`establish_connection()`, `api_router(..)`) needs the same pin;
//! `oxy-guards`' `shared_db_registry` finds this group through its harness
//! include and fails the build if one is missing or stale.
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

mod admin_staff_scope;
mod airhouse_broker;
mod airhouse_broker_preview;
mod airhouse_lifecycle;
mod airhouse_pool_ceiling;
mod airhouse_pool_expiry;
mod airhouse_pool_poison;
mod airhouse_pool_reuse;
mod airhouse_provisioner;
mod airway_compile_boundary;
mod airway_fleet_control;
mod analytics_run_thread_access;
mod audit_append_only;
mod authz_loader_differential;
mod compile_first;
mod compile_from_git;
mod compile_oltp_promote;
mod compile_push;
mod compile_reconcile;
mod compile_request;
mod compile_self_heal;
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
mod migration_rollback_safety;
mod migration_rollback_tolerance;
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
mod preagg_promote_nudge;
mod queued_chat_run;
mod revision_miss_not_from_disk;
mod rollout_gates;
mod run_routes_workspace_scope;
mod semantic_scan_from_memory;
mod sentry_surface_gate;
mod worker_ctx_follows_promote;
mod workspace_repo_facts;
