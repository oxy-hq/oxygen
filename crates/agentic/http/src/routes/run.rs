//! Run lifecycle handlers: create, stream, answer, cancel, update thinking mode.

use std::sync::Arc;

use agentic_runtime::hub_task::spawn_with_hub;
use axum::{
    Json,
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use agentic_pipeline::PipelineBuilder;
use agentic_pipeline::platform::{BuilderBridges, PlatformContext, Requester};
use agentic_pipeline::{AutoAcceptInputProvider, BuilderLlmMetadata, LlmClient, OpenAiProvider};
use oxy_auth::extractor::AuthenticatedUserExtractor;

use crate::{
    db, sse,
    state::{AgenticState, RunStatus},
};

use super::run_scope::{ensure_thread_access, run_in_workspace};
use super::{AnswerRequest, CreateRunRequest, CreateRunResponse, RunIdPath, ThinkingMode};

/// Cap on the number of tables an onboarding request may supply — guards
/// against pathological LLM prompts and request-size blowups.
const MAX_ONBOARDING_TABLES: usize = 50;

/// Construct the onboarding-flow `LlmClient`. Used at create time and on
/// cold resume so both code paths produce a byte-identical client.
async fn build_onboarding_llm_client(
    platform: &dyn PlatformContext,
    vendor: &str,
    model_ref: &str,
    key_var: &str,
) -> LlmClient {
    let api_key = platform.resolve_secret(key_var).await.unwrap_or_default();
    if vendor == "openai" {
        LlmClient::with_provider(OpenAiProvider::new(&api_key, model_ref))
    } else {
        LlmClient::with_model(api_key, model_ref.to_string())
    }
}

/// Where a Builder or onboarding run starts, relative to the analytics mount:
/// the route the role manifest pins to the Factory
/// ([`crate::router_roles`]). The customer route `POST /runs` names it when it
/// refuses one on a process without the working copy.
pub const BUILDER_START_ROUTE: &str = "/analytics/builder/runs";
/// Where an answer to a Builder run goes, for the same reason: the run is
/// driven in the Factory's memory, and only a process with the working copy
/// may resume it.
pub const BUILDER_ANSWER_ROUTE: &str = "/analytics/builder/runs/{id}/answer";

/// The header every refusal that only the Factory can serve carries, as the
/// role middleware's own misroute answer does.
const HEADER_REQUIRED_ROLE: &str = "x-oxy-required-role";

/// Which of the two routes a start or an answer arrived on. The handlers are
/// shared; what they accept is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunRoute {
    /// `POST /runs`, `POST /runs/{id}/answer`: a customer's chat. On the customer
    /// list, so the Factory gate lets every member through, and it never starts or
    /// answers a Builder run, on any process.
    Chat,
    /// `POST /builder/runs`, `POST /builder/runs/{id}/answer`: on the Factory list
    /// (the gate decides who), pinned to the Factory by the role manifest.
    Builder,
}

/// Whether to refuse a Builder start: always on the chat route, which the
/// Factory gate does not cover (a plain member would otherwise start a run that
/// edits workspace files), and on the Builder route only where the files are not.
fn refuses_builder_start(route: RunRoute, is_builder: bool, holds_working_copy: bool) -> bool {
    is_builder && (route == RunRoute::Chat || !holds_working_copy)
}

/// Whether an answer to a run whose drive is in another process's memory must
/// not start a second drive here: the run's lease is live and not held by a
/// drive in this process. Its own driver takes the answer through its channel.
fn driven_elsewhere(
    driver_id: Option<&str>,
    heartbeat_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    held_here: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let live = match (driver_id, heartbeat_at) {
        (Some(_), Some(at)) => {
            now.signed_duration_since(at)
                < chrono::Duration::seconds(agentic_runtime::crud::DRIVER_LEASE_TTL_SECS)
        }
        _ => false,
    };
    live && !held_here
}

/// Whether a start is a Builder run: the Builder domain, or onboarding (which
/// runs the Builder with its own prompt and tool list). Both edit workspace
/// files.
fn starts_a_builder_run(body: &CreateRunRequest) -> bool {
    body.domain.as_deref() == Some("builder") || body.onboarding_context.is_some()
}

/// Whether a run of this `source_type` may be resumed only by a process that
/// holds the working copy: a Builder run (the Builder copilot, onboarding).
fn resumes_only_with_the_files(source_type: Option<&str>) -> bool {
    source_type == Some("builder")
}

