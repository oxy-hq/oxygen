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

use agentic_pipeline::AutoAcceptInputProvider;
use agentic_pipeline::PipelineBuilder;
use agentic_pipeline::platform::{PlatformContext, Requester};
use oxy_auth::extractor::AuthenticatedUserExtractor;

use crate::{
    db, sse,
    state::{AgenticState, RunStatus},
};

use super::run_scope::{ensure_thread_access, run_in_workspace};
use super::{AnswerRequest, CreateRunRequest, CreateRunResponse, RunIdPath, ThinkingMode};

/// The `error` of the answer to a Builder start or a Builder-run answer: the
/// Builder, which edited workspace files, was removed with the IDE.
pub const BUILDER_REMOVED: &str = "builder_removed";

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

/// Whether a run of this `source_type` is a Builder run (the Builder copilot,
/// onboarding), which nothing resumes any more.
fn is_a_builder_run(source_type: Option<&str>) -> bool {
    source_type == Some("builder")
}

/// `410 Gone` for a Builder start or an answer to a Builder run: the Builder
/// edited workspace files and was removed with the IDE. A workspace is changed
/// in its repository.
fn builder_removed(what: &str) -> Response {
    (
        StatusCode::GONE,
        Json(serde_json::json!({
            "error": BUILDER_REMOVED,
            "message": format!(
                "{what} is no longer possible: the Builder was removed with the IDE. \
                 Change the workspace in its repository."
            ),
        })),
    )
        .into_response()
}

