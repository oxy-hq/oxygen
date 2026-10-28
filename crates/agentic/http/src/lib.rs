//! `agentic-http` — Axum HTTP routes for the agentic analytics pipeline.
//!
//! # Wiring into your axum app
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use agentic_http::{AgenticState, router};
//!
//! let state = Arc::new(AgenticState::new(shutdown_token, db));
//!
//! let app = axum::Router::new()
//!     .nest("/analytics", router(state));
//! ```
//!
//! # Routes exposed
//!
//! | Method | Path                          | Description                          |
//! |--------|-------------------------------|--------------------------------------|
//! | POST   | `/analytics/runs`             | Start a pipeline run                 |
//! | GET    | `/analytics/runs/:id/events`  | SSE stream (live + catch-up)         |
//! | POST   | `/analytics/runs/:id/answer`  | Deliver answer to a suspended run    |
//! | POST   | `/analytics/runs/:id/cancel`  | Cancel a running or suspended run    |
//! | POST   | `/analytics/builder/runs`     | Start a Builder or onboarding run    |
//! | POST   | `/analytics/builder/runs/:id/answer` | Answer a suspended Builder run |

pub mod coordinator;
pub mod db;
pub mod routes;
pub mod sse;
pub mod state;

pub use state::{AgenticState, RunStatus};

use sea_orm::DatabaseConnection;

/// Run startup maintenance: reconcile stale (running/suspended) runs left by a
/// previous process. Runs that had made progress are marked **for resume** (the
/// recovery loop re-drives them); only runs that can't be resumed (interrupted
/// delegation, never-started placeholders) are failed.
///
/// Call this once after migrations complete, before the HTTP server begins
/// accepting requests.  Idempotent — safe to call every boot.
pub async fn cleanup_stale_runs(
    db: &DatabaseConnection,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let count = db::cleanup_stale_runs(db).await?;
    if count > 0 {
        // Not all "failed": runs with progress are set to `needs_resume` and
        // re-driven by the recovery loop. This is reconciliation, not loss.
        tracing::info!(
            count,
            "reconciled stale agentic runs on startup (those with progress resume; the rest fail)"
        );
    }
    Ok(count)
}

use axum::{
    Router,
    routing::{get, patch, post},
};
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};
use std::sync::Arc;

