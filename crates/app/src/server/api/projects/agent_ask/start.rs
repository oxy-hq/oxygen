//! The two ways an ask's run starts: queued for any driver on the fleet, or —
//! on an app's staging host — driven in the request's own process.
//!
//! **A live ask is queued** ([`queue_ask`]). The request checks the agent can
//! start, writes the run, its extension row and its `queued` Global entry in
//! one transaction, and returns. Whichever driver claims the entry runs
//! [`super::executor::AgentAskExecutor`], which builds the caller's context
//! from the record written here and starts the pipeline
//! [`super::pipeline::ask_pipeline`] defines for both paths. The replica that
//! took the request can go away without the ask going with it: a driver that
//! dies mid-run leaves an ask the next claim resumes from its checkpoint or
//! closes as interrupted, never one that starts again from the top.
//!
//! **A staging ask is driven here** ([`drive_here`]). Its platform holds every
//! write inside the request's process (`previews::request_hold`), and recovery
//! retires a run carrying the preview stamp at its stamp check, before any
//! executor. Moving it would mean rebuilding the hold on the driver.
//!
//! What a bundle sees differ between the two only where the queued one is
//! better or later: its first event about one latency-worker poll (1 s) later;
//! a failure after the run exists (no model, a bad connector) as a terminal
//! `error` on the stream instead of a `500` on the start; and an `error`
//! saying "interrupted — ask again" where a dead handler-driven drive used to
//! leave the stream open for ever.

use std::sync::Arc;

use agentic_pipeline::PipelineError;
use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::queued_run::{
    QueuedAnalyticsRun, check_analytics_agent, seed_queued_analytics_run,
};
use axum::http::StatusCode;
use axum::response::Response;
use sentry::SentryFutureExt;
use tokio::sync::{mpsc, watch};
use tracing::{error, warn};
use uuid::Uuid;

use super::{AskRequest, boxed, caller, err, err_with_hint, pipeline, task};
use crate::server::api::custom_apps_gates::CustomAppContext;
use crate::server::previews::request_hold::{HoldScope, scope_if};

/// Queue a live ask: check its agent can start, then seed the run and its
/// entry. Returns the run id. Registers nothing in this process — see
/// `agentic_pipeline::queued_run` for why the submitter must not.
pub(super) async fn queue_ask(
    db: &sea_orm::DatabaseConnection,
    platform: &Arc<dyn PlatformContext>,
    gates_ctx: &CustomAppContext,
    req: &AskRequest,
    thread_id: Uuid,
    agent_id: &str,
) -> Result<String, Response> {
    if let Err(e) = check_analytics_agent(platform, agent_id).await {
        warn!(%agent_id, error = %e, "custom-app ask: the agent cannot start");
        return Err(start_failed(agent_id, &e));
    }
    let run = QueuedAnalyticsRun {
        run_id: Uuid::new_v4().to_string(),
        workspace_id: gates_ctx.workspace.id,
        agent_id: agent_id.to_string(),
        question: req.question.clone(),
        thread_id: Some(thread_id),
        thinking_mode: Default::default(),
        metadata: [(
            caller::RUN_CALLER_KEY.to_string(),
            caller::RunCaller::of(gates_ctx).to_metadata(),
        )]
        .into_iter()
        .collect(),
        spec: task::spec(),
    };
    if let Err(e) = seed_queued_analytics_run(db, &run).await {
        error!(error = %e, "custom-app ask: could not queue the run");
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not start the ask",
        ));
    }
    Ok(run.run_id)
}

