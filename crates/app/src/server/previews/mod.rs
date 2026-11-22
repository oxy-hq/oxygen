//! Workspace previews — open a branch of a workspace on the real product,
//! against real data, without the branch being live. Staff only
//! (`Action::WorkspacePreview`): customers do not build or change workspaces,
//! Oxy does.
//!
//! Built on custom-app staging (`internal-docs/customer-apps-staging.md`), not
//! beside it — one mechanism:
//!
//! * **Compile** — a preview is a **staging revision**: the branch head compiled
//!   through `compile_request::compile` (`kind = 'staging'`, never promoted;
//!   a ready revision of the same SHA is reused). A branch on GitHub is
//!   compiled from the commit GitHub has, on any pod.
//! * **Registry** ([`store`]) — `workspace_previews` remembers only which
//!   branches staff are previewing, at which commit, and who asked. Everything
//!   else (status, revision, error) is read off `revisions`, so there is no
//!   second copy of compile state to drift.
//! * **Serving** ([`pin`], [`read_only`], and the write hold, `oxy_app_core::write_hold`) — a request
//!   carrying `x-oxy-preview-revision: <revision_id>`, from staff, naming a
//!   ready staging (or main) revision of this workspace **at the commit a live
//!   preview is at**, runs inside
//!   `oxy_app_core::staging_pin_scope::with_staging_pin`: every compile-boundary read
//!   answers from that revision on whichever pod serves it, rollups are skipped,
//!   data-app caches are partitioned, and it may read but not run or change
//!   anything — refused by route ([`read_only`]) and, on the routes that do run
//!   something, held where it executes (`oxy_app_core::write_hold`: every connector
//!   refuses a write, no automation delegation, no Airway step, no write over
//!   HTTP). The pin is an immutable revision id, never a branch name. Every
//!   request without the header — the IDE on the same branch included — is
//!   untouched.
//! * **Release** (`revisions`) — deleting a preview, or refreshing it to a
//!   newer commit, deletes the staging revisions it stopped using, under the
//!   retention rule (never current, never build-pinned) and never while another
//!   preview or an unfinished preview run still uses them.
//!
//! Background work never reads a preview: the pin is request context, a task
//! spawned from the request does not inherit it, and `current_revision_id`
//! never points at a staging revision.
//!
//! * **Checks** (`analyze`, `checks`) — when a previewed branch's staging
//!   revision is ready, a durable `preview_analyze` task compares each
//!   `.airway.yml` the branch changed with the live tables and flags a change
//!   Airway cannot absorb as "needs Reset schema after merge". One check per
//!   revision (`workspace_preview_runs`, kind `analyze`); its report is the
//!   run's outcome metadata. The same check lists the automations the branch
//!   changed and queues a `transform_build` for each pure-Airhouse transform.
//! * **Procedure dry runs** (`runs`, [`runtime`], and the platform in
//!   `agentic_wiring::preview_ctx`) — staff run a branch's procedure on the
//!   worker fleet against its staging revision; reads and agents run, managed
//!   Airhouse writes land in the preview's own schemas, every other write is
//!   held and reported. Behind `OXY_PREVIEW_RUNS`. This is the one place
//!   background work reads a staging revision, and only for a run the
//!   registry names.
//! * **Transform compares** (`compare`) — a finished `transform_build` is
//!   compared with live, table by table, counts only.
//! * **Airway samples** (`sample`, [`sources`]) — staff run a bounded window
//!   of a branch's pipeline into the preview's own schemas, under a
//!   preview-owned pipeline key; a QuickBooks sample runs only against a
//!   sandbox company staff registered, never production's grant.
//! * **Preview schemas** ([`registry`], [`ddl`], `maintenance`, `drop`) —
//!   a preview's Airhouse writes land in its own schemas,
//!   `preview_<key>__<live schema>` ([`namespace`]). Each is registered in
//!   `workspace_preview_schemas` before it is created, expires
//!   `OXY_PREVIEW_SCHEMA_TTL_HOURS` after the preview last wrote, and is
//!   dropped by a `preview_schema_drop` task that a boot-time sweep queues.
//!   Only registered, well-formed schemas the preview itself created are ever
//!   dropped, and only the relations it recorded in them; a schema that was
//!   already there, or holds anything else, is refused and left alone.

//!
//! **Where it lives.** Since 2026-11-14 this module is the lower half: what the
//! workspace runtime, the middleware and the compile worker below the previews
//! surface read. Since 2026-11-22 (the decompose's P1) its rows — the registry
//! and DDL, the namespace, the sources, the pin and the run queue in `queue`,
//! including the per-pass runs' sweep the global driver calls inline — are the
//! `oxy-previews` crate, re-exported here at their old paths. What is still
//! here: the read-only guard's axum adapter ([`read_only`]; its table went
//! down to `oxy_previews::read_only` on 2026-11-22, P2), which goes down with
//! the workspace middleware. The run platform ([`runtime`]) went down into
//! `oxy-workspace-runtime` (`preview_runtime`) with the preview platform on
//! 2026-11-22 (W5), re-exported here at its old path. The DuckDB double ([`airhouse_duckdb`]) went down into
//! `oxy-workspace-runtime` with the preview connector on 2026-11-22 (W3),
//! re-exported here at its old path. The upper half — the
//! `/{workspace_id}/previews` routes, `service`, `revisions`, `analyze`,
//! `checks`, `compare`, `sample`, `drop`, `runs` and `maintenance` — is
//! `oxy-api-previews`, mounted through `SurfaceSeams::previews`, `tasks` and
//! `background`; it reads the rows from `oxy_previews` and the rest through
//! `oxy_app::surface::previews`.

/// Test stand-in for Airhouse; never wired into the server (see its module doc).
#[doc(hidden)]
pub use oxy_workspace_runtime::airhouse_duckdb;
/// The run platform resolver, `oxy-workspace-runtime`'s `preview_runtime` since
/// 2026-11-22 (W5), re-exported at its old path.
pub use oxy_workspace_runtime::preview_runtime as runtime;
/// The read-only guard's axum adapter (`check`, `refusal`, and the table it
/// re-exports from `oxy_previews::read_only`), down with the workspace
/// middleware into `oxy-workspace-runtime` (decompose W7, 2026-11-22).
pub use oxy_workspace_runtime::request::preview_read_only as read_only;

pub use oxy_previews::{
    ddl, ddl_airhouse, ddl_duckdb, namespace, pin, queue, registry, sources, sql_kind, store,
};