/// The answer to a Builder start or a Builder answer on the chat routes (on any
/// process), or on the Builder routes where the working copy is not:
/// `421 Misdirected Request`, as the role middleware answers a route this process
/// cannot serve, naming the route that reaches the Factory. Never a run against
/// files that are not here, and never one the Factory gate did not admit.
fn builder_needs_the_factory(what: &str, route: &str) -> Response {
    let mut response = (
        StatusCode::MISDIRECTED_REQUEST,
        Json(serde_json::json!({
            "error": format!(
                "{what} edits workspace files; send it to {route}, which the Factory serves"
            ),
            "route": route,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        HEADER_REQUIRED_ROLE,
        axum::http::HeaderValue::from_static("ide"),
    );
    response
}

/// Build a structured error response for a failed pipeline start/resume.
///
/// The body is `{ "error": <human message> }` so the frontend can surface
/// the real cause (e.g. a broken semantics file) instead of a bare status
/// code. Configuration / semantic-model failures are reworded into a
/// user-facing sentence; everything else passes through verbatim.
fn classify_pipeline_error_message(raw: &str) -> String {
    if raw.contains("semantic") || raw.contains("ConfigError") || raw.contains("config error") {
        format!(
            "Your project configuration is invalid, so this run could not start: {raw}. \
             Fix the reported file (often a broken .view.yml / .topic.yml semantic \
             definition) and try again."
        )
    } else {
        raw.to_string()
    }
}

/// `503` for a start the caller can simply retry — the workspace is not
/// compiled yet, or the compile boundary did not answer — and `400` for
/// everything else. A `400` tells a client the request was wrong; for a
/// pod that could not read its inputs, it was not.
fn start_failure_status(error: &agentic_pipeline::PipelineError) -> StatusCode {
    if error.is_retryable() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::BAD_REQUEST
    }
}

/// `run_id` is included when the failed run was persisted, so the frontend
/// can reconcile its live failed state with the run that appears in thread
/// history (otherwise it renders the question + error twice).
fn pipeline_error_response(
    status: StatusCode,
    err: impl std::fmt::Display,
    run_id: Option<&str>,
) -> Response {
    let message = classify_pipeline_error_message(&err.to_string());
    let mut body = serde_json::json!({ "error": message });
    if let Some(rid) = run_id {
        body["run_id"] = serde_json::Value::String(rid.to_string());
    }
    (status, Json(body)).into_response()
}

/// `POST /runs`: a customer's chat. Never a Builder run (see [`RunRoute`]).
pub async fn create_run(
    state: Extension<Arc<AgenticState>>,
    platform: Extension<Arc<dyn PlatformContext>>,
    bridges: Extension<BuilderBridges>,
    query: axum::extract::RawQuery,
    user: AuthenticatedUserExtractor,
    body: Json<CreateRunRequest>,
) -> Response {
    create_run_on(RunRoute::Chat, state, platform, bridges, query, user, body).await
}

/// `POST /builder/runs`: the Builder copilot and onboarding.
pub async fn create_builder_run(
    state: Extension<Arc<AgenticState>>,
    platform: Extension<Arc<dyn PlatformContext>>,
    bridges: Extension<BuilderBridges>,
    query: axum::extract::RawQuery,
    user: AuthenticatedUserExtractor,
    body: Json<CreateRunRequest>,
) -> Response {
    create_run_on(
        RunRoute::Builder,
        state,
        platform,
        bridges,
        query,
        user,
        body,
    )
    .await
}

async fn create_run_on(
    route: RunRoute,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    Extension(bridges): Extension<BuilderBridges>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<CreateRunRequest>,
) -> Response {
    tracing::info!(
        agent_id = %body.agent_id,
        domain = ?body.domain,
        thread_id = ?body.thread_id,
        "create_run: received request"
    );

    // A Builder run edits the working copy and is driven in this process's
    // memory. The chat route never starts one (the Factory gate does not cover
    // it), and a process without the files (a serve replica) refuses it on the
    // Builder route too; both before anything is written, naming the route the
    // Factory serves.
    if refuses_builder_start(
        route,
        starts_a_builder_run(&body),
        (state.holds_working_copy)(),
    ) {
        tracing::info!(
            domain = ?body.domain,
            onboarding = body.onboarding_context.is_some(),
            "create_run: a Builder start reached a process without the working copy"
        );
        return builder_needs_the_factory("A Builder or onboarding run", BUILDER_START_ROUTE);
    }

    if let Some(ctx) = &body.onboarding_context
        && ctx.tables.len() > MAX_ONBOARDING_TABLES
    {
        return (
            StatusCode::BAD_REQUEST,
            format!("onboarding_context.tables exceeds limit of {MAX_ONBOARDING_TABLES} entries"),
        )
            .into_response();
    }

    let db = state.db.clone();

    let mut thread_id_uuid = body
        .thread_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());

    // The pipeline reads this thread's history into the run and appends the
    // run to it: the thread must be in this workspace and not another user's.
    if let Some(tid) = thread_id_uuid
        && let Err(refused) =
            ensure_thread_access(&state, &user.id, tid, platform.workspace_id()).await
    {
        return refused;
    }

    // If onboarding context is provided, build the prompt server-side.
    // Otherwise use the raw question from the request.
    let question = if let Some(ctx) = &body.onboarding_context {
        ctx.build_prompt()
    } else {
        body.question.clone()
    };

    // Auto-provision a thread when an interactive run is started without
    // one. Persisting a real `threads` row (a) satisfies the
    // `agentic_runs.thread_id` FK and (b) hands the client a stable id it
    // can reuse for follow-up questions so the agent keeps conversation
    // context. Onboarding / builder runs manage their own threading, so
    // they are excluded.
    if thread_id_uuid.is_none()
        && body.domain.as_deref() != Some("builder")
        && body.onboarding_context.is_none()
    {
        let title: String = question.chars().take(120).collect();
        match state
            .thread_owner
            .create_thread(platform.workspace_id(), &title)
            .await
        {
            Ok(tid) => thread_id_uuid = Some(tid),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "create_run: failed to auto-create thread; starting run unlinked"
                );
            }
        }
    }

    // A plain chat is started on the queue: seeded here, driven by whichever
    // driver claims it, so this replica may go away without taking the run
    // with it. Which starts qualify, and why the rest stay here:
    // `agentic_pipeline::queued_run::ChatAdmission`. Only while the host turns
    // queued starts on (`AgenticState::queued_starts`): until then every chat is
    // driven here, as before.
    let admission = agentic_pipeline::queued_run::ChatAdmission {
        builder_domain: body.domain.as_deref() == Some("builder"),
        onboarding: body.onboarding_context.is_some(),
        auto_accept: body.auto_accept,
        working_copy_branch: branch_in_query(query.as_deref()),
    };
    let queued_for = if (state.queued_starts)() {
        admission.queueable(&*platform)
    } else {
        None
    };
    if let Some(requester) = queued_for {
        let chat = agentic_pipeline::queued_run::ChatStart {
            workspace_id: platform.workspace_id(),
            agent_id: body.agent_id.clone(),
            question: question.clone(),
            thread_id: thread_id_uuid,
            thinking_mode: body.thinking_mode,
        };
        return match agentic_pipeline::queued_run::queue_chat(&db, &platform, chat, requester).await
        {
            Ok(run_id) => Json(CreateRunResponse {
                run_id,
                thread_id: thread_id_uuid.map(|u| u.to_string()),
            })
            .into_response(),
            Err(agentic_pipeline::queued_run::QueueChatError::Agent(e)) => {
                tracing::error!(agent_id = %body.agent_id, error = %e, "create_run: the agent cannot start");
                pipeline_error_response(start_failure_status(&e), &e, None)
            }
            Err(agentic_pipeline::queued_run::QueueChatError::Seed(e)) => {
                tracing::error!(error = %e, "create_run: could not queue the chat");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "could not start the run" })),
                )
                    .into_response()
            }
        };
    }

    // Onboarding phases declare which reference cards their prompt
    // needs pre-populated; interactive (no onboarding context) runs
    // default to no cards and rely on the `lookup_reference` tool.
    let knowledge_cards = body
        .onboarding_context
        .as_ref()
        .map(|ctx| ctx.knowledge_cards())
        .unwrap_or_default();

    // Onboarding also declares a per-phase tool allowlist (drops dbt /
    // search_text / run_tests etc.) and skips the Interpreting LLM
    // call (the UI surfaces CTAs in place of the synthesized summary).
    let tool_allowlist = body
        .onboarding_context
        .as_ref()
        .map(|ctx| ctx.tool_allowlist());
    let skip_interpreting = body.onboarding_context.is_some();

    let mut builder = PipelineBuilder::new(platform.clone())
        .workspace_id(platform.workspace_id())
        .with_builder_bridges(bridges.clone())
        .question(&question)
        .thinking_mode(body.thinking_mode)
        .schema_cache(Arc::clone(&state.schema_cache))
        .knowledge_cards(knowledge_cards)
        .skip_interpreting(skip_interpreting);
    // Who asked, on the run's own insert: whatever drives this run later
    // without this request (recovery after a restart, a queued start's driver)
    // builds the requester's context from it instead of running the root on
    // its own subject-less one.
    if let Some(requester) = platform.requester() {
        builder = builder.run_metadata(Requester::KEY, requester.to_metadata());
    }

    if let Some(allowlist) = tool_allowlist {
        builder = builder.tool_allowlist(allowlist);
    }

    if let Some(tid) = thread_id_uuid {
        builder = builder.thread(tid);
    }
    if let Some(runner) = state.builder_test_runner.clone() {
        builder = builder.test_runner(runner);
    }

    // Onboarding auto-accepts all file_change tool calls — no HITL.
    if body.auto_accept {
        builder = builder.human_input(Arc::new(AutoAcceptInputProvider));
    }

    // During onboarding the chosen model may not be in config.yml yet (the
    // builder agent is about to write it). When onboarding_context carries a
    // model_config, build the LlmClient directly and override the pipeline's
    // default resolution. Also persist the (vendor, model_ref, key_var)
    // tuple via with_builder_llm_metadata so a cold resume (server restart
    // mid-onboarding) can rebuild an identical client.
    if let Some(mc) = body
        .onboarding_context
        .as_ref()
        .and_then(|ctx| ctx.model_config.as_ref())
    {
        let client =
            build_onboarding_llm_client(&*platform, &mc.vendor, &mc.model_ref, &mc.key_var).await;
        builder = builder
            .with_builder_llm_client(client)
            .with_builder_llm_metadata(BuilderLlmMetadata {
                vendor: mc.vendor.clone(),
                model_ref: mc.model_ref.clone(),
                key_var: mc.key_var.clone(),
            });
    }

    builder = if body.domain.as_deref() == Some("builder") {
        builder.builder(body.model.clone())
    } else {
        builder.analytics(&body.agent_id)
    };

    let started = match builder.start(&db).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                agent_id = %body.agent_id,
                domain = ?body.domain,
                thread_id = ?body.thread_id,
                error = %e,
                "create_run: pipeline start failed"
            );
            return pipeline_error_response(
                start_failure_status(&e.source),
                &e,
                e.run_id.as_deref(),
            );
        }
    };

    let run_id = started.run_id.clone();

    let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state.register(&run_id, answer_tx, cancel_tx);

    let runtime_state = state.runtime.clone();
    let schema_cache = Some(state.schema_cache.clone());
    let builder_test_runner = state.builder_test_runner.clone();
    let router = state.router.clone();
    spawn_with_hub(async move {
        agentic_pipeline::drive_with_coordinator(
            started,
            db,
            runtime_state,
            answer_rx,
            cancel_rx,
            platform,
            Some(bridges),
            schema_cache,
            builder_test_runner,
            router,
        )
        .await;
    });

    Json(CreateRunResponse {
        run_id,
        // Return the resolved thread id — the one the client supplied, or
        // the one we just auto-created — so the client can thread follow-ups.
        thread_id: thread_id_uuid.map(|u| u.to_string()),
    })
    .into_response()
}