/// Drive a staging ask in this process, inside its hold, as every ask was
/// driven before asks were queued. Returns the run id.
pub(super) async fn drive_here(
    agentic_state: &Arc<agentic_http::AgenticState>,
    platform: Arc<dyn PlatformContext>,
    gates_ctx: &CustomAppContext,
    req: &AskRequest,
    thread_id: Uuid,
    agent_id: &str,
    hold: HoldScope,
) -> Result<String, Response> {
    // The caller is recorded on the run in the insert that creates it:
    // whatever drives this run later without this request rebuilds the
    // context from that record instead of running it on its own subject-less
    // platform — see `caller`.
    let builder = pipeline::ask_pipeline(
        platform.clone(),
        gates_ctx.workspace.id,
        &req.question,
        Some(thread_id),
        Some(Arc::clone(&agentic_state.schema_cache)),
        agent_id,
    )
    .run_metadata(
        caller::RUN_CALLER_KEY,
        caller::RunCaller::of(gates_ctx).to_metadata(),
    );

    // Inside the hold, so the run row is stamped.
    let started = match boxed(|| scope_if(Some(hold), builder.start(&agentic_state.db))).await {
        Ok(s) => s,
        Err(e) => {
            warn!(%agent_id, error = %e, "custom-app ask: pipeline start failed");
            return Err(start_failed(agent_id, &e.source));
        }
    };
    let run_id = started.run_id.clone();

    // `cancel_tx` is the live wire — `cancel::cancel_ask` signals through this
    // watch channel (`RuntimeState::cancel`) to abort the coordinator's drive
    // loop on the same instance. A cancel that lands on ANOTHER replica reaches
    // it within about 5 s through the durable flag, which the drive's
    // driver-lease ticker polls (`agentic_runtime::lifecycle::driver_lease`).
    //
    // `answer_tx` is registered but the custom-app surface exposes no
    // `/answer` endpoint — clarification answers go through `useAgentRun`'s
    // re-`ask()` (new run, same thread_id). The drive path still takes the
    // receiver, so the channel must exist.
    let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    agentic_state.register(&run_id, answer_tx, cancel_tx);

    let db = agentic_state.db.clone();
    let runtime_state = agentic_state.runtime.clone();
    let schema_cache = Some(agentic_state.schema_cache.clone());
    let builder_test_runner = agentic_state.builder_test_runner.clone();
    let builder_app_runner = agentic_state.builder_app_runner.clone();
    let router = agentic_state.router.clone();
    let run_id_for_drive = run_id.clone();
    // Built in `boxed`'s frame too: the drive's future is a large one.
    tokio::spawn(boxed(|| {
        async move {
            agentic_pipeline::drive_with_coordinator(
                started,
                db,
                runtime_state,
                answer_rx,
                cancel_rx,
                platform,
                None, // no builder bridges — analytics-only surface
                schema_cache,
                builder_test_runner,
                builder_app_runner,
                router,
            )
            .await;
            tracing::debug!(run_id = %run_id_for_drive, "custom-app ask: drive finished");
        }
        // The drive outlives the 202 this handler returns, so the request's hub
        // is gone by the time the pipeline runs. It carries the bundle's prompt
        // and the warehouse errors answering it, and nothing under
        // `agentic_pipeline` has a `custom_apps` target for barrier 1 to read,
        // so the tag has to travel with it (`middlewares::sentry_surface`).
        .bind_hub(sentry::Hub::current())
    }));
    Ok(run_id)
}

/// The response to a start that could not begin: `agent_not_found` (`404`)
/// with a hint where to look, `503` for a start that succeeds later (not
/// compiled yet), `500` otherwise. The same answer whichever path started.
fn start_failed(agent_id: &str, e: &PipelineError) -> Response {
    let msg = e.to_string();
    if msg.contains("not found") || msg.contains("No such file") {
        return err_with_hint(
            StatusCode::NOT_FOUND,
            msg,
            "agent_not_found",
            format!(
                "Expected `{agent_id}.agentic.yml` in the project root. \
                 Check the agent id matches a file under your workspace; \
                 agentic agents end in `.agentic.yml`, classic `.agent.yml` \
                 agents aren't reachable from custom-app bundles."
            ),
        );
    }
    let status = if e.is_retryable() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    err(status, msg)
}
