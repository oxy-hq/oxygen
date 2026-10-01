//! Workspace previews — open a branch of a workspace on the real product,
//! against real data, without the branch being live. Staff only
//! (`Action::WorkspacePreview`): customers do not build or change workspaces,
//! Oxy does.
//!
//! Built on custom-app staging (`internal-docs/customer-apps-staging.md`), not
//! beside it — one mechanism:
//!
//! * **Compile** — a preview is a **staging revision**: the branch head compiled
//!   through `compile_staging::stage_branch` (`kind = 'staging'`, never
//!   promoted; a ready revision of the same SHA is reused).
//! * **Registry** ([`store`]) — `workspace_previews` remembers only which
//!   branches staff are previewing, at which commit, and who asked. Everything
//!   else (status, revision, error) is read off `revisions`, so there is no
//!   second copy of compile state to drift.
//! * **Serving** ([`pin`], [`read_only`], [`request_hold`]) — a request
//!   carrying `x-oxy-preview-revision: <revision_id>`, from staff, naming a
//!   ready staging (or main) revision of this workspace **at the commit a live
//!   preview is at**, runs inside
//!   `custom_apps_staging_pin::with_staging_pin`: every compile-boundary read
//!   answers from that revision on whichever pod serves it, rollups are skipped,
//!   data-app caches are partitioned, and it may read but not run or change
//!   anything — refused by route ([`read_only`]) and, on the routes that do run
//!   something, held where it executes ([`request_hold`]: every connector
//!   refuses a write, no automation delegation, no Airway step, no write over
//!   HTTP). The pin is an immutable revision id, never a branch name. Every
//!   request without the header — the IDE on the same branch included — is
//!   untouched.
//! * **Release** ([`revisions`]) — deleting a preview, or refreshing it to a
//!   newer commit, deletes the staging revisions it stopped using, under the
//!   retention rule (never current, never build-pinned) and never while another
//!   preview or an unfinished preview run still uses them.
//!
//! Background work never reads a preview: the pin is request context, a task
//! spawned from the request does not inherit it, and `current_revision_id`
//! never points at a staging revision.
//!
//! * **Checks** ([`analyze`], [`checks`]) — when a previewed branch's staging
//!   revision is ready, a durable `preview_analyze` task compares each
//!   `.airway.yml` the branch changed with the live tables and flags a change
//!   Airway cannot absorb as "needs Reset schema after merge". One check per
//!   revision (`workspace_preview_runs`, kind `analyze`); its report is the
//!   run's outcome metadata. The same check lists the automations the branch
//!   changed and queues a `transform_build` for each pure-Airhouse transform.
//! * **Procedure dry runs** ([`runs`], [`runtime`], and the platform in
//!   `agentic_wiring::preview_ctx`) — staff run a branch's procedure on the
//!   worker fleet against its staging revision; reads and agents run, managed
//!   Airhouse writes land in the preview's own schemas, every other write is
//!   held and reported. Behind `OXY_PREVIEW_RUNS`. This is the one place
//!   background work reads a staging revision, and only for a run the
//!   registry names.
//! * **Transform compares** ([`compare`]) — a finished `transform_build` is
//!   compared with live, table by table, counts only.
//! * **Airway samples** ([`sample`], [`sources`]) — staff run a bounded window
//!   of a branch's pipeline into the preview's own schemas, under a
//!   preview-owned pipeline key; a QuickBooks sample runs only against a
//!   sandbox company staff registered, never production's grant.
//! * **Preview schemas** ([`registry`], [`ddl`], [`maintenance`], [`drop`]) —
//!   a preview's Airhouse writes land in its own schemas,
//!   `preview_<key>__<live schema>` ([`namespace`]). Each is registered in
//!   `workspace_preview_schemas` before it is created, expires
//!   `OXY_PREVIEW_SCHEMA_TTL_HOURS` after the preview last wrote, and is
//!   dropped by a `preview_schema_drop` task that a boot-time sweep queues.
//!   Only registered, well-formed schemas the preview itself created are ever
//!   dropped, and only the relations it recorded in them; a schema that was
//!   already there, or holds anything else, is refused and left alone.

/// Test stand-in for Airhouse; never wired into the server (see its module doc).
#[doc(hidden)]
pub mod airhouse_duckdb;
pub mod analyze;
pub mod checks;
pub mod compare;
pub mod ddl;
pub mod ddl_airhouse;
pub mod ddl_duckdb;
pub mod drop;
pub mod hold;
pub mod maintenance;
pub mod namespace;
pub mod pin;
pub mod read_only;
pub mod registry;
pub mod request_hold;
pub mod revisions;
pub mod runs;
pub mod runtime;
pub mod sample;
pub mod service;
pub mod sources;
pub mod sql_kind;
pub mod store;
