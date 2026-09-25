//! Custom Apps platform tests — access control, visibility, the compile/serve
//! boundary, cache invalidation, the seeded example app, and Oxy Functions.
//!
//! One binary for the whole domain; see `tests/authz/main.rs` for why. Add a
//! case as a `mod` here rather than a new `tests/*.rs`.
//!
//! Oxy Functions are tested in layers; the isolate's own unit tests (over a
//! `MockHost`) live in `custom_apps_functions/runtime.rs`. Here:
//!
//! | Module | Proves |
//! | ------ | ------ |
//! | `warehouse_writes_on_engines` | host writes land on real engines — Rust to host, no V8 |
//! | `custom_apps_publish_function_artifacts` | a bundle declaring functions it lacks is refused |
//! | `function_failure_alerts` | the pager's SQL over invocation rows inserted by hand |
//! | `custom_app_functions_e2e` | publish, route call, isolate, invocation row; success and throw |
//! | `custom_app_functions_host_failures` | a caught paging host-call failure writes its fingerprint; a caught `not_found` does not |
//! | `custom_app_functions_clickhouse` | `ctx.warehouse.insert` from the isolate onto real ClickHouse |
//! | `custom_app_functions_manual_run` | admin Run now, queue, the production driver entry point and executor, run status |
//! | `custom_app_functions_manual_run_guards` | source scans: the executor registration and admin stack that test copies still match production |
//! | `custom_app_functions_shape_zoo` | every ClickHouse, Postgres and DuckDB zoo case, read one column at a time through `ctx.warehouse.query` in a published function, against `expect.warehouse` |
//! | `custom_app_functions_shape_zoo_oltp` | the Postgres zoo cases through `ctx.oltp` in the app's own provisioned schema, against `expect.oltp` |
//! | `shape_zoo` | `fixtures/data-shapes/zoo.json` is well formed, and the SQL built from it matches the shared vector the canary also tests |
//! | `shape_zoo_coverage` | source scans: every native type `ch_type_to_typed`, `strip_type_wrappers`, `pg_typname_to_typed`, `is_decodable` and `describe_type_to_typed` name has a zoo case, or a reasoned exemption |
//! | `canary_coverage` | source scans: every host op in `HOST_OPS` is declared by a platform-canary step (`STEP_OPS` in the canary's `steps.ts`), or exempted with a reason; a declared op the host lacks is refused |
//!
//! `custom_app_functions_fixture` holds no tests: it seeds, publishes and calls.
//!
//! Most of these are database-backed via [`common::test_db`], which gives each
//! test its own database cloned from a per-run template. The binary is therefore
//! in the `db-per-test` group (`max-threads = 4`) in `.config/nextest.toml`, not
//! the fully-serialized `serial-db` group — they contend for one Postgres server,
//! not for a schema.

#[path = "../common/mod.rs"]
mod common;

mod admin_app_workspace_in_org;
mod app_environment_pointer_writes;
mod app_environments;
mod app_preflight;
mod app_scoped_secrets;
mod canary_coverage;
mod custom_app_access_control;
mod custom_app_activity_roles;
mod custom_app_functions_clickhouse;
mod custom_app_functions_e2e;
mod custom_app_functions_fixture;
mod custom_app_functions_host_failures;
mod custom_app_functions_manual_run;
mod custom_app_functions_manual_run_guards;
mod custom_app_functions_shape_zoo;
mod custom_app_functions_shape_zoo_oltp;
mod custom_app_platform_runtime;
mod custom_app_storage_routes;
mod custom_app_visibility;
mod custom_apps_boundary;
mod custom_apps_cache_invalidation;
mod custom_apps_publish_function_artifacts;
mod custom_apps_publish_workspace;
mod example_app_serving;
mod function_failure_alerts;
mod seed_example_app;
mod shape_zoo;
mod shape_zoo_coverage;
mod storage_history_query;
mod warehouse_writes_on_engines;