/// The `branch` a start's query names, if any. The workspace middleware builds
/// the request's context on that branch's working copy and revision, and a
/// driver elsewhere would rebuild it on the promoted one — so a branch-scoped
/// start is not queued (`queued_run::ChatAdmission::working_copy_branch`).
pub fn branch_in_query(query: Option<&str>) -> Option<String> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('=').or(Some((pair, ""))))
        .find(|(key, _)| *key == "branch")
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
}

// ── GET /runs/:id/events (SSE) ────────────────────────────────────────────────

/// Synthesize a terminal SSE event from the run row's authoritative
/// `task_status`.
///
/// The SSE contract is that the stream closes only after a terminal event
/// (`done`/`error`/`cancelled`). That contract used to depend on *some*
/// code path remembering to persist a terminal event row — which it does
/// not when a run dies *before* the orchestrator loop starts (e.g. a
/// broken semantics file fails solver construction). The single source of
/// truth for run lifecycle is the `task_status` column, so when a stream
/// is about to close without having emitted a terminal event we derive
/// one from the run row instead of letting the client hang.
///
/// Returns `None` when the run is not (yet) terminal — the caller should
/// keep the stream open.
/// Pure mapping from a run row's terminal `task_status` to the SSE event
/// type + payload the frontend already knows how to render. Returns `None`
/// for non-terminal statuses (the stream should stay open). Split out from
/// the DB fetch so it can be unit-tested without a database.
pub fn terminal_event_for_status(
    task_status: Option<&str>,
    error_message: Option<&str>,
    answer: Option<&str>,
    run_id: &str,
) -> Option<(&'static str, serde_json::Value)> {
    match task_status? {
        "failed" | "timed_out" => Some((
            "error",
            serde_json::json!({
                "message": error_message
                    .unwrap_or("The run failed before it could report an error."),
                "trace_id": run_id,
            }),
        )),
        "cancelled" => Some(("cancelled", serde_json::json!({ "trace_id": run_id }))),
        "done" => Some((
            "done",
            serde_json::json!({
                "answer": answer.unwrap_or_default(),
                "trace_id": run_id,
            }),
        )),
        // running / awaiting_input / delegating — not terminal yet.
        _ => None,
    }
}

/// How long [`stream_events`] will park on the in-process notifier before
/// re-reading `agentic_run_events` anyway.
///
/// **The notifier is not a sufficient wake source, because the driver is often
/// in a different process.** `RuntimeState.notifiers` is a per-process
/// `DashMap<run_id, Notify>` and `state.notify(..)` rings only this process's
/// copy. Since airway submits enqueue `TaskScope::Global`
/// (`routes/airway.rs::start_and_drive`), the pod that accepted the submit —
/// and therefore serves this SSE stream, the route being `IdeOnly` — is
/// usually *not* the pod driving the run. The driver writes rows to
/// `agentic_run_events` in Postgres and rings a notifier we cannot see.
///
/// Parking on the notifier alone therefore hangs the stream: the client gets
/// whatever was already written at the first poll and then nothing, while
/// `still_active` stays true forever (only an in-process `deregister` clears
/// the entry, and the driver's process is the one that calls it). That breaks
/// the invariant that every run stream ends in `done`/`error`/`cancelled`.
///
/// A timer costs one indexed `get_events_after` per stream per interval and
/// needs no new infrastructure. Per-run LISTEN/NOTIFY was the alternative and
/// is rejected for the reasons `adr-postgres-as-worker-queue.md` gives (§1,
/// §3): a second permanent LISTEN connection per pod is a horizontal-scale cap
/// and a PgBouncer footgun. #2823 rejected it on the same grounds for the
/// world-model bus and tailed a table instead.
///
/// The notifier stays as the fast path: a locally-driven run (`OXY_ROLE=all`,
/// or any run this process drives) still wakes instantly, so this interval is
/// the added latency only when the driver is remote.
const REMOTE_DRIVER_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// The backed-off interval used once a stream has been quiet for
/// [`IDLE_TICKS_BEFORE_BACKOFF`].
///
/// A run that is `awaiting_input` is not terminal and deliberately never times
/// out (`SuspendedHuman`), so its stream can legitimately stay open for hours.
/// At the fast interval that is two indexed queries a second, per open
/// browser, for the whole wait — a real cost on a handler shared by every
/// domain, not a hypothetical one.
///
/// Backing off is safe here precisely because it only engages after a long
/// quiet stretch: an active pipeline emits events continuously and resets to
/// the fast interval, and a run waiting to be claimed is picked up in
/// well under the idle threshold, so neither case ever reaches this value.
const IDLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Consecutive event-free polls before [`IDLE_POLL_INTERVAL`] takes over.
///
/// Sized to be far longer than any realistic wait for a driver to claim a
/// freshly-enqueued run, so the back-off can never be what makes a pipeline
/// look slow to start. A worker claims in ~300 ms; the ungated periodic
/// stranded tick is the slow path at ~30 s. 90 s clears both.
const IDLE_TICKS_BEFORE_BACKOFF: u32 = 90;

/// Ask the run row on one in every N **timed** wakes, not on all of them.
///
/// ## Why this is paced rather than free
///
/// `idle_timeout` cannot reap a connection any poller keeps touching — sqlx
/// holds idle connections in a FIFO `ArrayQueue` and `release()` restamps
/// `idle_since`, so a sequential caller round-robins the whole idle set and
/// resets the very clock the reaper measures. The threshold is
/// `N_idle / idle_timeout` acquires per second: **0.27/s** at the default
/// ceiling of 80, and **0.13/s** at the ceiling of 40 the incident review
/// recommends. That is the standing condition behind the 189-connection
/// plateau on oxy-prod (2026-09-01) — six hours of 30 s stalls and ~2,000
/// refused connections an hour — and it is still live, so anything that adds a
/// steady poll has to be sized against it rather than assumed cheap.
///
/// Before this change an idle SSE stream parked on the notifier and issued
/// **zero** acquires. It cannot stay that way and still serve a run driven in
/// another process, so the budget is spent deliberately — and only where it
/// buys something. **Both** queries count: `get_events_after` runs on every
/// wake, `get_run` on one timed wake in `N`.
///
/// | stream | interval | acquires/sec |
/// | --- | --- | --- |
/// | notifier present, notified wake | — | 0 |
/// | notifier present, non-airway, quiet | 30 s | 0.033 + 0.007 = **0.04** |
/// | airway, or no notifier — active | 1 s | 1 + 0.2 = **1.2** |
/// | either, quiet ≥ `IDLE_TICKS_BEFORE_BACKOFF` | 30 s | **0.04** |
///
/// The fast tier is keyed on *whether the timer is the wake source*, not on
/// `source_type` alone: a stream holding no notifier has nothing that can ring
/// it, whatever kind it is. `notifier` is snapshotted once before the loop and
/// never re-read, so that state is permanent for the stream's life — see the
/// gate at the park.
///
/// An earlier version of this table read `driver local … ~0` and was wrong: it
/// counted only `get_run`, and `timed_wakes` not advancing on a notified wake
/// gates that query alone. `get_events_after` fires on every wake regardless,
/// so before `driver_may_be_remote` existed an analytics stream paid ~1.2/s
/// through every LLM call and warehouse query — gaps long enough to time out
/// but short enough to keep resetting `idle_polls`, so it never reached the
/// back-off. That is the largest number in the table, on the pod this change
/// exists to relieve, for the source types that do not need a timer at all.
///
/// Hence the gate: only a kind whose driver can be in another process pays the
/// fast interval. Everything else sits at 30 s, where the notifier is doing
/// the real work and the timer is a safety net.
///
/// The remaining 1.2/s applies to airway streams only, is bounded by the run's
/// duration, and decays to 0.04/s after `IDLE_TICKS_BEFORE_BACKOFF`.
const ROW_CHECK_EVERY_N_TIMED_WAKES: u32 = 5;

/// Why [`stream_events`] stopped parking.
enum Wake {
    /// The in-process driver flushed events — re-read immediately.
    Notified,
    /// Nothing rang us within [`REMOTE_DRIVER_POLL_INTERVAL`]. Either the run
    /// is idle, or its driver is in another process. Re-read either way.
    Timeout,
    Shutdown,
}

