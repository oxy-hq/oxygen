//! Automation run lifecycle handlers.
//!
//! Mirrors the analytics-side `/runs` shape but uses the automation primitives
//! end-to-end: automations are seeded from `.automation.yml` files, the decider
//! drives them, and SSE just streams whatever ends up in
//! `agentic_run_events`. No `OutputContainer` translation, no
//! `WorkflowLauncher` plumbing — events and final state are returned as
//! native JSON.
//!
//! ## Routes
//!
//! | Method | Path                                | Purpose |
//! |--------|-------------------------------------|---------|
//! | POST   | `/agentic-workflows/runs`                   | Start a run |
//! | GET    | `/agentic-workflows/runs/:id`               | Snapshot (status + state) |
//! | GET    | `/agentic-workflows/runs/:id/events`        | SSE event stream (replay + live) |
//! | POST   | `/agentic-workflows/runs/:id/cancel`        | Cancel a running automation |
//! | GET    | `/agentic-workflows/files`                  | List runnable automation files |
//! | GET    | `/agentic-workflows/files/:path_b64`        | Fetch an automation file's text |

use std::sync::Arc;

use axum::{
    Json,
    extract::{Extension, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::{mpsc, watch};

use agentic_pipeline::WorkflowWorkspaceContext;
use agentic_pipeline::automation_run::{
    AutomationRunError, StartAutomationRequest, get_automation_snapshot, list_automation_runs,
    spawn_automation_run_drive, start_automation_run,
};
use agentic_pipeline::platform::{BuilderBridges, PlatformContext};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use uuid::Uuid;

use super::run_scope::ensure_run_access;
use crate::state::AgenticState;

// ── Response types (request shape comes from agentic_pipeline) ─────────────

#[derive(Serialize)]
pub struct CreateAutomationRunResponse {
    pub run_id: String,
}

#[derive(Serialize)]
pub struct AutomationFile {
    /// Relative-to-workspace path, suitable for use as `workflow_ref`.
    pub path: String,
    /// URL-safe-base64 of `path`, for use in the `:path_b64` route param
    /// (a literal slash inside a path segment is otherwise ambiguous).
    pub path_b64: String,
    /// Top-level `description` from the automation YAML, when present.
    /// Trimmed and truncated server-side; `None` for missing/empty
    /// descriptions and for files that fail to read or parse.
    pub description: Option<String>,
}

#[derive(Serialize)]
pub struct AutomationFileContent {
    pub path: String,
    pub content: String,
}

// ── Path extractors ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AutomationRunIdPath {
    id: String,
}

#[derive(Deserialize)]
pub struct AutomationFilePath {
    path_b64: String,
}

// ── POST /agentic-workflows/runs ───────────────────────────────────────────────────

pub async fn create_automation_run(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    Extension(bridges): Extension<BuilderBridges>,
    Json(body): Json<StartAutomationRequest>,
) -> Response {
    // Interactive run: a co-located scoped coordinator is spawned right
    // below via `spawn_automation_run_drive`, so it owns this run's tree.
    // workspace_id stamped from the platform context (injected by the
    // app's workspace_middleware from the `/{workspace_id}/...` path) so
    // out-of-process drivers can route this row back to its workspace.
    let workspace_id = platform.workspace_id();
    let run_id = match start_automation_run(
        &state.db,
        body,
        agentic_pipeline::TaskScope::Scoped,
        workspace_id,
    )
    .await
    {
        Ok(id) => id,
        Err(AutomationRunError::InvalidInput(msg)) => {
            return (StatusCode::BAD_REQUEST, msg).into_response();
        }
        Err(e) => {
            tracing::error!(%e, "create_automation_run: start failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("start: {e}")).into_response();
        }
    };

    // Register cancel + answer channels so the run is cancellable via SSE.
    // The cancel_rx is handed to `spawn_automation_run_drive`'s forwarder
    // task — dropping it here would leave `state.cancel(run_id)` with
    // nothing listening, so the Stop button on the run page hangs in its
    // loading state until the run finishes on its own.
    let (answer_tx, _answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state.register(&run_id, answer_tx, cancel_tx);

    // Without this drive, the queued `TaskSpec::Automation` row in
    // `agentic_task_queue` would sit forever — the per-request
    // coordinator + worker is what claims it and emits events.
    spawn_automation_run_drive(
        state.db.clone(),
        state.runtime.clone(),
        run_id.clone(),
        platform,
        Some(bridges),
        cancel_rx,
        state.router.clone(),
    );

    Json(CreateAutomationRunResponse { run_id }).into_response()
}

// ── GET /agentic-workflows/runs/:id ────────────────────────────────────────────────

pub async fn get_automation_run(
    Path(AutomationRunIdPath { id: run_id }): Path<AutomationRunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Response {
    if let Err(resp) = ensure_run_access(&state, &user.id, &run_id, platform.workspace_id()).await {
        return resp;
    }
    match get_automation_snapshot(&state.db, &run_id).await {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(AutomationRunError::NotFound) => {
            (StatusCode::NOT_FOUND, "run not found").into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("snapshot: {e}")).into_response(),
    }
}

// ── GET /agentic-workflows/runs?workflow_ref=... ───────────────────────────────────

#[derive(Deserialize)]
pub struct ListRunsQuery {
    pub workflow_ref: String,
    /// Hard cap to keep response sizes bounded; the dropdown only shows
    /// the most-recent N anyway. Defaults to 50.
    #[serde(default)]
    pub limit: Option<u64>,
}

/// Workspace-scoped: `workflow_ref` is a workspace-relative path, so it is
/// only unique together with the workspace. `platform.workspace_id()` is the
/// id `start_automation_run` stamps on the run (the nil UUID in local mode).
pub async fn list_runs_for_automation(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    axum::extract::Query(q): axum::extract::Query<ListRunsQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).min(200);
    match list_automation_runs(&state.db, platform.workspace_id(), &q.workflow_ref, limit).await {
        Ok(runs) => Json(runs).into_response(),
        Err(e) => {
            tracing::error!(%e, "list_runs_for_automation failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("list runs: {e}")).into_response()
        }
    }
}

// ── GET /agentic-workflows/threads/:thread_id/run ──────────────────────────────────
//
// Returns the most recent automation run linked to this thread (i.e. its
// `agentic_runs.thread_id` matches and `source_type = 'workflow'`). Used by
// the chat-thread automation page to recover state on reload — the
// in-memory log buffer is empty after a refresh, so we re-derive it from
// the stored events for the latest run.

#[derive(Deserialize)]
pub struct ThreadIdPath {
    thread_id: String,
}

pub async fn latest_run_for_thread(
    Path(ThreadIdPath { thread_id }): Path<ThreadIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Response {
    let thread_uuid = match Uuid::parse_str(&thread_id) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid thread_id").into_response(),
    };

    // Mirror the analytics-side `get_run_by_thread` guard — without this an
    // authenticated workspace member can fetch the latest automation run for
    // any thread_id and observe run_id, task_status, and timestamps for
    // threads owned by other users.
    match state.thread_owner.thread_owner(thread_uuid).await {
        Ok(None) => return (StatusCode::NOT_FOUND, "thread not found").into_response(),
        Ok(Some(Some(owner_id))) if owner_id != user.id => {
            return (StatusCode::FORBIDDEN, "access denied").into_response();
        }
        Ok(_) => {}
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response();
        }
    }

    // `get_run_by_thread` is source-type-agnostic — the same thread might
    // also have an analytics run linked to it. Filter to automation rows so
    // we don't accidentally hand the chat-thread page an analytics run id
    // it can't replay through the automation event aggregator.
    match agentic_runtime::crud::get_runs_by_thread(&state.db, thread_uuid).await {
        Ok(runs) => {
            let latest = runs
                .into_iter()
                .rev()
                .find(|r| r.source_type.as_deref() == Some(agentic_pipeline::WORKFLOW_SOURCE_TYPE));
            match latest {
                Some(run) => Json(serde_json::json!({
                    "run_id": run.id,
                    "task_status": run.task_status,
                    "created_at": run.created_at,
                    "updated_at": run.updated_at,
                }))
                .into_response(),
                None => {
                    (StatusCode::NOT_FOUND, "no automation run for this thread").into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!(%e, "latest_run_for_thread failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")).into_response()
        }
    }
}

// ── POST /agentic-workflows/runs/:id/cancel ────────────────────────────────────────

pub async fn cancel_automation_run(
    Path(AutomationRunIdPath { id: run_id }): Path<AutomationRunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Response {
    if let Err(resp) = ensure_run_access(&state, &user.id, &run_id, platform.workspace_id()).await {
        return resp;
    }
    // Durable, cross-process cancel signal: a recovered / Global run is
    // driven out-of-process (periodic loop / future standalone worker),
    // where the in-memory `state.cancel` watch never reaches it. Its
    // cancel forwarder polls this flag. Harmless for interactive runs.
    agentic_runtime::crud::request_cancel(&state.db, &run_id)
        .await
        .ok();
    if !state.cancel(&run_id) {
        // No live cancel channel — two indistinguishable cases:
        //   1. Coordinator never ran (stuck queue row), OR
        //   2. Coordinator just finished cleanly and `deregister`'d
        //      the cancel channel before we got here.
        //
        // Case 1 wants the defensive `update_run_failed` so the
        // stop button stops spinning. Case 2 is the race we must
        // NOT corrupt: a successful `done` run would otherwise get
        // rewritten to `failed("cancelled by user")` and on reload
        // show as failed.
        //
        // Gate the write on `task_status` being non-terminal. The
        // status enum lives in `crud::user_facing_status`:
        // `done | failed | cancelled | timed_out` are terminal,
        // anything else (running, delegating, awaiting_input, null)
        // is fair game for the defensive cancel.
        let already_terminal = match agentic_runtime::crud::get_run(&state.db, &run_id).await {
            Ok(Some(run)) => matches!(
                run.task_status.as_deref(),
                Some("done") | Some("failed") | Some("cancelled") | Some("timed_out")
            ),
            // No row → nothing to update; pretend terminal so the
            // defensive write is skipped (a 404 would be misleading
            // since the user did just click Stop).
            Ok(None) => true,
            Err(e) => {
                tracing::warn!(%run_id, error = %e, "cancel: status lookup failed, skipping defensive write");
                true
            }
        };
        if !already_terminal
            && let Err(e) =
                agentic_runtime::crud::update_run_failed(&state.db, &run_id, "cancelled by user")
                    .await
        {
            tracing::warn!(%run_id, error = %e, "cancel: defensive DB update failed");
        }
        state.notify(&run_id);
        // Drop the notifier so the SSE loop's `state.notifiers.contains_key`
        // check breaks and the client observes a clean stream close.
        state.notifiers.remove(&run_id);
        tracing::debug!(%run_id, already_terminal, "cancel: no live channel; closed SSE");
    }
    StatusCode::NO_CONTENT.into_response()
}

// ── GET /agentic-workflows/files ───────────────────────────────────────────────────

pub async fn list_automation_files(
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
) -> Response {
    let workspace: Arc<dyn WorkflowWorkspaceContext> = platform.clone();
    let paths = match workspace.list_automation_files().await {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list files: {e}"),
            )
                .into_response();
        }
    };
    // `list_automation_files` returns workspace-relative paths, so the strip
    // this used to do was already the identity. Dropping it also drops the last
    // reason this lister needed a workspace root.
    let mut files: Vec<AutomationFile> = Vec::with_capacity(paths.len());
    for rel_path in paths {
        let rel = rel_path.to_string_lossy().to_string();
        let path_b64 = URL_SAFE_NO_PAD.encode(rel.as_bytes());
        let description = read_automation_description(&workspace, &rel).await;
        files.push(AutomationFile {
            path: rel,
            path_b64,
            description,
        });
    }
    Json(files).into_response()
}

/// Best-effort description lookup for one listed file. Reads content
/// through the same `resolve_automation_yaml` path `get_automation_file`
/// uses, then parses the top-level `description`. Any read failure
/// degrades to `None` — a malformed automation must not break the listing.
async fn read_automation_description(
    workspace: &Arc<dyn WorkflowWorkspaceContext>,
    rel_path: &str,
) -> Option<String> {
    let content = workspace.resolve_automation_yaml(rel_path).await.ok()?;
    yaml_description(&content)
}

/// Hard cap on the listed description length; the UI clamps further.
const MAX_DESCRIPTION_CHARS: usize = 200;

/// Extract the top-level `description` field from automation YAML.
///
/// Defensive by design: invalid YAML, a non-mapping root, a missing or
/// non-string `description`, and a whitespace-only value all yield
/// `None`. The value is trimmed and truncated to
/// [`MAX_DESCRIPTION_CHARS`] (on a char boundary).
fn yaml_description(content: &str) -> Option<String> {
    let value = serde_yaml::from_str::<serde_yaml::Value>(content).ok()?;
    let desc = value.get("description")?.as_str()?.trim();
    if desc.is_empty() {
        return None;
    }
    Some(desc.chars().take(MAX_DESCRIPTION_CHARS).collect())
}

// ── GET /agentic-workflows/files/:path_b64 ─────────────────────────────────────────

pub async fn get_automation_file(
    Path(AutomationFilePath { path_b64 }): Path<AutomationFilePath>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
) -> Response {
    let raw = match decode_path_b64_tolerant(&path_b64) {
        Ok(b) => b,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid path_b64").into_response();
        }
    };
    let path = match String::from_utf8(raw) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "path_b64 not utf-8").into_response(),
    };

    let workspace: Arc<dyn WorkflowWorkspaceContext> = platform.clone();
    match workspace.resolve_automation_yaml(&path).await {
        Ok(content) => Json(AutomationFileContent { path, content }).into_response(),
        // Present and unparseable. Permanent, and the caller's to fix — so
        // neither 404 (it exists) nor 503 (retrying cannot help).
        Err(e) if e.is_invalid() => {
            (StatusCode::UNPROCESSABLE_ENTITY, format!("read file: {e}")).into_response()
        }
        // Could not look: no working copy and a boundary lookup that failed, or
        // a read that failed on a node that has the files. 503 + Retry-After,
        // matching the airway start path. Answering 404 told the caller their
        // automation had been deleted and told a retrying scheduler to stop.
        Err(e) if e.is_unavailable() => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(
                axum::http::header::RETRY_AFTER,
                agentic_pipeline::executor::AIRWAY_UNAVAILABLE_RETRY_SECS.to_string(),
            )],
            format!("read file: {e}"),
        )
            .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, format!("read file: {e}")).into_response(),
    }
}

