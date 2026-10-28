//! `POST /api/projects/{project_id}/agents/{agent_id}/asks` (start)
//!
//! One-shot natural-language entry point for custom-app bundles. The bundle
//! starts an analytics-agent run and reads its events from
//! `GET …/agents/runs/{run_id}/events` (`agent_run_stream`) until a terminal
//! one, or a clarifying question.
//!
//! A live ask is **queued**: this handler checks the agent can start, records
//! the question and the caller, seeds the run with its `queued` Global entry,
//! and returns `202`. A driver anywhere on the fleet claims it and runs the
//! analytics pipeline (`executor`), so the replica that took the request may
//! go away without taking the ask with it. An ask on an app's staging host is
//! still driven in this process — see `start`.
//!
//! Pipeline integration: both paths build from `pipeline::ask_pipeline`, on
//! `agentic_pipeline::PipelineBuilder` — the SAME entry point the workspace
//! `/runs` route uses for the IDE's analytics surface. So verified-query
//! routing, clarification suspension, knowledge cards, tool selection are all
//! live in custom-app land too.

use std::future::Future;
use std::sync::Arc;

use agentic_pipeline::platform::PlatformContext;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use entity::{messages, organizations, threads};
use futures::future::BoxFuture;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use tracing::{error, instrument, warn};
use uuid::Uuid;

use super::agent_ask_staging::{ask_scope, thread_source};
use crate::server::api::custom_apps_gates::{
    CustomAppContextExt, check_custom_app_gates, parse_versioned_body,
};
use crate::server::previews::request_hold::scope_if;
use crate::server::router::AppState;

pub mod caller;
mod cancel;
pub mod executor;
mod pipeline;
mod start;
pub mod task;

use start::{drive_here, queue_ask};

pub use cancel::cancel_ask;

/// `make()`'s future, built in this frame and moved to the heap.
///
/// The ask handlers run on a server worker's 2 MiB stack, and a debug build
/// gives every future an `async fn` awaits its own slot in that fn's poll
/// frame — twice over under `#[instrument]` and axum's handler wrapper. With
/// the gates, the platform build and the pipeline start awaited in place,
/// the handler's three frames came to ~1.3 MiB and a production ask aborted
/// the server ("has overflowed its stack"). Built here instead, each leaves
/// a pointer in the handler's future and nothing in its frames.
fn boxed<'a, F>(make: impl FnOnce() -> F) -> BoxFuture<'a, F::Output>
where
    F: Future + Send + 'a,
{
    Box::pin(make())
}

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
    let gates_ctx = match boxed(|| check_custom_app_gates(&headers, project_id)).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // On the app's staging host: every write held, or refused outright.
    let hold = match boxed(|| ask_scope(&gates_ctx.db, &headers, &gates_ctx.user, project_id)).await
    {
        Ok(hold) => hold,
        Err(resp) => return resp,
    };
    // The history this ask belongs to: production's, or this app's staging.
    let source = thread_source(hold.as_ref());

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
    //    Arc<dyn PlatformContext>. Built inside a staging ask's hold, so it
    //    captures it and keeps holding on the task the run is driven on.
    let proj_ctx = match boxed(|| scope_if(hold.clone(), gates_ctx.build_project_context())).await {
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
            // and avoids confirming the existence of others' threads. The
            // thread must also be this environment's (`thread_source`): a
            // staging ask cannot append to a production thread, nor the
            // reverse — the same 404.
            match threads::Entity::find_by_id(tid).one(&gates_ctx.db).await {
                Ok(Some(t))
                    if t.project_id == project_id
                        && t.user_id == Some(gates_ctx.user.id)
                        && t.source == source =>
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
                source: Set(source),
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

    // 6. Start. A live ask is queued: a driver anywhere on the fleet claims
    //    it, so the replica that took this request may go away without taking
    //    the ask with it. A staging ask is driven here, in the request's
    //    process: its platform holds every write in this process, and a
    //    driver elsewhere has no such hold (recovery retires a stamped run).
    let run_id = match hold {
        None => {
            match boxed(|| {
                queue_ask(
                    &agentic_state.db,
                    &platform,
                    &gates_ctx,
                    &req,
                    thread_uuid,
                    &agent_id,
                )
            })
            .await
            {
                Ok(run_id) => run_id,
                Err(resp) => return resp,
            }
        }
        Some(hold) => {
            match boxed(|| {
                drive_here(
                    &agentic_state,
                    platform,
                    &gates_ctx,
                    &req,
                    thread_uuid,
                    &agent_id,
                    hold,
                )
            })
            .await
            {
                Ok(run_id) => run_id,
                Err(resp) => return resp,
            }
        }
    };

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
