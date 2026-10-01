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
//! | `custom_apps_publish_machine` | a trusted (OIDC) publish records its build with no user, attributed to its workflow |
//! | `function_failure_alerts` | the pager's SQL over invocation rows inserted by hand |
//! | `app_environments_phase_1b` | staging hosts serve staging HTML to staff only; staging `/fn` runs for staff only and a dev slot's never; a cookie header and a cross-environment origin are refused; a staging task is refused by the runner |
//! | `environment_scoped_keys` | an idempotency key, a cached result or a ledger row from a non-production environment never stands in for production's |
//! | `staging_functions` | a staging host runs the staging build as `ctx.channel = "staging"` for staff; the held row is written on throw and timeout; a GET carrying a body is held |
//! | `staging_write_probe` | every write `HostOp`, from a table checked against the policy, is held (Airway refused) and listed in one `app.staging.held` row, unless the policy gives it a home — an unmapped warehouse write stays held; storage, secrets and email land in their isolated staging homes |
//! | `staging_homes_differential` | the same warehouse write from staging and production lands on disjoint databases, through the real host: the mapped destination (and `ctx.tx`) vs production's; an unmapped database is held; the mapped name passes production's gate; a statement naming production's database or another is held on the mapped connection |
//! | `staging_airhouse_sibling` | staging's `ctx.airhouse` appends and execs land in the sibling `app_<writer>__staging` on a connection scoped to it, never in the app's schema; its queries read production's |
//! | `staging_airhouse_migrations` | staging's Airhouse migrations run in the sibling under their own ledger target, so promote still applies production's |
//! | `staging_publish_never_blocks` | a staging-only publish queues its sibling migration once and answers; the task's failure is its run's, never the publish's; a promote whose mapping cannot be checked still publishes with a warning; production's and the sibling's apply locks do not contend |
//! | `staging_destination_guards` | the host re-checks a staging mapping on every write against the config as it is now, failing closed: production's host and user, any mapped production database, a chain, the workspace's own Airhouse, DuckDB to DuckDB, or an unresolvable host is refused (a chain held) and nothing is written |
//! | `staging_destination_publish` | publish refuses a `nonProduction.destinations` mapping onto production's host and user (a heuristic), onto itself, onto the workspace's own Airhouse, or to an unconfigured database |
//! | `staging_function_homes` | staging `ctx.storage` works in its own silo (production's read-only behind it; `delete`/`copy` never reach production; production's key exists for `allowOverwrite: false`), `ctx.email.send` reaches the invoker alone, replies included |
//! | `staging_function_secrets` | staging `ctx.env` overlays production only for keys both the staging build and production's build mark `shared` (none for an app never promoted), `ctx.secrets.set` writes staging's path; a publish refuses `shared` on a key this build or production's writes or verifies webhooks with |
//! | `staging_storage_limits` | a staging silo has its own cap, and its bytes never count toward the org quota that gates production's writes |
//! | `staging_secrets_admin` | the staff secrets surface sets, lists and deletes `apps/<id>/staging/<KEY>` for staff only (oxy-authz `AppNonProduction`), audited with the environment; a key is one segment; the tenant project-secrets routes never list or reach a staging row |
//! | `staging_functions_oltp` | a staging function reads production's OLTP rows; `COMMIT`/`SET TRANSACTION READ WRITE` escapes, `set_config` (by name, `U&"…"`-escaped, or inside `query_to_xml` text) and multi-statement strings are held unsent; `READ ONLY` refuses a write inside an app-defined function |
//! | `staging_functions_oltp_branch` | with the org's OLTP staging branch, every OLTP write op (a table checked against the policy), DDL, a savepoint and an app-defined writing function run on the branch and never production; `dblink`, server files, `COPY … PROGRAM` and `ALTER ROLE` are refused there and listed in the held row; production's and staging's `ctx.oltp` resolve disjoint databases; a branch row naming production is refused |
//! | `staging_branch_migrations_races` | a branch whose table another session holds makes the queued step time out and fail its task, and the staging pointer had already moved; a file applied across a branch reset is not recorded |
//! | `staging_branch_migrations` | a publish queues the org's OLTP staging branch migration, which the executor applies under `branch:<id>` and never production's ledger or database; a failing file fails the task, not the publish; a cut and a reset copy production's ledger |
//! | `staging_migration_tasks` | (helpers) runs a publish's queued staging migration tasks through the executor production registers |
//! | `staging_functions_semantic` | a staging function reads its build's pinned model, and a rollup of the promoted model never answers it |
//! | `staging_function_pager` | a failing staging function neither pages nor claims production's first-occurrence slot, end to end (both guards: `failure_page::observe` and `claim` itself) |
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
//! `custom_app_staging_pin` proves the staging semantic pin: a staging request
//! reads the draft build's pinned revision while live and every other request
//! read the promoted one, retention keeps a pinned revision, and a staging
//! revision is never promoted.
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
mod app_environments_phase_1b;
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
mod custom_app_functions_semantic_revision;
mod custom_app_functions_shape_zoo;
mod custom_app_functions_shape_zoo_oltp;
mod custom_app_platform_runtime;
mod custom_app_staging_pin;
mod custom_app_storage_routes;
mod custom_app_visibility;
mod custom_apps_boundary;
mod custom_apps_cache_invalidation;
mod custom_apps_publish_function_artifacts;
mod custom_apps_publish_machine;
mod custom_apps_publish_workspace;
mod environment_scoped_keys;
mod example_app_serving;
mod function_failure_alerts;
mod seed_example_app;
mod shape_zoo;
mod shape_zoo_coverage;
mod staging_airhouse_migrations;
mod staging_airhouse_sibling;
mod staging_branch_migrations;
mod staging_branch_migrations_races;
mod staging_destination_guards;
mod staging_destination_publish;
mod staging_function_homes;
mod staging_function_pager;
mod staging_function_secrets;
mod staging_functions;
mod staging_functions_oltp;
mod staging_functions_oltp_branch;
mod staging_functions_semantic;
mod staging_homes_differential;
mod staging_homes_fixture;
mod staging_migration_tasks;
mod staging_publish_never_blocks;
mod staging_secrets_admin;
mod staging_storage_limits;
mod staging_write_probe;
mod storage_history_query;
mod warehouse_writes_on_engines;
