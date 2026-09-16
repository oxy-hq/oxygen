//! `POST /api/projects/{project_id}/agents/{agent_id}/asks` (start)
//! `GET  /api/projects/{project_id}/agents/asks/{run_id}` (poll)
//!
//! One-shot natural-language entry point for custom-app bundles.
//! The bundle starts an analytics-agent run; the server kicks off
//! the pipeline (verified-query routing, semantic compile,
//! interpretation) and writes events into the existing
//! `agentic_runs` + `agentic_run_events` tables. The bundle polls
//! the GET endpoint until the run reaches a terminal state, then
//! renders the final answer + structured artifacts.
//!
//! Shape decision — **poll, not stream** — was made deliberately in
//! the SDK extensions spec (see customer-apps.md § "SDK extensions").
//! SSE is offered separately via `useAgentRun` for bundles that want
//! token-by-token streaming. One-shot polling covers ~80% of real
//! use cases at a fraction of the SDK weight.
//!
//! Pipeline integration: this handler reuses
//! `agentic_pipeline::PipelineBuilder` — the SAME entry point the
//! workspace `/runs` route uses for the IDE's analytics surface. So
//! verified-query routing, clarification suspension, knowledge cards,
//! tool selection are all live in custom-app land too.

use std::sync::Arc;

use agentic_pipeline::PipelineBuilder;
use agentic_pipeline::platform::PlatformContext;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use entity::{messages, organizations, threads};
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait, Set};
use sentry::SentryFutureExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tracing::{error, instrument, warn};
use uuid::Uuid;

use crate::server::api::custom_apps_gates::{check_custom_app_gates, parse_versioned_body};
use crate::server::router::AppState;

/// Build the relative path to the thread view in the oxy web app.
///
/// Local mode (no org_id): `/threads/<thread_id>` — the web-app
/// routes from root with no workspace switcher.
///
/// Cloud / multi-workspace: `/<org_slug>/workspaces/<workspace_id>/threads/<thread_id>`
/// — the URL the web-app constructs itself via
/// `routes.ORG(orgSlug).WORKSPACE(wsId).THREAD(threadId)`. We
/// mirror it so a bundle clicking the link lands in the right org
/// and workspace without a redirect dance.
///
/// Best-effort: if org_id is set but the org row is gone (deleted
/// between when the workspace was created and now), we fall back
/// to the local-mode path so the link doesn't 404 entirely. The
/// log is loud enough to catch this in practice.
async fn build_thread_path(
    db: &DatabaseConnection,
    workspace: &entity::workspaces::Model,
    thread_id: &str,
) -> String {
    let Some(org_id) = workspace.org_id else {
        return format!("/threads/{thread_id}");
    };
    let org = organizations::Entity::find_by_id(org_id).one(db).await;
    match org {
        Ok(Some(org)) => format!(
            "/{}/workspaces/{}/threads/{thread_id}",
            org.slug, workspace.id
        ),
        Ok(None) | Err(_) => {
            warn!(
                org_id = %org_id,
                workspace_id = %workspace.id,
                "thread_url: org missing or db error — emitting local-mode path"
            );
            format!("/threads/{thread_id}")
        }
    }
}

#[derive(Serialize)]
struct ApiErr {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    /// Free-text hint for the bundle author: what the server tried,
    /// what to check. Surfaced verbatim in the SDK error so the
    /// bundle can render it next to the error message without
    /// having to interpret the code. Used heavily on `*_not_found`
    /// responses to point at the right place.
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: None,
            hint: None,
        }),
    )
        .into_response()
}

fn err_with_code(status: StatusCode, msg: impl Into<String>, code: &'static str) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: Some(code),
            hint: None,
        }),
    )
        .into_response()
}