/// What each analytics route needs from the pod serving it.
///
/// Declared HERE, beside the routes, because this crate owns them. `oxy-app`
/// previously covered the whole sub-router with one `IdeOnly` wildcard and then
/// carved the Postgres-only reads back out by path from the outside — two lists
/// describing one thing, in two crates, free to drift.
///
/// Paths are relative to the mount point; the mounting crate prepends its prefix.
pub fn router_roles() -> &'static [RouteRoleDecl] {
    use RouteRole::{FleetOk, IdeOnly};
    &[
        // Run history is a `state.db` read. Viewing a past conversation must
        // never need the ide — that is the difference between a slow ide and an
        // unreadable product.
        RouteRoleDecl {
            method: "GET",
            path: "/threads/{thread_id}/run",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/threads/{thread_id}/runs",
            role: FleetOk,
        },
        // One UPDATE of the run's `analytics_run_extensions` row, after a
        // workspace-scope read of the run. Nothing in this process is told;
        // the thread reads above load the row back.
        RouteRoleDecl {
            method: "PATCH",
            path: "/runs/{id}/thinking_mode",
            role: FleetOk,
        },
        // One UPDATE of the run row after the same workspace-scope read and a
        // thread-owner read; when the run already ended, the host's run
        // observer writes the notice and queues its delivery as a durable
        // task. Postgres only, and asked from a phone: it must not need the ide.
        RouteRoleDecl {
            method: "POST",
            path: "/runs/{id}/notify",
            role: FleetOk,
        },
        // Ops dashboard — run history, recovery counts and queue health are
        // SELECTs over `agentic_runs`, `agentic_run_events` and
        // `agentic_task_queue`. With the Factory down these are how an
        // operator sees what the queue is doing.
        RouteRoleDecl {
            method: "GET",
            path: "/coordinator/runs",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/coordinator/recovery",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/coordinator/queue",
            role: FleetOk,
        },
        // Postgres rows, answered from the row's own status
        // (`user_facing_status`). They used to be overlaid from
        // `RuntimeState::statuses`, which is right only in the process driving
        // the run; every coordinator transition writes Postgres first, so the
        // row is never behind but between a cancel request and the driver
        // acknowledging it.
        RouteRoleDecl {
            method: "GET",
            path: "/coordinator/active-runs",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/coordinator/runs/{id}/tree",
            role: FleetOk,
        },
        // Writes Postgres and enqueues a `Global` task; a reaped airway run is
        // re-seeded through the start route's own `submit_airway_run`, so a
        // ref the promoted revision lacks is the same retryable 503.
        RouteRoleDecl {
            method: "POST",
            path: "/coordinator/runs/{id}/retry",
            role: FleetOk,
        },
        // SSE tailing the workspace's active runs in `agentic_runs`.
        RouteRoleDecl {
            method: "GET",
            path: "/coordinator/live",
            role: FleetOk,
        },
        // A chat: started, answered, cancelled and streamed by any pod. A run
        // reads the promoted revision on every role (the agent, the semantic
        // model and verified queries through the compile boundary, its context
        // root materialised from it), so none of this needs the working copy.
        // The start is queued while `queued_chat_and_workflow_starts` is on,
        // and driven in this process while it is off. An answer finds the
        // driver's channel here, or continues the run from its checkpoint; a
        // cancel is the durable flag the driver's lease polls; the stream polls
        // the event log when another process drives. A Builder start or a
        // Builder answer on a process without the files is refused with the
        // Builder route's name (`routes::run::builder_needs_the_factory`).
        RouteRoleDecl {
            method: "POST",
            path: "/runs",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/runs/{id}/answer",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/runs/{id}/cancel",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/runs/{id}/events",
            role: FleetOk,
        },
        // The Builder copilot and onboarding: they edit workspace files, and a
        // Builder run is driven in the memory of the process that started it,
        // so its start and its answers go to the Factory. Undoing a Builder
        // edit writes the working copy too.
        RouteRoleDecl {
            method: "POST",
            path: "/builder/runs",
            role: IdeOnly,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/builder/runs/{id}/answer",
            role: IdeOnly,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/runs/{id}/revert-file-changes",
            role: IdeOnly,
        },
        // A route added without a declaration of its own.
        RouteRoleDecl {
            method: "*",
            path: "/{*rest}",
            role: IdeOnly,
        },
    ]
}

/// Build the analytics sub-router.  Mount with `.nest("/analytics", router::<YourState>(state))`.
pub fn router<S>(state: Arc<AgenticState>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/runs", post(routes::create_run))
        // The same handlers, on routes the role manifest pins to the Factory:
        // what the IDE's Builder copilot and onboarding call. See `router_roles`.
        .route("/builder/runs", post(routes::create_builder_run))
        .route(
            "/builder/runs/{id}/answer",
            post(routes::answer_builder_run),
        )
        .route("/runs/{id}/events", get(routes::stream_events))
        .route("/runs/{id}/answer", post(routes::answer_run))
        .route("/runs/{id}/cancel", post(routes::cancel_run))
        .route("/runs/{id}/notify", post(routes::request_run_notice))
        .route(
            "/runs/{id}/revert-file-changes",
            post(routes::revert_file_changes),
        )
        .route(
            "/runs/{id}/thinking_mode",
            patch(routes::update_thinking_mode),
        )
        .route("/threads/{thread_id}/run", get(routes::get_run_by_thread))
        .route(
            "/threads/{thread_id}/runs",
            get(routes::list_runs_by_thread),
        )
        // Coordinator dashboard
        .route(
            "/coordinator/active-runs",
            get(coordinator::list_active_runs),
        )
        .route("/coordinator/runs", get(coordinator::list_runs))
        .route(
            "/coordinator/runs/{id}/tree",
            get(coordinator::get_run_tree),
        )
        .route("/coordinator/runs/{id}/retry", post(coordinator::retry_run))
        .route(
            "/coordinator/recovery",
            get(coordinator::get_recovery_stats),
        )
        .route("/coordinator/queue", get(coordinator::get_queue_health))
        .route("/coordinator/live", get(coordinator::live_stream))
        .layer(axum::Extension(state))
}

