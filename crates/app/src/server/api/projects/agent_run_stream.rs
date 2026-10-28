//! `GET /api/projects/{project_id}/agents/runs/{run_id}/events` — SSE
//! stream of agent-run events for custom-app bundles.
//!
//! Same agentic pipeline as `useAsk` (Phase 2) but with real-time
//! event delivery instead of polling. Bundle authors who want a
//! token-by-token chat UI (typing animation, partial answers,
//! mid-run artifacts) reach for `useAgentRun` instead of `useAsk`.
//!
//! Wire model: standard SSE. Each event has `id` (the DB sequence
//! number — clients send it back as `Last-Event-ID` after a
//! reconnect to resume from the right point) and `event` (the
//! ui-event type). Stream closes on terminal event (`done` / `error`
//! / `cancelled`).
//!
//! Auth: same gates as Phase 2's polling endpoints. Stream stays
//! open until terminal so the row scoping check runs once at
//! connect — sufficient because the cookie can't be revoked
//! mid-stream without disconnecting anyway.
//!
//! Why a separate handler from agentic-http's `stream_events`: the
//! agentic-http handler is workspace-scoped (its gates assume
//! workspace middleware). Customer-app endpoints use the public
//! router with cookie + origin auth. Sharing the implementation
//! would mean threading the auth-state choice through the SSE
//! generator. Cleaner to duplicate the event-emission loop here
//! (~60 lines of unique logic).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use agentic_runtime::crud::get_events_after;
use agentic_runtime::event_registry::EventRegistry;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tracing::{error, instrument};
use uuid::Uuid;

use crate::server::api::custom_apps_gates::check_custom_app_gates;
use crate::server::router::AppState;

#[derive(Serialize)]
struct ApiErr {
    message: String,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
        }),
    )
        .into_response()
}

