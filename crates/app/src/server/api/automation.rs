//! Compile-boundary-backed automation (a.k.a. workflow) listing.
//!
//! The customer-nav sidebar's "Automations" list must render on a stateless serve
//! replica that has no working copy. The legacy `/agentic-workflows/files`
//! lister lives in the `agentic-http` crate (which by the layering rules cannot
//! reach `compiled_reader`) and walks the filesystem, so it 502s on a serve node
//! when the ide is unreachable. This FleetOk endpoint serves the same list from
//! the compile boundary (`procedure_definitions`), falling through to the
//! filesystem in local / not-yet-promoted mode — mirroring `app::list_apps`.

use axum::extract::{self, Query};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;

#[derive(Debug, Deserialize)]
pub struct ListAutomationsQuery {
    /// Active branch hint; the boundary only serves the workspace default branch.
    pub branch: Option<String>,
}

/// One automation file, matching the `agentic-http` `AutomationFile` shape the FE
/// already consumes: a workspace-relative `path` and its `URL_SAFE_NO_PAD`
/// base64 (`path_b64`) used in the workflow route + single-file fetch. The
/// single-file fetch decodes `path_b64` tolerantly, so emitting the same
/// `URL_SAFE_NO_PAD` form the legacy lister used keeps every link round-tripping.
#[derive(Debug, Serialize)]
pub struct AutomationFileInfo {
    pub path: String,
    pub path_b64: String,
}

fn to_file(relative_path: String) -> AutomationFileInfo {
    let path_b64 = URL_SAFE_NO_PAD.encode(relative_path.as_bytes());
    AutomationFileInfo {
        path: relative_path,
        path_b64,
    }
}

/// `GET /{workspace_id}/procedures` — FleetOk. Serves the automation list from the
/// compile boundary, falling through to the filesystem only in local /
/// not-yet-promoted mode (the boundary is best-effort, exactly like `list_apps`).
pub async fn list_automations(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Query(_query): Query<ListAutomationsQuery>,
) -> Result<extract::Json<Vec<AutomationFileInfo>>, StatusCode> {
    let automations = workspace_manager
        .config_manager
        .list_automations()
        .await
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_manager.workspace_id,
                error = %e,
                "list_automations failed"
            );
            if e.retryable() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    Ok(extract::Json(
        automations
            .into_iter()
            .map(|a| to_file(a.file_path))
            .collect(),
    ))
}

/// One automation's raw YAML — matches the `agentic-http` `AutomationFileContent`
/// the FE's `getFile` consumes.
#[derive(Debug, Serialize)]
pub struct AutomationContent {
    pub path: String,
    pub content: String,
}

/// `GET /{workspace_id}/procedures/{path_b64}` — FleetOk. Serves one automation's
/// YAML from the compile boundary so clicking an automation renders its diagram on
/// a stateless serve replica; falls through to the FS in local / not-promoted
/// mode. (`/agentic-workflows/files/{path_b64}` is boundary-CAPABLE but classed
/// IdeOnly, so a serve node proxies it to the dead ide and 502s.)
pub async fn get_automation(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    extract::Path((_workspace_id, path_b64)): extract::Path<(Uuid, String)>,
    Query(_query): Query<ListAutomationsQuery>,
) -> Result<extract::Json<AutomationContent>, StatusCode> {
    let file_path = decode_path_b64(&path_b64).ok_or(StatusCode::BAD_REQUEST)?;

    // One call, because compiled-vs-disk is the manager's decision. This
    // handler used to spell the choice itself — a three-arm match plus a
    // second extractor to hold the disk it might need — which is the shape
    // that has to be re-derived correctly in every handler that reads an
    // artifact, and was not.
    let definition = workspace_manager
        .config_manager
        .automation_definition(&file_path)
        .await
        .map_err(|e| {
            tracing::warn!(file_path, error = %e, "get_automation: no source");
            // Retryable means "not compiled yet, ask again", not "your
            // workspace is wrong". A 404 here blames the customer for a
            // platform state.
            if e.retryable() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // RENDER-ONLY: the FE parses this for the automation diagram. Both arms
    // now return the parsed YAML re-serialised — semantically equal to the
    // source but not byte-identical (comments dropped, key order normalised).
    // Previously the disk arm returned the file verbatim, so the same URL
    // answered differently depending on which node served it. The verbatim
    // file is the IdeOnly file/edit path's job, never this one's.
    let content = serde_yaml::to_string(&definition).map_err(|e| {
        tracing::error!(file_path, "serialise automation: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(extract::Json(AutomationContent {
        path: file_path,
        content,
    }))
}

/// Decode a `path_b64` route param tolerantly — accept both the
/// `URL_SAFE_NO_PAD` form our listers emit and the standard-padded form the FE
/// `btoa` path historically produced. Used by the automation single-file fetch.
/// Returns the decoded workspace-relative path.
fn decode_path_b64(path_b64: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(path_b64).ok().or_else(|| {
        let pad = (4 - path_b64.len() % 4) % 4;
        STANDARD
            .decode(format!("{path_b64}{}", "=".repeat(pad)))
            .ok()
    })?;
    String::from_utf8(bytes).ok()
}