/// What each automation route needs. See [`router_roles`].
///
/// Mounted TWICE by `oxy-app` — at `/agentic-workflows` and at
/// `/agentic-automations` — and declared once here, which is the point: a
/// second mount cannot acquire a different, drifting classification.
pub fn automation_router_roles() -> &'static [RouteRoleDecl] {
    use RouteRole::{FleetOk, IdeOnly};
    &[
        // Postgres run history. Listing and opening a past run must survive the
        // ide being down.
        RouteRoleDecl {
            method: "GET",
            path: "/runs",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/runs/{id}",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/threads/{thread_id}/run",
            role: FleetOk,
        },
        // Starting a run, cancelling one and streaming one, from any pod. The
        // automation's YAML resolves through the compile boundary
        // (`resolve_automation_yaml` answers from the pinned revision on every
        // role), a start is queued while `queued_chat_and_workflow_starts` is
        // on and driven in this process while it is off, a cancel is the
        // durable flag, and a stream polls the event log when another process
        // drives.
        RouteRoleDecl {
            method: "POST",
            path: "/runs",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/runs/{id}/cancel",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/runs/{id}/events",
            role: FleetOk,
        },
        // The automation list and one automation's YAML, from the revision's
        // `automation_definitions` rows on a pod with no working copy.
        RouteRoleDecl {
            method: "GET",
            path: "/files",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/files/{path_b64}",
            role: FleetOk,
        },
        // A route added without a declaration of its own.
        RouteRoleDecl {
            method: "*",
            path: "/{*rest}",
            role: IdeOnly,
        },
    ]
}

/// Build the automation sub-router. Mount with `.nest("/agentic-workflows", automation_router(state))`.
///
/// Reuses the same [`AgenticState`] as the analytics router so cancellation
/// and the SSE event registry are shared. Automation runs flow through the
/// runtime coordinator + worker queue exactly like analytics runs — the only
/// thing this router does is seed the queue and surface state for the UI.
pub fn automation_router<S>(state: Arc<AgenticState>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/runs",
            post(routes::create_automation_run).get(routes::list_runs_for_automation),
        )
        .route("/runs/{id}", get(routes::get_automation_run))
        // Reuse the existing SSE handler — events are domain-routed by the
        // run's `source_type`, which is `"workflow"` for runs created here.
        .route("/runs/{id}/events", get(routes::stream_events))
        .route("/runs/{id}/cancel", post(routes::cancel_automation_run))
        .route(
            "/threads/{thread_id}/run",
            get(routes::latest_run_for_thread),
        )
        .route("/files", get(routes::list_automation_files))
        .route("/files/{path_b64}", get(routes::get_automation_file))
        .layer(axum::Extension(state))
}

/// What each airway route needs. See [`router_roles`].
///
/// One declaration per route, each with its reason, so that starting, stopping
/// and resetting a pipeline do not depend on the Factory (`OXY_ROLE=ide`) being
/// up. The test for `FleetOk` is the same for all of them: the handler reads
/// and writes Postgres only, resolves its `.airway.yml` through the compile
/// boundary (`agentic_pipeline::pipeline_ref`), and leaves nothing running in
/// this process when it returns. A request carrying `?branch=` never reaches a
/// replica whatever is declared here — `oxy-app`'s `enforce_role` escalates it
/// to the ide, the one node that can read a draft.
pub fn airway_router_roles() -> &'static [RouteRoleDecl] {
    use RouteRole::{FleetOk, IdeOnly};
    &[
        // Postgres reads: run list, coverage, and the backfill ranges the UI
        // draws. None of them touches the pipeline definition on disk.
        RouteRoleDecl {
            method: "GET",
            path: "/runs",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/coverage",
            role: FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/backfill-ranges",
            role: FleetOk,
        },
        // Start: validates the spec from the compile boundary and enqueues a
        // `Global` task for the worker fleet; drives nothing here.
        RouteRoleDecl {
            method: "POST",
            path: "/runs",
            role: FleetOk,
        },
        // Single-window backfill: the same enqueue with a window pinned on it.
        RouteRoleDecl {
            method: "POST",
            path: "/backfill",
            role: FleetOk,
        },
        // Cancel: writes the durable cancel flag the driving worker polls, and
        // decides the rest from the queue row — no in-process channel needed.
        RouteRoleDecl {
            method: "POST",
            path: "/runs/{id}/cancel",
            role: FleetOk,
        },
        // Event stream: tails `agentic_run_events` on a timer, because the
        // driver is a worker in another process whichever pod serves this.
        RouteRoleDecl {
            method: "GET",
            path: "/runs/{id}/events",
            role: FleetOk,
        },
        // Reset schema: spec from the compile boundary, state rows in
        // Postgres, and a destination drop over the network — as a worker does.
        RouteRoleDecl {
            method: "POST",
            path: "/reset-schema",
            role: FleetOk,
        },
        // Reset cursors: spec from the compile boundary, then the pipeline's
        // lease and one state row.
        RouteRoleDecl {
            method: "POST",
            path: "/reset-cursors",
            role: FleetOk,
        },
        // The names `/reset-cursors` accepts; must be reachable wherever it is.
        RouteRoleDecl {
            method: "GET",
            path: "/resource-cursors",
            role: FleetOk,
        },
        // Chunked backfill and its resume drive every chunk in THIS process, in
        // a detached task that is not a durable `TaskSpec`. A `serve` replica
        // runs no workers and the drive would die with the pod.
        RouteRoleDecl {
            method: "POST",
            path: "/chunked-backfill",
            role: IdeOnly,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/resume-backfill",
            role: IdeOnly,
        },
        // Authoring-only wizard call that dials a caller-supplied host; no
        // reason to let every public replica make that connection.
        RouteRoleDecl {
            method: "POST",
            path: "/sources/discover",
            role: IdeOnly,
        },
        // Anything not named above — a route added without a declaration —
        // stays on the ide until someone states what it needs.
        RouteRoleDecl {
            method: "*",
            path: "/{*rest}",
            role: IdeOnly,
        },
    ]
}