/// The refusal for a start that asks for the Builder (the Builder domain, or
/// onboarding), `None` for any other start. `create_run` calls it first, before
/// anything is written.
fn refuse_builder_start(body: &CreateRunRequest) -> Option<Response> {
    if !starts_a_builder_run(body) {
        return None;
    }
    tracing::info!(
        domain = ?body.domain,
        onboarding = body.onboarding_context.is_some(),
        "create_run: refused a Builder start"
    );
    Some(builder_removed("Starting a Builder or onboarding run"))
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

/// `POST /runs`: a chat. Never a Builder run (see [`builder_removed`]).
pub async fn create_run(
    state: Extension<Arc<AgenticState>>,
    platform: Extension<Arc<dyn PlatformContext>>,
    query: axum::extract::RawQuery,
    user: AuthenticatedUserExtractor,
    body: Json<CreateRunRequest>,
) -> Response {
    create_run_on(state, platform, query, user, body).await
}

async fn create_run_on(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
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

    // The Builder (the Builder domain, and onboarding, which ran it) edited the
    // working copy; it was removed with the IDE. Refused before anything is
    // written.
    if let Some(refusal) = refuse_builder_start(&body) {
        return refusal;
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

    let question = body.question.clone();

    // Auto-provision a thread when an interactive run is started without
    // one. Persisting a real `threads` row (a) satisfies the
    // `agentic_runs.thread_id` FK and (b) hands the client a stable id it
    // can reuse for follow-up questions so the agent keeps conversation
    // context.
    if thread_id_uuid.is_none() {
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
    // `agentic_pipeline::queued_run::ChatAdmission`. Only where the host queues
    // starts (`AgenticState::queued_starts`); elsewhere every chat is driven here.
    let admission = agentic_pipeline::queued_run::ChatAdmission {
        auto_accept: body.auto_accept,
        working_copy_branch: branch_in_query(query.as_deref()),
    };
    let queued_for = if state.queued_starts {
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
            recorded_runtime: None,
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

    let mut builder = PipelineBuilder::new(platform.clone())
        .workspace_id(platform.workspace_id())
        .question(&question)
        .thinking_mode(body.thinking_mode);
    // Who asked, on the run's own insert: whatever drives this run later
    // without this request (recovery after a restart, a queued start's driver)
    // builds the requester's context from it instead of running the root on
    // its own subject-less one.
    if let Some(requester) = platform.requester() {
        builder = builder.run_metadata(Requester::KEY, requester.to_metadata());
    }

    if let Some(tid) = thread_id_uuid {
        builder = builder.thread(tid);
    }

    // The caller asked for no HITL: every question is accepted.
    if body.auto_accept {
        builder = builder.human_input(Arc::new(AutoAcceptInputProvider));
    }

    let started = match builder.analytics(&body.agent_id).start(&db).await {
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
    let router = state.router.clone();
    spawn_with_hub(async move {
        agentic_pipeline::drive_with_coordinator(
            started,
            db,
            runtime_state,
            answer_rx,
            cancel_rx,
            platform,
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

// The terminal-event mapping and the poll pacing live in
// `agentic_runtime::lifecycle::run_stream`, so the custom-app stream
// (`oxy-api-custom-apps`) shares one copy without naming this crate. The
// re-export keeps `agentic_http::routes::run::{..}` working for one release.
pub use agentic_runtime::lifecycle::run_stream::{
    ROW_CHECK_EVERY_N_TIMED_WAKES, poll_after, terminal_event_for_status,
};

/// Why [`stream_events`] stopped parking.
enum Wake {
    /// The in-process driver flushed events — re-read immediately.
    Notified,
    /// A `NOTIFY oxy_run_events` for this run reached us from the write path:
    /// an append on any pod, or a terminal run-row transition that wrote no
    /// event. Re-read immediately; if nothing new is there, ask the row.
    Remote,
    /// Nothing rang us within [`poll_after`]: the run is idle, or the NOTIFY
    /// path is not delivering. Re-read either way.
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
    // Airway is the only kind enqueued `TaskScope::Global` from EVERY
    // interactive submit. A queued chat or member-started automation is too,
    // but its stream finds no notifier here and is caught by
    // `driver_may_be_elsewhere` below. A direct-driven run — builder, preview,
    // branch, a chat on a host that does not queue starts — has a notifier, and it IS
    // authoritative, so a fast
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

    // May the driver be in another process? Two ways, and `source_type` alone
    // covers just one: a kind that is enqueued `Global`, or a stream holding
    // **no notifier** — nothing here registered the run, so if anyone drives
    // it, it is another pod (a reconnect after the registering pod restarted,
    // a queued chat another pod claimed, a run that failed before its driver
    // spawned). `notifier` is snapshotted once and never re-read, so this is
    // permanent for the stream's life.
    //
    // Such a stream parks on the cross-pod wake too: every append to
    // `agentic_run_events` NOTIFYs its run id, and the router's listener rings
    // `remote_wakes` for it (`agentic_runtime::lifecycle::remote_wake`). The
    // subscription is dropped with the stream — client gone, terminal event,
    // shutdown — so the registry holds exactly the streams that are open.
    // A locally-driven stream does not subscribe: its own driver rings it,
    // and the NOTIFY echo would only add an empty read per flush.
    let driver_may_be_elsewhere = driver_may_be_remote || notifier.is_none();
    let remote_wake =
        driver_may_be_elsewhere.then(|| state.runtime.remote_wakes.subscribe(&run_id));

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
        // The last wake was a cross-pod NOTIFY. If the read it triggers finds
        // no new rows, the write behind it was a run-row-only terminal
        // transition (`transition_run`, the recovery paths), so the row is
        // asked on this pass rather than on the fifth timed wake.
        let mut remote_ring_pending = false;

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
            if std::mem::take(&mut remote_ring_pending) && rows.is_empty() {
                check_run_row = true;
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
                // "Doesn't hang" carries a tail only when nothing rings. A
                // run-row-only terminal transition (`transition_run` and the
                // recovery / dead-letter writers) NOTIFYs the run like an
                // append would; a stream parked on `remote_wakes` wakes, reads
                // nothing new, and asks the row on this very pass — so a
                // remote-driven run whose driver died closes within one
                // read of the row being written. Two streams still wait for a
                // timed wake on which `ROW_CHECK_EVERY_N_TIMED_WAKES` allows
                // the row query, `5 × poll_after`:
                //
                // - a locally-driven stream (it parks on `notifiers`, which
                //   the dead driver no longer rings, and does not subscribe to
                //   the NOTIFY) → 30 s tier → **~150 s**
                // - any stream while the LISTEN connection is down → its
                //   tier → **~50 s** inside the back-off, ~150 s past it
                //
                // Both are the deliberate trade for the query budget above:
                // a terminal *event* closes the stream on the very next wake,
                // and the row-only case is on the NOTIFY path too, so the
                // timer only ever pays for a dead local driver or a dead
                // listener.
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

            // Park. Two wake sources, then the timer: the in-process notifier
            // when the driver is local, the cross-pod `oxy_run_events` wake
            // when it may be elsewhere. The timer is the safety net for a
            // listener outage — fast only for a stream whose driver may be
            // elsewhere, and only until the stream goes quiet; a locally
            // driven stream sits at 30 s because its notifier is authoritative.
            //
            // A stream has at least one of the two sources: `remote_wake` is
            // `Some` exactly when `notifier` is `None` or the kind is airway.
            // An absent source is a future that never resolves.
            //
            // A *present* notifier still means zero acquires on the common
            // path, which is the whole point of the gate, and `idle_polls`
            // bounds the no-notifier case exactly as it bounds airway.
            let park_for = poll_after(driver_may_be_elsewhere, idle_polls);
            let local_ring = async {
                match &notifier {
                    Some(n) => n.notified().await,
                    None => std::future::pending::<()>().await,
                }
            };
            let remote_ring = async {
                match &remote_wake {
                    Some(sub) => sub.notified().await,
                    None => std::future::pending::<()>().await,
                }
            };
            let wake = tokio::select! {
                _ = local_ring => Wake::Notified,
                _ = remote_ring => Wake::Remote,
                _ = tokio::time::sleep(park_for) => Wake::Timeout,
                _ = state.shutdown_token.cancelled() => Wake::Shutdown,
            };
            match wake {
                Wake::Shutdown => break,
                // A local ring proves a driver in THIS process is alive and
                // will `deregister` when it finishes, so the run row has
                // nothing to add. Costs zero acquires, which is what keeps
                // analytics and builder — direct-driven, and most open
                // streams — exactly as cheap as before.
                Wake::Notified => check_run_row = false,
                // A NOTIFY says "something about this run changed". New rows
                // explain it, and a terminal one closes the stream at the top
                // of the loop; no new rows means a run-row-only write, so the
                // row is asked on the next pass (see `remote_ring_pending`).
                Wake::Remote => {
                    check_run_row = false;
                    remote_ring_pending = true;
                }
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

/// `POST /runs/{id}/answer`: an answer to a chat. Never to a Builder run (see
/// [`builder_removed`]).
pub async fn answer_run(
    path: Path<RunIdPath>,
    state: Extension<Arc<AgenticState>>,
    platform: Extension<Arc<dyn PlatformContext>>,
    body: Json<AnswerRequest>,
) -> Response {
    answer_run_on(path, state, platform, body).await
}

async fn answer_run_on(
    Path(RunIdPath { id: run_id }): Path<RunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    Json(body): Json<AnswerRequest>,
) -> Response {
    let scoped = match run_in_workspace(&state.db, &run_id, platform.workspace_id()).await {
        Ok(run) => run,
        Err(resp) => return resp,
    };
    // A Builder run is never resumed: the Builder was removed with the IDE.
    // Checked before the hot path, which would otherwise hand the answer to a
    // coordinator alive in this process.
    if is_a_builder_run(scoped.source_type.as_deref()) {
        return builder_removed("Answering a Builder run");
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

    // A chat that records its requester is continued through the queue: its
    // driver, wherever it was, is gone or parked it, and a driver on any pod
    // resumes it from its checkpoint as the requester
    // (`agentic_pipeline::queued_answer`). Anything else — a preview's, a chat
    // from before the record — is rebuilt and driven here, as before.
    //
    // Only for a run already on the queue, or where the host queues starts
    // (`AgenticState::queued_starts`). On a host that does not, a chat driven in
    // a request's process that lost its channel (its pod restarted) is resumed
    // here: no driver there would claim a queued answer.
    let queue_answers = state.queued_starts
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

    // Extract agent_id from run metadata.
    let agent_id = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("agent_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    // Rebuild the pipeline and drive it. `existing_run_id` skips the
    // DB insert (this is a resume), so workspace_id here is informational
    // only — but set it for consistency in case a future drive path adds
    // a fresh insert.
    let mut builder = PipelineBuilder::new(platform.clone())
        .workspace_id(platform.workspace_id())
        .question(&run.question);

    if let Some(tid) = run.thread_id {
        builder = builder.thread(tid);
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
    let router = state.router.clone();
    spawn_with_hub(async move {
        agentic_pipeline::drive_with_coordinator(
            started,
            db,
            runtime_state,
            answer_rx,
            cancel_rx,
            platform,
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
    use super::driven_elsewhere;

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

    use super::{branch_in_query, classify_pipeline_error_message};

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

    /// Asserts `refuse_builder_start` answers `body` with `410 builder_removed`.
    async fn assert_refused_as_gone(body: serde_json::Value) {
        use super::{BUILDER_REMOVED, refuse_builder_start};
        let response = refuse_builder_start(&start(body.clone()))
            .unwrap_or_else(|| panic!("{body} was not refused"));
        assert_eq!(response.status(), axum::http::StatusCode::GONE, "{body}");
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["error"], BUILDER_REMOVED, "{body}");
    }

    /// An old client's onboarding start must reach the 410, not die as a 400
    /// deserialisation error, whatever shape its `onboarding_context` has.
    #[tokio::test]
    async fn an_onboarding_start_with_any_payload_shape_gets_410() {
        for context in [
            serde_json::json!({ "vendor": "x" }),
            serde_json::json!("legacy"),
        ] {
            assert_refused_as_gone(serde_json::json!({
                "agent_id": "__builder__", "question": "q", "onboarding_context": context
            }))
            .await;
        }
    }

    #[tokio::test]
    async fn a_builder_domain_start_gets_410() {
        assert_refused_as_gone(
            serde_json::json!({ "agent_id": "a", "question": "q", "domain": "builder" }),
        )
        .await;
    }

    #[test]
    fn a_chat_start_is_not_refused_as_a_builder_start() {
        let chat = serde_json::json!({ "agent_id": "a.agentic.yml", "question": "q" });
        assert!(super::refuse_builder_start(&start(chat)).is_none());
    }

    #[test]
    fn only_a_builder_run_is_refused_an_answer() {
        use super::is_a_builder_run;
        assert!(is_a_builder_run(Some("builder")));
        for other in [Some("analytics"), Some("workflow"), Some("airway"), None] {
            assert!(!is_a_builder_run(other), "{other:?}");
        }
    }

    /// A Builder start or answer is `410 Gone`, saying the Builder was removed.
    #[tokio::test]
    async fn the_builder_refusal_is_gone() {
        use super::{BUILDER_REMOVED, builder_removed};
        let response = builder_removed("Starting a Builder run");
        assert_eq!(response.status(), axum::http::StatusCode::GONE);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["error"], BUILDER_REMOVED);
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