/// Is the run row itself terminal?
///
/// The backstop for a remotely-driven run: `still_active` keys on this
/// process's notifier map, which a remote driver never clears, so the run row
/// is the only authority this process has for "nothing more is coming".
/// Errs toward `false` — a failed lookup means "keep streaming", never a
/// premature close.
async fn run_row_is_terminal(db: &sea_orm::DatabaseConnection, run_id: &str) -> bool {
    matches!(
        db::get_run(db, run_id).await,
        Ok(Some(run)) if status_is_terminal(run.task_status.as_deref())
    )
}

/// Is this `task_status` one that means nothing more is coming?
///
/// Pulled out of [`run_row_is_terminal`] so it can be asserted without a
/// database, because it is now load-bearing in a way it was not before: this
/// predicate is the **only** thing keeping a stream open for a run driven in
/// another process. A missing notifier no longer closes a stream, so widening
/// this set — adding `awaiting_input`, say, which looks idle and is not —
/// would close HITL and in-flight streams early, emitting a synthesized
/// terminal event for a run that is still going. Narrowing it strands the
/// stream open instead.
fn status_is_terminal(status: Option<&str>) -> bool {
    matches!(
        status,
        Some("done") | Some("failed") | Some("cancelled") | Some("timed_out")
    )
}

async fn synth_terminal_event(
    db: &sea_orm::DatabaseConnection,
    run_id: &str,
    seq: i64,
) -> Option<SseEvent> {
    let run = db::get_run(db, run_id).await.ok().flatten()?;
    let (event_type, payload) = terminal_event_for_status(
        run.task_status.as_deref(),
        run.error_message.as_deref(),
        run.answer.as_deref(),
        run_id,
    )?;
    Some(
        SseEvent::default()
            .id(seq.to_string())
            .event(event_type)
            .data(payload.to_string()),
    )
}