/// Decode a `path_b64` route param tolerantly: accept both the
/// URL-safe alphabet (`-`/`_`, what the server emits at
/// `list_automation_files`) and the standard alphabet (`+`/`/`, what
/// `btoa` produces on the FE), with or without `=` padding.
///
/// Why tolerant: the FE's `encodeBase64` wraps `btoa`, which always
/// emits standard-alphabet base64 *with* padding (e.g. `…ymb=…`).
/// Our server-side emit path uses `URL_SAFE_NO_PAD`. A strict
/// `URL_SAFE_NO_PAD.decode` rejects everything the FE sends; a
/// strict `STANDARD.decode` rejects what the server emits. Rather
/// than coupling client and server to the same exact encoder, we
/// normalize to URL-safe + strip padding before decoding so any
/// reasonable producer (FE, CLI, manual `base64 -w0`) round-trips.
///
/// Bytes go through one decoder; this isn't a performance concern.
fn decode_path_b64_tolerant(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    // Normalize standard alphabet to URL-safe, drop any padding.
    // `.filter` strips `=`; `.map` rewrites `+`/`/`. We don't
    // validate further — the underlying decoder rejects anything
    // that isn't a valid base64 character with a clear error.
    let normalized: String = input
        .chars()
        .filter(|&c| c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    URL_SAFE_NO_PAD.decode(normalized.as_bytes())
}

#[cfg(test)]
mod path_b64_tests {
    use super::decode_path_b64_tolerant;

    const PATH: &str = "workflows/external-factors-correlation.procedure.yml";
    // URL-safe, no padding — what `list_automation_files` emits.
    const URL_SAFE: &str = "d29ya2Zsb3dzL2V4dGVybmFsLWZhY3RvcnMtY29ycmVsYXRpb24ucHJvY2VkdXJlLnltbA";
    // Standard alphabet, with padding — what `btoa(...)` produces.
    const STANDARD_PADDED: &str =
        "d29ya2Zsb3dzL2V4dGVybmFsLWZhY3RvcnMtY29ycmVsYXRpb24ucHJvY2VkdXJlLnltbA==";

    #[test]
    fn accepts_url_safe_no_pad() {
        let bytes = decode_path_b64_tolerant(URL_SAFE).expect("decode url-safe");
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), PATH);
    }

    #[test]
    fn accepts_standard_with_padding() {
        let bytes = decode_path_b64_tolerant(STANDARD_PADDED).expect("decode padded");
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), PATH);
    }

    #[test]
    fn accepts_url_safe_with_padding() {
        // Mixed scenario: URL-safe alphabet but `=` padding kept.
        let mixed = format!("{URL_SAFE}==");
        let bytes = decode_path_b64_tolerant(&mixed).expect("decode mixed");
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), PATH);
    }

    #[test]
    fn rejects_garbage() {
        // Invalid base64 chars (space, `#`) must still error rather
        // than silently producing wrong bytes.
        assert!(decode_path_b64_tolerant("not base64 #").is_err());
    }
}

