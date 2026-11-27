//! Workspace previews' **rows**: what the workspace runtime, the workspace
//! middleware, the compile worker and the previews surface (`oxy-api-previews`)
//! read and write about a preview, below all of them.
//!
//! Out of `oxy-app`'s `server::previews` (and the custom-app preview settle,
//! `server::api::projects::automation_run::preview_settle`, which moved on to
//! `oxy_custom_apps::automation_run::preview_settle` on 2026-11-26) on
//! 2026-11-22, the decompose's P1, and re-exported at the old paths, so no
//! caller in `oxy-app` changed:
//!
//! - [`store`]: `workspace_previews`, which branches staff preview, at which
//!   commit, and who asked.
//! - [`registry`], [`ddl`], [`ddl_airhouse`], [`ddl_duckdb`], [`namespace`]: a
//!   preview's own Airhouse schemas, registered before they are created and
//!   dropped only when the preview created them.
//! - [`sources`]: the sandbox companies an Airway sample may run against.
//! - [`pin`]: which requests are served from a preview, and the headers that
//!   say so.
//! - [`queue`]: the preview run queue, its sweep and ceiling, the compares
//!   and change checks it queues.
//! - [`read_only`]: what a request pinned to a preview may still call, and
//!   what a refusal says (P2, 2026-11-22; the axum adapter that applies it,
//!   `check` and `refusal`, stays beside the workspace middleware in
//!   `oxy-app`).
//!
//! It is its own crate because the preview platform (`agentic_wiring::
//! preview_ctx`, `oxy-workspace-runtime`'s since W5) and these rows named
//! each other: the platform builds on the registry and the DDL, and the queue
//! named the platform's sample kind (inverted: [`queue::sample_seed::RUN_KIND`]).
//! What stays in `oxy-app` for now: `read_only`'s axum adapter (with the
//! workspace middleware); `airhouse_duckdb` went to `oxy-workspace-runtime`
//! with the preview connector, and `runtime` with the preview platform (as
//! `preview_runtime`): `internal-docs/domain-boundaries.md`, End state.

pub mod ddl;
pub mod ddl_airhouse;
pub mod ddl_duckdb;
pub mod namespace;
pub mod pin;
pub mod queue;
pub mod read_only;
pub mod registry;
pub mod sources;
pub mod store;
