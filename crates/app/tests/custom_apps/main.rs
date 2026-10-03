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
//! | `app_environments_phase_1b` | staging hosts serve staging HTML to staff only; staging `/fn` runs for staff only; a sandbox host and a sandbox's `/fn` serve the sandbox's own build to staff only, read per request, and a name nobody created runs nothing; a cookie header and a cross-environment origin are refused; a staging task is refused by the runner |
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
//! | `nonprod_function_uploads` | a staging or sandbox function's `ctx.fetch` PUT to the upload URL its own invocation minted is sent, and is not in its held row; every other mutating fetch is still held — production's silo, another environment's, another app's, another host, a traversal, another bucket or query, another method, a URL an earlier invocation minted; production sends what it always sent. No V8: the real host, driven from Rust |
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
//! | `custom_app_functions_manual_run` | admin Run now, queue, the production driver entry point and executor, run status; its children: a check run in a named environment and every refusal around one, the invocation listings and who may read which rows (a build only staging or a sandbox serves included), the held-write and run read-backs, the logs filter |
//! | `custom_app_functions_manual_run_guards` | source scans: the executor registration and admin stack that test copies still match production |
//! | `custom_app_functions_shape_zoo` | every ClickHouse, Postgres and DuckDB zoo case, read one column at a time through `ctx.warehouse.query` in a published function, against `expect.warehouse` |
//! | `custom_app_functions_shape_zoo_oltp` | the Postgres zoo cases through `ctx.oltp` in the app's own provisioned schema, against `expect.oltp` |
//! | `shape_zoo` | `fixtures/data-shapes/zoo.json` is well formed, and the SQL built from it matches the shared vector the canary also tests |
//! | `shape_zoo_coverage` | source scans: every native type `ch_type_to_typed`, `strip_type_wrappers`, `pg_typname_to_typed`, `is_decodable` and `describe_type_to_typed` name has a zoo case, or a reasoned exemption |
//! | `canary_coverage` | source scans: every host op in `HOST_OPS` is declared by a platform-canary step (`STEP_OPS` in the canary's `steps.ts`), or exempted with a reason; a declared op the host lacks is refused |
//! | `sandbox_environments` | a sandbox resolves its own build from its row, uncached, and never another environment's; an absent or deleting one resolves to nothing; a sandbox page's data reads take the pin of its own build |
//! | `sandbox_isolation` | two sandboxes of one app, through the real serve route after a real publish: each runs its own build in its own storage silo; each reads its own secrets, then staging's, then production's `shared` ones, and cannot rotate a key it read from staging |
//! | `sandbox_isolation_airhouse` | two sandboxes' `ctx.airhouse` appends land in two distinct siblings, each on a connection scoped to its own |
//! | `sandbox_loop` | the whole loop on one app, in order: two sandboxes created by route, a build each, a call in each by `X-Oxy-App-Env`, a check through the queue and the production executor, its invocation and held list by route, a delete and its teardown — each sandbox on its own build, policy, secrets and silo, invisible to the other, to staging and to production, whose pointers never move; an upload URL one sandbox minted is sent from it and held from the other and from staging; the freed name inherits nothing. And again for the app's own database: each sandbox's `oltp_schema` goes `ready` by route, a row written in one is seen by no other environment, a teardown drops that schema alone, and the freed name starts from staging's rows |
//! | `sandbox_oltp_isolation` | with the org's OLTP staging branch, two sandboxes of one app and staging each read and write their own `orders` through the real publish, queued task and serve route, and production is untouched; a sandbox's migration adds a column only that sandbox has, under its own ledger target, and a file naming staging's schema fails the task; a statement naming another schema or changing the search path is refused and listed; before the schema is seeded, and after a branch reset until the next publish, `ctx.oltp` is refused rather than run in staging's schema; with no branch it is held as before |
//! | `sandbox_oltp_teardown` | a teardown drops that sandbox's OLTP schema and ledger rows and leaves staging's and the other sandbox's; one that cannot confirm the drop (the branch mid-reset) fails and keeps the row, then finishes once the branch is back; a state this build cannot read still gets its schema dropped; a sandbox whose row records no schema connects to nothing |
//! | `sandbox_oltp_seed` | a sandbox's schema on the org's OLTP staging branch is created by the branch owner and seeded as the app's writer: every table's structure, the rows under the cap, sequences at staging's position behind re-pointed defaults, foreign keys on the copies; the drop removes that schema alone; a name that is a writer's own schema is never created over or dropped; no branch, nothing |
//! | `sandbox_publish` | a publish with `environment` moves that sandbox's pointer and leaves the app row, staging, production and the schedules byte-identical; a semantic pin rides the sandbox's build |
//! | `sandbox_publish_migrations` | a sandbox publish queues its Airhouse migrations under their own kind and applies no OLTP file; the task applies nothing for a sandbox that is locked, deleted or serving another build; sandbox builds are pruned in a window of their own |
//! | `sandbox_publish_refusals` | what a sandbox publish refuses, each leaving no build row or bytes: `promote`, no reach, an unknown app or sandbox, a deleting one; a machine or app-scoped token, refused by the sandbox itself past `authorize_publish`; a sandbox deleted after admission — `409`, the stored build rolled back |
//! | `sandbox_publish_route` | the `environment` multipart field through the real publish handler: the sandbox, the two new response fields on every publish, the plain-text refusals, a publish token refused |
//! | `sandbox_routes` | create / list / show / delete through the staff console's guards: the Environment object; a duplicate, a name still tearing down, `staging` and a malformed name refused with their codes; a second `DELETE` answers the run on its way |
//! | `sandbox_routes_guards` | every handler refuses a caller without non-production reach and a publish token; the console's layers refuse a non-staff caller and answer an operator scoped to another org `404`; production mounts the routes inside those layers |
//! | `sandbox_routes_limit` | the 21st sandbox is refused and one being torn down counts; every create waits for the app row's lock, so racing creates cannot pass the limit |
//! | `sandbox_schema_collision` | a legacy `--` slug's own schema is never another app's sibling: a sandbox of that name is not created or dropped, and an apply into it — staging's or a sandbox's — fails naming the collision instead of ending "nothing to apply" |
//! | `sandbox_secrets_publish_token` | a publish token, whoever minted it, cannot list, reveal, set or delete staging's or a sandbox's secrets (the reads behind the token's own scope middleware); production's are answered as before |
//! | `sandbox_sweep` | the sweep expires a sandbox idle past its TTL (exactly what `expires_at` says) and spares one invoked since; a second replica queues nothing; a teardown stuck six hours is queued again, one still on its way is not |
//! | `sandbox_sweep_races` | the sweep looks again under the row lock: a publish that lands between its selection and its delete saves the sandbox; a retry never deletes a sandbox created again under the name |
//! | `sandbox_task_kinds` | both sandbox task kinds are registered with the worker fleet and filed as platform daemons in the run feed's lists (backend and frontend) and workspace health's |
//! | `sandbox_teardown` | dropping a sandbox's Airhouse sibling and ledger rows alone, never a fixed environment's; a fixed environment's is refused with everything still in it |
//! | `sandbox_teardown_failures` | a teardown whose Airhouse step cannot finish leaves the sandbox deleting: Airhouse unreachable, and a worker with no Airhouse at all while the ledger says a sibling exists; a sandbox that never had one is still torn down there |
//! | `sandbox_teardown_races` | a late teardown run removes nothing once an earlier run finished, nor anything of a sandbox created again; a run that cannot take the sandbox's lock removes nothing; a deleted app's sandbox is still torn down; a secret already gone is not a failure |
//! | `sandbox_teardown_task` | the queued teardown, through its executor, removes one sandbox's silo, secrets and row and keeps invocations, events and audit rows; a failing step leaves it deleting; a payload naming staging removes nothing |
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
mod nonprod_function_uploads;
mod sandbox_environments;
mod sandbox_isolation;
mod sandbox_isolation_airhouse;
mod sandbox_loop;
mod sandbox_oltp_isolation;
mod sandbox_oltp_seed;
mod sandbox_oltp_teardown;
mod sandbox_publish;
mod sandbox_publish_migrations;
mod sandbox_publish_refusals;
mod sandbox_publish_route;
mod sandbox_routes;
mod sandbox_routes_guards;
mod sandbox_routes_limit;
mod sandbox_schema_collision;
mod sandbox_secrets_publish_token;
mod sandbox_sweep;
mod sandbox_sweep_races;
mod sandbox_task_kinds;
mod sandbox_teardown;
mod sandbox_teardown_failures;
mod sandbox_teardown_races;
mod sandbox_teardown_task;
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