pub async fn stream_events(
    Path(RunIdPath { id: run_id }): Path<RunIdPath>,
    headers: HeaderMap,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
) -> Response {
    // The event log carries the run's payloads — LLM output, tool and SQL
    // results — so it is the workspace's to read, and no one else's.
    if let Err(resp) = run_in_workspace(&state.db, &run_id, platform.workspace_id()).await {
        return resp;
    }
    let last_seq = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(-1);

    let notifier = state.notifiers.get(&run_id).map(|n| Arc::clone(&*n));
    let run_id = run_id.clone();

    let db = state.db.clone();

    let source_type = db::get_run(&db, &run_id)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.source_type)
        .unwrap_or_else(|| "analytics".to_string());

    // Can this run's driver be in another process? Only then does the timer
    // below need to be fast.
    //
    // Airway is the only kind enqueued `TaskScope::Global` from an interactive
    // submit, so it is the only kind whose events can be written by a pod that
    // cannot ring this one's notifier. Everything else — analytics, builder,
    // automation — direct-drives, so its notifier IS authoritative and a fast
    // timer buys nothing while costing a `get_events_after` per second through
    // every LLM call and warehouse query. That is most open streams, and on
    // `main` they parked on the notifier at zero acquires; keeping them there
    // is the whole point of this flag.
    //
    // ⚠️ MAINTENANCE HAZARD: a kind that moves to `TaskScope::Global` must be
    // added here, or its stream silently inherits the 30 s interval and looks
    // like a slow UI with no error anywhere. There is no compile-time link
    // between the enqueue scope and this list — if you are moving a submit to
    // `Global`, this is the second place to change.
    let driver_may_be_remote = source_type == agentic_runtime::coordinator::AIRWAY_SOURCE_TYPE;

    let registry = Arc::clone(&state.event_registry);

    let stream = async_stream::stream! {
        let mut last_sent_seq = last_seq;
        let mut processor = registry.stream_processor(&source_type);
        // Tracks whether a terminal event (`done`/`error`/`cancelled`) was
        // streamed. If the run ends without one, we synthesize it from the
        // run row so the client never hangs (see `synth_terminal_event`).
        let mut terminal_emitted = false;
        // Consecutive event-free polls, for the idle back-off. Reset by any
        // row, so an active run always polls at the fast interval.
        let mut idle_polls: u32 = 0;
        // Timed (not notified) wakes so far, which is what paces the run-row
        // check. See `ROW_CHECK_EVERY_N_TIMED_WAKES`.
        let mut timed_wakes: u32 = 0;
        // May this iteration spend a query on the run row?
        //
        // Set at the wake site rather than derived from `timed_wakes` here: a
        // notified wake leaves the counter alone, so `timed_wakes % N == 0`
        // would be permanently TRUE for a locally-driven stream and ask the
        // row on every single event batch — the exact opposite of the intent,
        // and a new query per batch on the handler every domain shares.
        // Starts true so a stream opened against an already-finished run closes
        // on its first pass.
        let mut check_run_row = true;

        loop {
            let rows = match db::get_events_after(&db, &run_id, last_sent_seq).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(run_id, "SSE db error: {e}");
                    break;
                }
            };

            if rows.is_empty() {
                idle_polls = idle_polls.saturating_add(1);
            } else {
                idle_polls = 0;
            }

            let mut terminal = false;
            for row in rows {
                last_sent_seq = row.seq;

                // Pass recovery_resumed events directly (not domain events).
                if row.event_type == "recovery_resumed" {
                    let event = SseEvent::default()
                        .id(row.seq.to_string())
                        .event("recovery_resumed")
                        .data(row.payload.to_string());
                    yield Ok::<_, std::convert::Infallible>(event);
                    continue;
                }

                for (ui_event_type, mut ui_payload) in processor.process(&row.event_type, &row.payload) {
                    // Inject attempt number into every SSE payload.
                    if let serde_json::Value::Object(ref mut map) = ui_payload {
                        map.insert("attempt".into(), row.attempt.into());
                    }
                    let event = SseEvent::default()
                        .id(row.seq.to_string())
                        .event(&ui_event_type)
                        .data(ui_payload.to_string());
                    yield Ok::<_, std::convert::Infallible>(event);

                    if sse::is_terminal(&ui_event_type, &source_type) {
                        terminal = true;
                        terminal_emitted = true;
                    }
                }
            }
            if terminal {
                // Drop this process's notifier entry for a finished run. For a
                // locally-driven run the driver's `deregister` already did it;
                // for a remotely-driven one nothing else ever will, and the
                // entry would otherwise accumulate one per interactive run for
                // the life of the pod.
                //
                // Confirmed against the RUN ROW first, and that is not
                // belt-and-braces. `is_terminal` is a per-source-type
                // classification of a UI event, not proof the run is over —
                // for airway it fires on `load_completed`, and this very
                // predicate has already had a bug of exactly that shape
                // (`subrun_completed` ending an analytics stream mid-run, see
                // `sse::is_terminal`). Removing the notifier on a false
                // positive would take EVERY OTHER subscriber down with it:
                // their `still_active` goes false, they drain, and
                // `synth_terminal_event` yields nothing for a non-terminal
                // status — so they close with no terminal event and their
                // clients hang. This stream returning early is pre-existing
                // behaviour; making it everyone's problem would not be.
                //
                // One primary-key lookup, once, as a stream ends.
                //
                // `deregister` clears `notifiers` AND `answer_txs` AND
                // `cancel_txs` — the three `state.register` created. The drive
                // this handler used to spawn was what called it; removing the
                // drive removed the cleanup, and the remote driver's own
                // `deregister` runs against the WORKER's `RuntimeState`.
                if run_row_is_terminal(&db, &run_id).await {
                    state.deregister(&run_id);
                }
                return;
            }

            // Should this stream close?
            //
            // A notifier entry in THIS process used to be the answer, and it
            // no longer is. Since airway submits enqueue `TaskScope::Global`,
            // a run legitimately outlives the pod that registered its
            // notifier: an ide restart mid-pipeline leaves a reconnecting
            // browser on a fresh pod with an empty map and a run that is very
            // much alive on a worker. Closing there emits NO terminal event —
            // `synth_terminal_event` yields nothing for a `running` row — and
            // `fetchEventSource` answers a clean close by reconnecting, so the
            // bounded poll below degrades into an unbounded reconnect loop
            // through auth middleware on an `IdeOnly` route. That is both the
            // hang this handler exists to prevent and, per the 189-connection
            // incident, the more expensive of the two failure shapes.
            //
            // So the RUN ROW is the authority and the notifier is only a hint
            // about whether anything local will wake us. The row is consulted
            // on a schedule rather than every tick — see
            // `ROW_CHECK_EVERY_N_TIMED_WAKES`.
            if check_run_row && run_row_is_terminal(&db, &run_id).await {
                // One final drain. The run row can reach a terminal status a
                // moment before its last events land, so read once more rather
                // than truncate the stream.
                if let Ok(final_rows) = db::get_events_after(&db, &run_id, last_sent_seq).await {
                    for row in final_rows {
                        last_sent_seq = row.seq;
                        if row.event_type == "recovery_resumed" {
                            let event = SseEvent::default()
                                .id(row.seq.to_string())
                                .event("recovery_resumed")
                                .data(row.payload.to_string());
                            yield Ok(event);
                            continue;
                        }
                        for (ui_event_type, mut ui_payload) in processor.process(&row.event_type, &row.payload) {
                            if let serde_json::Value::Object(ref mut map) = ui_payload {
                                map.insert("attempt".into(), row.attempt.into());
                            }
                            let event = SseEvent::default()
                                .id(row.seq.to_string())
                                .event(&ui_event_type)
                                .data(ui_payload.to_string());
                            yield Ok(event);
                            if sse::is_terminal(&ui_event_type, &source_type) {
                                terminal_emitted = true;
                            }
                        }
                    }
                }
                // Nothing terminal was ever streamed (a failure outside the
                // orchestrator loop, e.g. a broken semantics file, or a driver
                // that died). Derive it from the authoritative run-row status
                // so the client doesn't hang.
                //
                // "Doesn't hang" now carries a tail, and it is worth stating.
                // This path used to fire on the next wake after the notifier
                // went away; it now waits for a wake on which
                // `ROW_CHECK_EVERY_N_TIMED_WAKES` allows the row query. That is
                // `5 × poll_after`, so it depends on which tier the stream is
                // in:
                //
                // - timer is the wake source (airway, or no notifier here) and
                //   still inside the back-off → 1 s tier → **~5 s**
                // - anything at the 30 s tier — a quiet non-airway stream from
                //   its first park, or any stream past
                //   `IDLE_TICKS_BEFORE_BACKOFF` → **~150 s**
                //
                // The 150 s case is real and not only a backed-off one: a
                // local run whose driver died without writing a terminal event
                // leaves its notifier registered, so the stream sits at 30 s
                // and waits it out. Only the synthesized path pays this at all
                // — a terminal *event* closes the stream on the very next poll
                // — and it is the deliberate trade for the query budget above.
                if !terminal_emitted
                    && let Some(ev) =
                        synth_terminal_event(&db, &run_id, last_sent_seq + 1).await
                {
                    yield Ok(ev);
                }
                // `deregister`, not `notifiers.remove`: the drive this handler
                // used to spawn was what called it, and removing the drive
                // removed the cleanup with it. The remote driver's own
                // `deregister` runs against the WORKER's `RuntimeState`, not
                // this one, so without this `answer_txs` and `cancel_txs` grow
                // by one entry per interactive submit for the life of the pod —
                // on exactly the pod this change exists to relieve. Safe here
                // because the run row is confirmed terminal, so nothing is
                // waiting on any of the three maps.
                //
                // NOT a complete reaper, and shouldn't be remembered as one:
                // this only runs when a client streams through to termination.
                // A tab closed mid-run, or a submit whose stream is never
                // opened, still leaves the three entries behind for the life of
                // the pod. That is the same order of growth as `statuses`,
                // which `deregister` deliberately never clears so late
                // subscribers can still read a finished run's outcome — so it
                // is a pre-existing class rather than a new one, and closing it
                // properly means a sweeper, not a bigger `deregister`.
                state.deregister(&run_id);
                return;
            }

            // Park. The notifier is the fast path when the driver is local; the
            // timer is what makes a remotely-driven run work at all, and the
            // only wake source when this process holds no notifier for the run.
            // Fast when the timer is actually the wake source, and only until
            // the stream goes quiet.
            //
            // Two ways it can be the wake source, and `source_type` alone
            // covers just one. A run whose driver may be in another process is
            // the obvious case. The other is a stream holding **no notifier**:
            // `notifier` is snapshotted once, before this loop, and never
            // re-read, so a stream that opens while this process has no entry
            // keeps `None` for its whole life — even if recovery resumes the
            // run here a moment later and registers one. Gating on
            // `driver_may_be_remote` alone put that stream at 30 s forever: an
            // ide restart mid-analytics-run would leave a reconnecting browser
            // receiving events in 30 s batches, and waiting the full
            // `ROW_CHECK_EVERY_N_TIMED_WAKES` × 30 s for a synthesized terminal
            // event, with no back-off needed to get there.
            //
            // A *present* notifier still means zero acquires on the common
            // path, which is the whole point of the gate, and `idle_polls`
            // bounds the no-notifier case exactly as it bounds airway.
            let timer_is_the_wake_source = driver_may_be_remote || notifier.is_none();
            let poll_after = if timer_is_the_wake_source && idle_polls < IDLE_TICKS_BEFORE_BACKOFF {
                REMOTE_DRIVER_POLL_INTERVAL
            } else {
                IDLE_POLL_INTERVAL
            };
            let wake = match &notifier {
                Some(n) => tokio::select! {
                    _ = n.notified() => Wake::Notified,
                    _ = tokio::time::sleep(poll_after) => Wake::Timeout,
                    _ = state.shutdown_token.cancelled() => Wake::Shutdown,
                },
                // No notifier in this process — either the run was never
                // registered here (a reconnect after the registering pod
                // restarted) or it failed before the driver spawned. Nothing
                // can ring us, so the timer is the whole wake source.
                None => tokio::select! {
                    _ = tokio::time::sleep(poll_after) => Wake::Timeout,
                    _ = state.shutdown_token.cancelled() => Wake::Shutdown,
                },
            };
            match wake {
                Wake::Shutdown => break,
                // A notifier ring proves a driver in THIS process is alive and
                // will `deregister` when it finishes, so the run row has
                // nothing to add. Costs zero acquires, which is what keeps
                // analytics and builder — still direct-driven, and most open
                // streams — exactly as cheap as they were before this change.
                Wake::Notified => check_run_row = false,
                Wake::Timeout => {
                    timed_wakes = timed_wakes.saturating_add(1);
                    check_run_row = timed_wakes.is_multiple_of(ROW_CHECK_EVERY_N_TIMED_WAKES);
                }
            }
        }

        // Reached only via the shutdown-token / DB-error `break` paths
        // above. Make a best-effort attempt to close with a terminal event
        // rather than dropping the client mid-stream.
        if !terminal_emitted
            && let Some(ev) = synth_terminal_event(&db, &run_id, last_sent_seq + 1).await
        {
            yield Ok(ev);
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ── POST /runs/:id/answer ─────────────────────────────────────────────────────

/// `POST /runs/{id}/answer`: an answer to a customer's chat. Never to a Builder
/// run (see [`RunRoute`]).
pub async fn answer_run(
    path: Path<RunIdPath>,
    state: Extension<Arc<AgenticState>>,
    platform: Extension<Arc<dyn PlatformContext>>,
    bridges: Extension<BuilderBridges>,
    body: Json<AnswerRequest>,
) -> Response {
    answer_run_on(RunRoute::Chat, path, state, platform, bridges, body).await
}

/// `POST /builder/runs/{id}/answer`: an answer to a Builder or onboarding run.
pub async fn answer_builder_run(
    path: Path<RunIdPath>,
    state: Extension<Arc<AgenticState>>,
    platform: Extension<Arc<dyn PlatformContext>>,
    bridges: Extension<BuilderBridges>,
    body: Json<AnswerRequest>,
) -> Response {
    answer_run_on(RunRoute::Builder, path, state, platform, bridges, body).await
}

async fn answer_run_on(
    route: RunRoute,
    Path(RunIdPath { id: run_id }): Path<RunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    Extension(bridges): Extension<BuilderBridges>,
    Json(body): Json<AnswerRequest>,
) -> Response {
    let scoped = match run_in_workspace(&state.db, &run_id, platform.workspace_id()).await {
        Ok(run) => run,
        Err(resp) => return resp,
    };
    // A run's kind never changes, so the row the scope check read decides both
    // Builder refusals below. Read from that row, not again: a second read that
    // failed would skip the refusal.
    let is_builder_run = resumes_only_with_the_files(scoped.source_type.as_deref());
    // The chat route never answers a Builder run, on any process: the Factory
    // gate does not cover it, and the Builder edits workspace files. Checked
    // before the hot path, which would otherwise hand the answer straight to a
    // Builder coordinator alive in this process.
    if route == RunRoute::Chat && is_builder_run {
        return builder_needs_the_factory("Answering a Builder run", BUILDER_ANSWER_ROUTE);
    }
    // Hot path: the coordinator is still alive in memory — deliver the
    // answer through the answer channel. The coordinator receives it via
    // answer_rxs and handles resume via TaskSpec::Resume (spawning a fresh
    // pipeline). We skip the status check because the coordinator processes
    // messages sequentially — it handles TaskOutcome::Suspended before
    // checking answer_rxs, so the answer is safe to buffer.
    if let Some(tx) = state.answer_txs.get(&run_id) {
        let tx = tx.clone();
        if tx.send(body.answer.clone()).await.is_ok() {
            return Json(serde_json::json!({ "ok": true, "resumed": true })).into_response();
        }
        // Coordinator dropped the answer channel — fall through to cold
        // resume so the pipeline can be rebuilt from persisted suspension data.
        tracing::warn!(
            run_id = %run_id,
            "hot-path answer channel closed, falling through to cold resume"
        );
    }

    let db = state.db.clone();

    // A Builder run is resumed only where its files are. On a process without
    // them the cold resume below would rebuild the Builder over a working copy
    // that is not here; the Factory holds the run's answer channel, and its
    // answer route reaches it.
    if !(state.holds_working_copy)() && is_builder_run {
        return builder_needs_the_factory("Answering a Builder run", BUILDER_ANSWER_ROUTE);
    }

    // A chat that records its requester is continued through the queue: its
    // driver, wherever it was, is gone or parked it, and a driver on any pod
    // resumes it from its checkpoint as the requester
    // (`agentic_pipeline::queued_answer`). Anything else — a builder run, an
    // onboarding run, a preview's, a chat from before the record — is rebuilt
    // and driven here, as before.
    //
    // Only for a run already on the queue, or while queued starts are on
    // (`AgenticState::queued_starts`). With the switch off, a chat driven in
    // a request's process that lost its channel (its pod restarted) is resumed
    // here, as before: queueing its answer would hand a Global resume to
    // whatever driver claims it, and in a rolling deploy that can be one from
    // before the requester record, which would run it on its own subject-less
    // context.
    let queue_answers = (state.queued_starts)()
        || agentic_pipeline::queued_run::started_on_the_queue(&db, &run_id)
            .await
            .unwrap_or(false);
    if queue_answers && let Some(resp) = answer_through_the_queue(&db, &scoped, &body.answer).await
    {
        return resp;
    }

    // Cold resume: coordinator is dead (e.g. after server restart).
    // Rebuild the pipeline and drive a new coordinator.

    // Retry a few times: there is a small window where the frontend sees the
    // awaiting_input SSE event but the coordinator hasn't yet persisted the
    // suspension status to DB (e.g. if the server just restarted and the DB
    // write is in flight).
    let mut run = None;
    for attempt in 0..3 {
        match db::get_run(&db, &run_id).await {
            Ok(Some(r)) if r.task_status.as_deref() == Some("awaiting_input") => {
                run = Some(r);
                break;
            }
            Ok(Some(r)) if attempt < 2 => {
                tracing::debug!(
                    run_id = %run_id,
                    task_status = ?r.task_status,
                    attempt,
                    "answer_run: task_status not yet awaiting_input, retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Ok(Some(r)) => {
                return (
                    StatusCode::CONFLICT,
                    format!(
                        "run is not suspended (task_status: {})",
                        r.task_status.as_deref().unwrap_or("none")
                    ),
                )
                    .into_response();
            }
            Ok(None) => return (StatusCode::NOT_FOUND, "run not found").into_response(),
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}"))
                    .into_response();
            }
        }
    }
    let run = run.expect("loop only breaks when task_status == awaiting_input");

    // A live driver in another process (a chat driven in the memory of the pod
    // that took its start, with the queue off) still holds this run and will take
    // its answer through its own channel. A cold resume here would be a second
    // drive of the same run. Refuse, retryably: the client retries on
    // `Retry-After`, and the answer reaches the driver when a retry lands on its
    // pod, or resumes here once that driver's lease lapses.
    if driven_elsewhere(
        run.driver_id.as_deref(),
        run.driver_heartbeat_at,
        agentic_runtime::lifecycle::driver_lease::held_in_this_process(&run_id),
        chrono::Utc::now(),
    ) {
        tracing::info!(run_id = %run_id, "answer_run: another process drives this run; not resuming it here");
        return driven_elsewhere_response();
    }

    let source_type = run.source_type.as_deref().unwrap_or("analytics");

    let resume_data = match db::get_suspension(&db, &run_id).await {
        Ok(Some(data)) => data,
        Ok(None) => {
            return (StatusCode::GONE, "no suspension data found for this run").into_response();
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response();
        }
    };

    // Extract agent_id and model from run metadata.
    let agent_id = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("agent_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    let model = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Restore the cached-prefix card set so the resumed solver builds
    // a system prefix byte-identical to the create-time one — anything
    // else would defeat the prompt cache on the resume turn.
    let knowledge_cards: Vec<agentic_pipeline::KnowledgeCard> = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("knowledge_cards"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_str()
                        .and_then(agentic_pipeline::KnowledgeCard::from_slug)
                })
                .collect()
        })
        .unwrap_or_default();

    // Same for skip_interpreting and tool_allowlist — both must match
    // the create-time values for cache stability on the resume turn,
    // and skip_interpreting in particular changes terminal behavior
    // (empty answer text vs. synthesized summary).
    let skip_interpreting = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("skip_interpreting"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let tool_allowlist: Option<Vec<String>> = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("tool_allowlist"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

    // Rebuild the pipeline and drive it. `existing_run_id` skips the
    // DB insert (this is a resume), so workspace_id here is informational
    // only — but set it for consistency in case a future drive path adds
    // a fresh insert.
    let mut builder = PipelineBuilder::new(platform.clone())
        .workspace_id(platform.workspace_id())
        .with_builder_bridges(bridges.clone())
        .question(&run.question)
        .schema_cache(Arc::clone(&state.schema_cache))
        .knowledge_cards(knowledge_cards)
        .skip_interpreting(skip_interpreting);

    if let Some(allowlist) = tool_allowlist {
        builder = builder.tool_allowlist(allowlist);
    }

    if let Some(tid) = run.thread_id {
        builder = builder.thread(tid);
    }
    if let Some(runner) = state.builder_test_runner.clone() {
        builder = builder.test_runner(runner);
    }

    // Cold-resume of an onboarding builder run: rebuild the same LlmClient
    // create_run constructed, since the chosen model may not be in
    // config.yml yet (the run was suspended mid-Config-phase, before the
    // file was written). Without this, the resumed solver falls through to
    // `resolve_model` against an incomplete config and ends up calling the
    // LLM with an empty API key.
    let onboarding_vendor = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("onboarding_vendor"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let onboarding_model_ref = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("onboarding_model_ref"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let onboarding_key_var = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("onboarding_key_var"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let (Some(vendor), Some(model_ref), Some(key_var)) =
        (onboarding_vendor, onboarding_model_ref, onboarding_key_var)
    {
        let client = build_onboarding_llm_client(&*platform, &vendor, &model_ref, &key_var).await;
        builder = builder.with_builder_llm_client(client);
    }

    // Persist an input_resolved event so the SSE stream (and page reloads)
    // can see that the suspension was resolved. The trace_id matches the
    // corresponding awaiting_input event for frontend correlation.
    let answer_for_event = body.answer.clone();
    {
        let max_seq = db::get_max_seq(&db, &run_id).await.unwrap_or(-1);
        let payload =
            serde_json::json!({ "answer": answer_for_event, "trace_id": &resume_data.trace_id });
        if let Err(e) = db::insert_event(
            &db,
            &run_id,
            max_seq + 1,
            "input_resolved",
            &payload,
            run.attempt,
        )
        .await
        {
            tracing::error!(run_id = %run_id, error = %e, "failed to persist input_resolved for cold resume");
        }
    }

    let started = match builder
        .resume(
            &db,
            &run_id,
            source_type,
            &agent_id,
            model,
            resume_data,
            body.answer,
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            // `builder.resume()` already transitioned the run row to
            // `failed` via the pipeline chokepoint, so any SSE stream still
            // open on this suspended run will synthesize a terminal `error`
            // event and stop hanging. Return the real cause to the caller.
            return pipeline_error_response(StatusCode::BAD_REQUEST, &e, Some(&run_id));
        }
    };

    // Register in-memory state and drive the resumed pipeline via coordinator.
    let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state.register(&run_id, answer_tx, cancel_tx);

    let runtime_state = state.runtime.clone();
    let schema_cache = Some(state.schema_cache.clone());
    let builder_test_runner = state.builder_test_runner.clone();
    let router = state.router.clone();
    spawn_with_hub(async move {
        agentic_pipeline::drive_with_coordinator(
            started,
            db,
            runtime_state,
            answer_rx,
            cancel_rx,
            platform,
            Some(bridges),
            schema_cache,
            builder_test_runner,
            router,
        )
        .await;
    });

    Json(serde_json::json!({ "ok": true, "resumed": true })).into_response()
}

/// How often, and how far apart, an answer re-reads a run that is not yet
/// parked. The question reaches the client as an event before its driver has
/// written the suspension and let the run go, so an answer can arrive in that
/// gap: a few tries over about a second covers it.
const ANSWER_SETTLE_TRIES: u32 = 6;
const ANSWER_SETTLE_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// The answer to an answer whose run a live driver in another process holds:
/// `503` with `Retry-After`, which the web client retries on. That driver takes
/// the answer through its own channel once a retry reaches its pod, and a
/// retry here resumes the run once its lease lapses.
fn driven_elsewhere_response() -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "This question is still being worked on by another server. Try again in a moment.",
            "retryable": true,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("2"),
    );
    response
}

/// Continue `run` with `answer` through the queue, when it is a run that
/// goes that way: an analytics root that records its requester and is not a
/// preview's. `None` for any other run — the caller resumes it in process.
/// What it reads from `run` (kind, parent, metadata) is fixed at the start.
async fn answer_through_the_queue(
    db: &sea_orm::DatabaseConnection,
    run: &agentic_runtime::entity::run::Model,
    answer: &str,
) -> Option<Response> {
    use agentic_pipeline::queued_answer::{QueuedAnswer, queue_answer};

    let run_id = run.id.as_str();
    let queued_kind = run.source_type.as_deref() == Some("analytics")
        && run.parent_run_id.is_none()
        && !agentic_pipeline::platform::preview_stamp::is_stamped(run.metadata.as_ref())
        && matches!(Requester::recorded(run.metadata.as_ref()), Ok(Some(_)));
    if !queued_kind {
        return None;
    }
    let mut tries = 0;
    loop {
        tries += 1;
        let result = queue_answer(db, run_id, answer).await;
        // Not parked yet: its driver is still writing the suspension (the run
        // reads `running`, or has no checkpoint), or still holds the lease it
        // is about to release.
        let settling = match &result {
            Ok(QueuedAnswer::DrivenElsewhere | QueuedAnswer::NoCheckpoint) => true,
            Ok(QueuedAnswer::NotWaiting(Some(status))) => status == "running",
            _ => false,
        };
        if !settling || tries >= ANSWER_SETTLE_TRIES {
            return Some(answer_response(run_id, result));
        }
        tokio::time::sleep(ANSWER_SETTLE_DELAY).await;
    }
}

/// The response to an answer [`answer_through_the_queue`] settled.
fn answer_response(
    run_id: &str,
    result: Result<agentic_pipeline::queued_answer::QueuedAnswer, sea_orm::DbErr>,
) -> Response {
    use agentic_pipeline::queued_answer::QueuedAnswer;
    match result {
        Ok(QueuedAnswer::Queued) => {
            Json(serde_json::json!({ "ok": true, "resumed": true })).into_response()
        }
        Ok(QueuedAnswer::NotWaiting(status)) => (
            StatusCode::CONFLICT,
            format!(
                "run is not suspended (task_status: {})",
                status.as_deref().unwrap_or("none")
            ),
        )
            .into_response(),
        Ok(QueuedAnswer::DrivenElsewhere) => driven_elsewhere_response(),
        Ok(QueuedAnswer::NoCheckpoint) => {
            (StatusCode::GONE, "no suspension data found for this run").into_response()
        }
        Err(e) => {
            tracing::error!(%run_id, error = %e, "answer: could not queue the continuation");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response()
        }
    }
}

// ── POST /runs/:id/revert-file-changes ───────────────────────────────────────

#[derive(Deserialize)]
pub struct RevertFileChangesRequest {
    /// Files to revert. Empty / omitted reverts every file the builder
    /// changed in this run.
    #[serde(default)]
    pub file_paths: Vec<String>,
}

/// Revert builder-applied file change(s) for a run. Used by the analytics
/// suspend → builder-agent panel to undo edits the delegated builder
/// auto-applied (e.g. a semantic-file fix that wasn't wanted).
pub async fn revert_file_changes(
    Path(RunIdPath { id: run_id }): Path<RunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    Json(body): Json<RevertFileChangesRequest>,
) -> Response {
    if let Err(resp) = run_in_workspace(&state.db, &run_id, platform.workspace_id()).await {
        return resp;
    }
    match agentic_pipeline::revert_builder_file_changes(
        &state.db,
        &platform,
        &run_id,
        &body.file_paths,
    )
    .await
    {
        Ok(reverted) => {
            Json(serde_json::json!({ "ok": true, "reverted": reverted })).into_response()
        }
        Err(e) => {
            tracing::warn!(run_id = %run_id, error = %e, "revert_file_changes failed");
            pipeline_error_response(StatusCode::BAD_REQUEST, &e, Some(&run_id))
        }
    }
}

// ── POST /runs/:id/cancel ─────────────────────────────────────────────────────

pub async fn cancel_run(
    Path(RunIdPath { id: run_id }): Path<RunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
) -> Response {
    if let Err(resp) = run_in_workspace(&state.db, &run_id, platform.workspace_id()).await {
        return resp;
    }
    // Durable cross-process cancel signal — set before the in-memory
    // fast path: a recovered run has a registered cancel_tx (so
    // `state.cancel` returns true) but is driven out-of-process where
    // nothing observes the watch; its forwarder polls this flag.
    agentic_runtime::crud::request_cancel(&state.db, &run_id)
        .await
        .ok();
    if state.cancel(&run_id) {
        return Json(serde_json::json!({ "ok": true })).into_response();
    }
    // No driver here. Close the row only when nothing can be driving it: a
    // run with a queue entry, or whose driver lease is live in another
    // process, is that driver's to close — it turns the flag into a proper
    // `cancelled` within about 5 s. A run that already ended stays as it
    // ended. What is left is a run nobody will ever close (its driver died,
    // or it is parked on a person), and failing it is what keeps it from
    // reading `running` for ever.
    match agentic_pipeline::queued_answer::close_if_undriven(
        &state.db,
        &run_id,
        "cancelled by user",
    )
    .await
    {
        Ok(true) => {
            state.statuses.insert(
                run_id.clone(),
                RunStatus::Failed("cancelled by user".into()),
            );
        }
        Ok(false) => {}
        Err(e) => tracing::warn!(%run_id, error = %e, "cancel: closing an undriven run failed"),
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

// ── PATCH /runs/:id/thinking_mode ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UpdateThinkingModeRequest {
    pub thinking_mode: Option<ThinkingMode>,
}

pub async fn update_thinking_mode(
    Path(RunIdPath { id: run_id }): Path<RunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    Json(body): Json<UpdateThinkingModeRequest>,
) -> Response {
    if let Err(resp) = run_in_workspace(&state.db, &run_id, platform.workspace_id()).await {
        return resp;
    }
    let db = state.db.clone();
    let thinking_mode = body.thinking_mode.unwrap_or(ThinkingMode::Auto);
    match db::update_run_thinking_mode(&db, &run_id, thinking_mode.to_db()).await {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response(),
    }
}

// ── GET /threads/:thread_id/runs ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{RunRoute, driven_elsewhere, refuses_builder_start};

    /// The chat route is on the customer list, so the Factory gate lets any member
    /// through it: it must refuse a Builder start on every process, including the
    /// Factory pod and a single-process server that hold the files.
    #[test]
    fn the_chat_route_never_starts_a_builder_run() {
        for holds in [true, false] {
            assert!(
                refuses_builder_start(RunRoute::Chat, true, holds),
                "holds={holds}"
            );
            assert!(
                !refuses_builder_start(RunRoute::Chat, false, holds),
                "holds={holds}"
            );
        }
        // The Builder route (on the Factory list) starts one where the files are.
        assert!(!refuses_builder_start(RunRoute::Builder, true, true));
        assert!(refuses_builder_start(RunRoute::Builder, true, false));
    }

    /// An answer never starts a second drive of a run another process drives.
    #[test]
    fn an_answer_does_not_resume_a_run_another_process_is_driving() {
        let now = chrono::Utc::now();
        let fresh = Some((now - chrono::Duration::seconds(10)).fixed_offset());
        let lapsed = Some((now - chrono::Duration::seconds(600)).fixed_offset());
        assert!(driven_elsewhere(Some("direct-a"), fresh, false, now));
        // Its own drive (the channel closed) may be resumed here.
        assert!(!driven_elsewhere(Some("direct-a"), fresh, true, now));
        // No driver, or one whose lease lapsed: nobody else will take it.
        assert!(!driven_elsewhere(None, None, false, now));
        assert!(!driven_elsewhere(Some("direct-a"), lapsed, false, now));
    }

    use super::{branch_in_query, classify_pipeline_error_message, terminal_event_for_status};

    fn start(body: serde_json::Value) -> super::CreateRunRequest {
        serde_json::from_value(body).expect("a run request")
    }

    #[test]
    fn a_builder_or_onboarding_start_is_a_builder_run_and_a_chat_is_not() {
        use super::starts_a_builder_run;
        let chat = serde_json::json!({ "agent_id": "a.agentic.yml", "question": "q" });
        assert!(!starts_a_builder_run(&start(chat)));
        let builder = serde_json::json!({
            "agent_id": "__builder__", "question": "q", "domain": "builder"
        });
        assert!(starts_a_builder_run(&start(builder)));
        // Onboarding sends `domain: "builder"` too; its context alone decides.
        let onboarding = serde_json::json!({
            "agent_id": "__builder__", "question": "",
            "onboarding_context": { "tables": ["public.orders"], "warehouse_type": "postgres" }
        });
        assert!(starts_a_builder_run(&start(onboarding)));
    }

    #[test]
    fn only_a_builder_run_needs_the_files_to_resume() {
        use super::resumes_only_with_the_files;
        assert!(resumes_only_with_the_files(Some("builder")));
        for other in [Some("analytics"), Some("workflow"), Some("airway"), None] {
            assert!(!resumes_only_with_the_files(other), "{other:?}");
        }
    }

    /// The refusal reads like the role middleware's own misroute answer, so a
    /// client handles both the same way, and names where to go instead.
    #[tokio::test]
    async fn the_refusal_names_the_route_the_factory_serves() {
        use super::{BUILDER_START_ROUTE, builder_needs_the_factory};
        let response = builder_needs_the_factory("A Builder run", BUILDER_START_ROUTE);
        assert_eq!(
            response.status(),
            axum::http::StatusCode::MISDIRECTED_REQUEST
        );
        assert_eq!(
            response.headers().get("x-oxy-required-role").unwrap(),
            "ide"
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["route"], BUILDER_START_ROUTE);
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|e| e.contains(BUILDER_START_ROUTE))
        );
    }

    #[test]
    fn a_start_names_a_branch_only_through_a_non_empty_branch_param() {
        assert_eq!(branch_in_query(None), None);
        assert_eq!(branch_in_query(Some("")), None);
        assert_eq!(branch_in_query(Some("branch=")), None);
        assert_eq!(branch_in_query(Some("branches=x")), None);
        assert_eq!(
            branch_in_query(Some("branch=feat-x")),
            Some("feat-x".into())
        );
        assert_eq!(branch_in_query(Some("a=1&branch=dev")), Some("dev".into()));
    }

    #[test]
    fn failed_status_maps_to_error_event_with_message() {
        let (ty, payload) = terminal_event_for_status(
            Some("failed"),
            Some("semantic catalog error: bad yaml"),
            None,
            "r1",
        )
        .expect("failed is terminal");
        assert_eq!(ty, "error");
        assert_eq!(payload["message"], "semantic catalog error: bad yaml");
        assert_eq!(payload["trace_id"], "r1");
    }

    #[test]
    fn timed_out_maps_to_error_event() {
        let (ty, _) = terminal_event_for_status(Some("timed_out"), None, None, "r1")
            .expect("timed_out is terminal");
        assert_eq!(ty, "error");
    }

    #[test]
    fn failed_without_message_uses_fallback_text() {
        let (_, payload) = terminal_event_for_status(Some("failed"), None, None, "r1").unwrap();
        assert_eq!(
            payload["message"],
            "The run failed before it could report an error."
        );
    }

    #[test]
    fn cancelled_and_done_map_to_their_events() {
        assert_eq!(
            terminal_event_for_status(Some("cancelled"), None, None, "r1")
                .unwrap()
                .0,
            "cancelled"
        );
        let (ty, payload) =
            terminal_event_for_status(Some("done"), None, Some("42"), "r1").unwrap();
        assert_eq!(ty, "done");
        assert_eq!(payload["answer"], "42");
    }

    #[test]
    fn non_terminal_statuses_do_not_synthesize() {
        for s in ["running", "awaiting_input", "delegating"] {
            assert!(terminal_event_for_status(Some(s), None, None, "r1").is_none());
        }
        assert!(terminal_event_for_status(None, None, None, "r1").is_none());
    }

    #[test]
    fn semantic_errors_are_reworded_for_the_user() {
        let msg = classify_pipeline_error_message("build error: semantic catalog error: x");
        assert!(msg.contains("Your project configuration is invalid"));
        assert!(msg.contains("semantic catalog error: x"));
    }

    #[test]
    fn non_config_errors_pass_through_verbatim() {
        let msg = classify_pipeline_error_message("db error: connection refused");
        assert_eq!(msg, "db error: connection refused");
    }

    /// The four statuses that close a stream. `stream_events` no longer treats
    /// a missing notifier as "the run is over" — a `Global` run outlives the
    /// pod that registered it — so this predicate is the only thing that
    /// closes a remotely-driven stream.
    #[test]
    fn only_the_four_terminal_statuses_close_a_stream() {
        for s in ["done", "failed", "cancelled", "timed_out"] {
            assert!(super::status_is_terminal(Some(s)), "{s} must be terminal");
        }
    }

    /// The half that actually bites. Every one of these means the run is still
    /// going, and closing on any of them emits a synthesized terminal event
    /// over live work — `awaiting_input` most dangerously, since a
    /// `SuspendedHuman` run looks idle for hours by design.
    #[test]
    fn an_in_flight_run_never_closes_the_stream() {
        for s in [
            "running",
            "awaiting_input",
            "delegating",
            "waiting_on_child",
            "waiting_on_children",
            "needs_resume",
            "shutdown",
            "pending",
        ] {
            assert!(
                !super::status_is_terminal(Some(s)),
                "{s} is not terminal; closing here would strand a live run"
            );
        }
    }

    /// A run row with no status, or none the query could read, must keep the
    /// stream open rather than close it — erring toward a slightly long-lived
    /// stream instead of a truncated one.
    #[test]
    fn an_absent_status_is_not_terminal() {
        assert!(!super::status_is_terminal(None));
    }
}