#[cfg(test)]
mod yaml_description_tests {
    use super::{MAX_DESCRIPTION_CHARS, yaml_description};

    #[test]
    fn present_returns_trimmed() {
        let yaml = "name: revenue\ndescription: '  Weekly revenue rollup  '\ntasks: []\n";
        assert_eq!(
            yaml_description(yaml).as_deref(),
            Some("Weekly revenue rollup")
        );
    }

    #[test]
    fn absent_returns_none() {
        assert_eq!(yaml_description("name: revenue\ntasks: []\n"), None);
    }

    #[test]
    fn invalid_yaml_returns_none() {
        // Unterminated flow sequence — parse error, not a panic.
        assert_eq!(yaml_description("name: [unclosed"), None);
        // Valid YAML but not a mapping at the root.
        assert_eq!(yaml_description("- a\n- b\n"), None);
        // Non-string description (e.g. accidental number).
        assert_eq!(yaml_description("description: 42\n"), None);
        // Whitespace-only description collapses to None after trim.
        assert_eq!(yaml_description("description: '   '\n"), None);
    }

    #[test]
    fn long_value_truncated() {
        let long = "x".repeat(MAX_DESCRIPTION_CHARS + 300);
        let yaml = format!("description: {long}\n");
        let got = yaml_description(&yaml).expect("description should parse");
        assert_eq!(got.chars().count(), MAX_DESCRIPTION_CHARS);
    }
}