#[instrument(skip_all, fields(project_id = %project_id, run_id = %run_id))]
pub async fn stream_agent_run(
    State(app_state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Response {
    // Customer-app gates run once at connect. The cookie-auth
    // protection is enough; once the stream is open the only data
    // flowing is events the agentic pipeline already approved for
    // this user.
    let _gates_ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let agentic_state = match app_state.agentic_state.as_ref() {
        Some(s) => s.clone(),
        None => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent runtime not configured in this deployment",
            );
        }
    };

    let last_seq = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(-1);

    // Resolve the run's source_type. Customer-app bundles only
    // create analytics runs but the registry is keyed on source so
    // we look it up rather than hardcode.
    //
    // The run is resolved WITHIN the project the gates admitted the caller
    // to. A run id is the caller's to type, so another project's run — and a
    // check run staff queued in this one outside production — answers the
    // same not-found as an id that names nothing: opening a stream must not
    // confirm a run exists (`get_run_in_workspace`).
    let run =
        match agentic_runtime::crud::get_run_in_workspace(&agentic_state.db, project_id, &run_id)
            .await
        {
            Ok(Some(r)) => r,
            Ok(None) => return err(StatusCode::NOT_FOUND, "run not found"),
            Err(e) => {
                error!(run_id = %run_id, error = %e, "stream: run lookup failed");
                return err(StatusCode::INTERNAL_SERVER_ERROR, "run lookup failed");
            }
        };

    let source_type = run
        .source_type
        .clone()
        .unwrap_or_else(|| "analytics".to_string());

    let db = agentic_state.db.clone();
    let registry: Arc<EventRegistry> = Arc::clone(&agentic_state.event_registry);
    let run_id_for_stream = run_id.clone();

    let stream = async_stream::stream! {
        let mut last_sent_seq = last_seq;
        let mut processor = registry.stream_processor(&source_type);
        let mut quiet_polls: u32 = 0;
        // Set once the row has read terminal: the next pass is the last.
        let mut row_terminal: Option<agentic_runtime::entity::run::Model> = None;

        loop {
            // Pull all new events past last_sent_seq.
            let rows = match get_events_after(&db, &run_id_for_stream, last_sent_seq).await {
                Ok(r) => r,
                Err(e) => {
                    error!(run_id = %run_id_for_stream, error = %e, "stream: db read failed");
                    break;
                }
            };
            quiet_polls = if rows.is_empty() { quiet_polls + 1 } else { 0 };

            let mut terminal = false;
            for row in rows {
                last_sent_seq = row.seq;

                // Pass-through for runtime events that aren't
                // domain-specific (e.g. recovery_resumed). Matches
                // the workspace SSE behavior.
                if row.event_type == "recovery_resumed" {
                    let event = SseEvent::default()
                        .id(row.seq.to_string())
                        .event("recovery_resumed")
                        .data(row.payload.to_string());
                    yield Ok::<_, Infallible>(event);
                    continue;
                }

                for (ui_event_type, mut ui_payload) in
                    processor.process(&row.event_type, &row.payload)
                {
                    // Inject attempt number so reconnecting clients
                    // can show the right "retry N" label.
                    if let serde_json::Value::Object(ref mut map) = ui_payload {
                        map.insert("attempt".into(), row.attempt.into());
                    }
                    let event = SseEvent::default()
                        .id(row.seq.to_string())
                        .event(&ui_event_type)
                        .data(ui_payload.to_string());
                    yield Ok(event);

                    // Terminal events close the stream cleanly. The
                    // SDK's EventSource consumer stops listening as
                    // soon as it sees one.
                    if is_terminal_event(&ui_event_type) {
                        terminal = true;
                    }
                }
            }

            if terminal {
                return;
            }

            // The log alone cannot say a run is over. A driver that dies
            // between the run's end and its terminal event leaves none, and
            // recovery reuses the sequence numbers of the partial events it
            // deletes, so a terminal event written after that can sit below
            // `last_sent_seq` where this loop never looks. The row is the
            // authority: once it reads terminal, the events already read are
            // re-read once more (the event may have landed just before the
            // row), and if none of them ends the stream, the row's status
            // does.
            if let Some(run) = row_terminal.take() {
                if let Some((event_type, payload)) =
                    agentic_http::routes::run::terminal_event_for_status(
                        run.task_status.as_deref(),
                        run.error_message.as_deref(),
                        run.answer.as_deref(),
                        &run.id,
                    )
                {
                    yield Ok(SseEvent::default()
                        .id(last_sent_seq.saturating_add(1).to_string())
                        .event(event_type)
                        .data(payload.to_string()));
                }
                return;
            }
            if quiet_polls > 0 && quiet_polls % ROW_CHECK_EVERY_QUIET_POLLS == 0 {
                match agentic_runtime::crud::get_run(&db, &run_id_for_stream).await {
                    Ok(Some(run)) if run_is_over(run.task_status.as_deref()) => {
                        row_terminal = Some(run);
                        continue;
                    }
                    // Still going, or a failed read: keep streaming.
                    _ => {}
                }
            }

            // Poll cadence: 250 ms between sweeps. Matches what the
            // workspace handler uses as its notifier-free fallback;
            // tight enough for token-like UX, loose enough that a
            // long-running quiet run doesn't burn DB connections.
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };

    // Axum polls this body *after* the handler future resolves, so the outer
    // tower layer's `Hub::run` has already unwound and `Hub::current()` on the
    // polling task is the untagged default. Read the request's hub here, while
    // it is still current, and bind it across every poll: the `error!` above is
    // on this module's target, which barrier 1 does not drop, and it — like a
    // panic under the same poll — carries a tenant agent run's id and error
    // text (`middlewares::sentry_surface`).
    let stream = crate::server::api::middlewares::sentry_surface::bind_hub_stream(
        stream,
        sentry::Hub::current(),
    );

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Quiet 250 ms polls between reads of the run row: one read every 2 s while
/// nothing arrives, never while events are flowing.
const ROW_CHECK_EVERY_QUIET_POLLS: u32 = 8;

/// Does this `task_status` mean nothing more is coming? `awaiting_input` is
/// not over: the stream has already carried the question, and the bundle
/// stops on that event itself.
fn run_is_over(status: Option<&str>) -> bool {
    matches!(
        status,
        Some("done") | Some("failed") | Some("cancelled") | Some("timed_out")
    )
}

/// Customer-app SSE bundles see a small subset of the analytics
/// UI-event taxonomy. Terminal events are the ones that should close
/// the stream. Keep the set explicit so a new UI event added
/// upstream doesn't accidentally truncate streams here.
fn is_terminal_event(ui_event_type: &str) -> bool {
    matches!(ui_event_type, "done" | "error" | "cancelled" | "failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row closes the stream only when the run has ended. A run parked on
    /// a question stays open: the bundle stops on the `awaiting_input` event
    /// itself, and closing here would hand it an end the run never reached.
    #[test]
    fn only_an_ended_run_closes_the_stream() {
        for over in ["done", "failed", "cancelled", "timed_out"] {
            assert!(run_is_over(Some(over)), "{over}");
        }
        for going in [
            "running",
            "awaiting_input",
            "delegating",
            "needs_resume",
            "shutdown",
        ] {
            assert!(!run_is_over(Some(going)), "{going}");
        }
        assert!(!run_is_over(None));
    }

    /// Every status the row can close the stream with maps to an event the
    /// bundle ends on — so a closed stream is never one the SDK reconnects to.
    #[test]
    fn every_ending_the_row_reports_is_an_event_the_bundle_stops_on() {
        for over in ["done", "failed", "cancelled", "timed_out"] {
            let (event, _) = agentic_http::routes::run::terminal_event_for_status(
                Some(over),
                Some("why"),
                None,
                "run-1",
            )
            .expect("terminal");
            assert!(is_terminal_event(event), "{over} -> {event}");
        }
    }
}