fn err_with_hint(
    status: StatusCode,
    msg: impl Into<String>,
    code: &'static str,
    hint: impl Into<String>,
) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: Some(code),
            hint: Some(hint.into()),
        }),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskRequest {
    /// Natural-language question. Required, non-empty after trim.
    pub question: String,
    /// Optional conversation continuity. When set, the agent picks up
    /// from prior context on the same thread. Bundle authors should
    /// echo the `thread_id` returned by a prior `/asks` response on
    /// follow-up questions.
    #[serde(default)]
    pub thread_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AskStartResponse {
    pub run_id: String,
    pub thread_id: String,
    /// Relative path to inspect this conversation in the oxy admin
    /// UI (e.g. `/threads/<thread_id>`). Bundles render as a
    /// `<a href={threadUrl} target="_blank">View in Oxy ↗</a>` so
    /// the user can click through to the full conversation, replay
    /// the agent's reasoning, inspect the SQL it ran. Relative so
    /// the browser resolves against whatever origin is serving the
    /// bundle — no need for the server to know its own host.
    pub thread_url: String,
}

/// `POST /api/projects/{project_id}/agents/{agent_id}/asks`
#[instrument(skip_all, fields(project_id = %project_id, agent_id = %agent_id))]
pub async fn start_ask(
    State(app_state): State<AppState>,
    Path((project_id, agent_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // 1. Gates.
    let gates_ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    // 2. Body.
    let req: AskRequest = match parse_versioned_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if req.question.trim().is_empty() {
        return err_with_code(
            StatusCode::BAD_REQUEST,
            "`question` must be non-empty",
            "agent_question_empty",
        );
    }

    // 3. Agentic state must be available — the custom-app router
    //    populates it; 503 if a deployment shape missed it.
    let agentic_state = match app_state.agentic_state.as_ref() {
        Some(s) => s.clone(),
        None => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent runtime not configured in this deployment",
            );
        }
    };

    // 4. Platform context (project-scoped). Pipeline needs an
    //    Arc<dyn PlatformContext>.
    let proj_ctx = match gates_ctx.build_project_context().await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let platform: Arc<dyn PlatformContext> = Arc::new(proj_ctx);

    // 5. Resolve the thread. Reuse the client's thread on a follow-up;
    //    otherwise provision one OWNED BY THE VIEWER (`user_id`) so it
    //    surfaces in their bundle chat history (the auto-provision path in
    //    `thread_owner.rs` leaves `user_id` null — no per-request user
    //    there — which would make history un-scopable). Then record the
    //    question as a human message so the transcript is reconstructable
    //    on restore (the answer is replayed from the run's persisted
    //    events; the bundle drive doesn't persist assistant messages).
    let thread_uuid = match req
        .thread_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        Some(tid) => {
            // Ownership gate: a client-supplied thread must belong to THIS
            // viewer in THIS project before we append to it. Without this an
            // authenticated caller who clears the gate for `project_id` could
            // pass any thread UUID and inject a message into — and pull prior
            // context out of — another user's/project's thread. A 404 (not
            // 403) mirrors `custom_apps_threads.rs::get_thread_transcript`
            // and avoids confirming the existence of others' threads.
            match threads::Entity::find_by_id(tid).one(&gates_ctx.db).await {
                Ok(Some(t))
                    if t.project_id == project_id && t.user_id == Some(gates_ctx.user.id) =>
                {
                    tid
                }
                Ok(_) => return err(StatusCode::NOT_FOUND, "thread not found"),
                Err(e) => {
                    error!(error = %e, "custom-app ask: thread ownership lookup failed");
                    return err(StatusCode::INTERNAL_SERVER_ERROR, "thread lookup failed");
                }
            }
        }
        None => {
            let tid = Uuid::new_v4();
            let title: String = req.question.chars().take(120).collect();
            let thread = threads::ActiveModel {
                id: Set(tid),
                user_id: Set(Some(gates_ctx.user.id)),
                title: Set(title),
                input: Set(req.question.clone()),
                output: Set(String::new()),
                source: Set("custom-app".to_string()),
                source_type: Set("analytics".to_string()),
                references: Set("[]".to_string()),
                is_processing: Set(false),
                created_at: ActiveValue::not_set(),
                project_id: Set(project_id),
                sandbox_info: Set(None),
            };
            if let Err(e) = threads::Entity::insert(thread).exec(&gates_ctx.db).await {
                error!(error = %e, "custom-app ask: thread create failed");
                return err(StatusCode::INTERNAL_SERVER_ERROR, "could not create thread");
            }
            tid
        }
    };

    // Persist the question (new thread or follow-up) as a human message.
    // Fatal: this runs BEFORE the run is started, and the transcript endpoint
    // pairs human messages to runs positionally — a silently-dropped question
    // would shift every later turn's answer by one on restore. Failing here
    // (before any run exists) keeps the question/run counts aligned.
    let question_msg = messages::ActiveModel {
        id: Set(Uuid::new_v4()),
        content: Set(req.question.clone()),
        is_human: Set(true),
        thread_id: Set(thread_uuid),
        created_at: ActiveValue::not_set(),
        input_tokens: Set(0),
        output_tokens: Set(0),
    };
    if let Err(e) = messages::Entity::insert(question_msg)
        .exec(&gates_ctx.db)
        .await
    {
        error!(error = %e, "custom-app ask: question message persist failed");
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not record question",
        );
    }

    // 6. Build the pipeline. Analytics domain only — builder is
    //    workspace-scoped and not exposed to bundles.
    let builder = PipelineBuilder::new(platform.clone())
        .workspace_id(project_id)
        .question(&req.question)
        .schema_cache(Arc::clone(&agentic_state.schema_cache))
        .thread(thread_uuid)
        .analytics(&agent_id);

    // 7. Start.
    let started = match builder.start(&agentic_state.db).await {
        Ok(s) => s,
        Err(e) => {
            warn!(
                agent_id = %agent_id,
                error = %e,
                "custom-app ask: pipeline start failed"
            );
            // Distinguish "agent not found" so the SDK can surface a
            // sharper error than a generic 500.
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
            return err(StatusCode::INTERNAL_SERVER_ERROR, msg);
        }
    };

    let run_id = started.run_id.clone();

    // 7. Hook up answer + cancel channels.
    //
    // `cancel_tx` is the live wire — `cancel_ask` (below) calls
    // `RuntimeState::cancel_run`, which signals through this watch
    // channel to abort the coordinator's drive loop on the same
    // instance. Without registering it, the cancel endpoint would
    // only stamp the DB row and the drive loop would keep burning
    // LLM budget until natural completion.
    //
    // `answer_tx` is registered but the custom-app surface
    // doesn't expose a `/answer` endpoint — clarification answers
    // go through `useAgentRun`'s re-`ask()` (new run, same
    // thread_id) rather than resuming a suspended run. The drive
    // path still calls `RuntimeState::take_answer_rx` and holds
    // the receiver, so the channel must exist or the runtime
    // panics on the missing-receiver path. The unused tx side is
    // dropped naturally when the spawned task exits.
    let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    agentic_state.register(&run_id, answer_tx, cancel_tx);

    // 8. Drive in background via the coordinator. Same path the
    //    workspace `/runs` route uses; HITL answers + cancellation
    //    flow through `RuntimeState::answer_txs` / `cancel_txs`
    //    registered above. Builder bridges are `None` because
    //    analytics is the only domain reachable from bundles.
    let db = agentic_state.db.clone();
    let runtime_state = agentic_state.runtime.clone();
    let schema_cache = Some(agentic_state.schema_cache.clone());
    let builder_test_runner = agentic_state.builder_test_runner.clone();
    let builder_app_runner = agentic_state.builder_app_runner.clone();
    let router = agentic_state.router.clone();
    let run_id_for_drive = run_id.clone();
    let platform_drive = platform.clone();
    tokio::spawn(
        async move {
            agentic_pipeline::drive_with_coordinator(
                started,
                db,
                runtime_state,
                answer_rx,
                cancel_rx,
                platform_drive,
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
        .bind_hub(sentry::Hub::current()),
    );

    // The thread is always provisioned now (§5), so return its id.
    let thread_id_str = thread_uuid.to_string();
    let thread_url =
        build_thread_path(&agentic_state.db, &gates_ctx.workspace, &thread_id_str).await;

    let resp = AskStartResponse {
        run_id,
        thread_id: thread_id_str,
        thread_url,
    };
    (StatusCode::ACCEPTED, Json(resp)).into_response()
}
/// `POST /api/projects/{project_id}/agents/asks/{run_id}/cancel`
///
/// Stop an in-flight agent run. Idempotent. Returns 204 on success.
/// `RuntimeState::cancel_run` handles both the live-run case
/// (propagates through the cancel watch channel to the coordinator
/// + pipeline drivers) and the stale case (marks `failed` in DB so
/// pollers don't hang).
#[instrument(skip_all, fields(project_id = %project_id, run_id = %run_id))]
pub async fn cancel_ask(
    State(app_state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Response {
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

    let run = match agentic_runtime::crud::get_run(&agentic_state.db, &run_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return err_with_code(
                StatusCode::NOT_FOUND,
                "run not found",
                "agent_run_not_found",
            );
        }
        Err(e) => {
            error!(run_id = %run_id, error = %e, "cancel: run lookup failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "run lookup failed");
        }
    };
    if run.workspace_id != project_id {
        return err_with_code(
            StatusCode::FORBIDDEN,
            "run does not belong to this project",
            "thread_project_mismatch",
        );
    }

    if let Err(e) = agentic_state
        .runtime
        .cancel_run(&agentic_state.db, &run_id)
        .await
    {
        error!(run_id = %run_id, error = ?e, "cancel: cancel_run failed");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "cancel failed");
    }
    StatusCode::NO_CONTENT.into_response()
}