/// Build the airway sub-router. Mount with
/// `.nest("/agentic-airway", airway_router(state))`.
///
/// Shares [`AgenticState`] with the analytics + automation routers so
/// cancellation and the SSE event registry are common. Airway runs go
/// through the same coordinator + worker queue as automation runs; this
/// router only seeds the queue and exposes cancel. Events reuse the
/// domain-agnostic `stream_events` handler — they're routed by the
/// run's `source_type` (`"airway"`).
pub fn airway_router<S>(state: Arc<AgenticState>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/runs",
            post(routes::create_airway_run).get(routes::list_runs_for_pipeline),
        )
        // Bounded date-window backfill (toast, quickbooks, sp_api — see
        // `executor::WINDOWED_BACKFILL_KINDS`): seeds a run with the window
        // pinned on the source.
        .route("/backfill", post(routes::backfill_airway))
        // Resumable chunked backfill: splits a long window into checkpointed
        // chunks and drives them detached; `/coverage` reports progress.
        .route("/chunked-backfill", post(routes::chunked_backfill))
        // Resume: re-run only a range's not-`done` chunks (no window needed).
        .route("/resume-backfill", post(routes::airway_resume))
        // List a pipeline's backfill ranges (the gantt); per-range coverage.
        .route("/backfill-ranges", get(routes::airway_backfill_ranges))
        .route("/coverage", get(routes::airway_coverage))
        // Reuse the domain-agnostic SSE handler.
        .route("/runs/{id}/events", get(routes::stream_events))
        .route("/runs/{id}/cancel", post(routes::cancel_airway_run))
        // Live source introspection for the New Pipeline table picker.
        .route("/sources/discover", post(routes::discover_source_tables))
        // Drop a pipeline's destination tables + clear its stored schema/cursor
        // so a later run re-infers a fresh schema.
        .route("/reset-schema", post(routes::reset_airway_schema))
        // Rewind the cursors and NOTHING else, so a resource can be re-pulled
        // from an earlier `default_start` without the pipeline's history being
        // the price. Refuses where a re-pull would duplicate rather than
        // converge; `force` overrides.
        .route("/reset-cursors", post(routes::reset_airway_cursors))
        // The resource names `/reset-cursors` accepts, so a caller offers them
        // rather than asking for them — a name guessed from a run's lineage is
        // the table name, which diverges from the cursor key often enough that
        // a reset scoped to it would silently clear nothing.
        .route("/resource-cursors", get(routes::airway_resource_cursors))
        .layer(axum::Extension(state))
}

#[cfg(test)]
#[path = "airway_role_tests.rs"]
mod airway_role_tests;

#[cfg(test)]
#[path = "analytics_role_tests.rs"]
mod analytics_role_tests;

// The schedule routes were relocated to the `app` crate (§12 FU4b):
// they require `WorkspaceAdmin` from `crate::api::middlewares::role_guards`
// which lives above agentic-http in the layer stack. See
// `crates/app/src/server/api/schedules.rs` and
// `crates/app/src/server/router/workspace.rs::build_schedule_routes`.
