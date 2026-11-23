//! Agent listing endpoint.
//!
//! The classic `.agent.yml` execution surface (preview/sync/test) has been
//! removed alongside the `oxy-agent` crate. Only listing — used by the chat
//! panel's agent selector — remains here, serving the agentic-only world.

use axum::{extract, http::StatusCode};
use oxy_workspace_runtime::request::workspace_context::WorkspaceManagerReadOnly;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentConfigResponse {
    pub name: String,
    pub public: bool,
    pub path: String,
    /// Model ref this agent resolves through. Lets the home page tie a
    /// readiness gap to the *agent the chat will actually use* rather than
    /// to "any LLM key is missing".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// IANA timezone (e.g. `America/Los_Angeles`) from the agent's
    /// `.agentic.yml`. Lets the workspace top-bar clock render local time
    /// instead of UTC; the frontend defaults to `America/Los_Angeles` when
    /// this is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl AgentConfigResponse {
    pub fn new(name: String, path: String, public: bool) -> Self {
        Self {
            name,
            path,
            public,
            model: None,
            timezone: None,
        }
    }
}

#[derive(serde::Deserialize, Default)]
pub struct GetAgentsQuery {
    /// Active branch in the IDE. Consumed by `workspace_middleware`, which
    /// applies the branch gate once and records the outcome as the manager's
    /// `Origin`. Declared here so it stays part of the documented contract.
    #[serde(default)]
    pub branch: Option<String>,
}

/// List analytics agents (`.agentic.yml`) in a workspace.
#[utoipa::path(
    method(get),
    path = "/{workspace_id}/agents",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID")
    ),
    responses(
        (status = OK, description = "Success", body = Vec<String>, content_type = "application/json")
    ),
    security(
        ("ApiKey" = [])
    )
)]
pub async fn get_agents(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    extract::Query(_query): extract::Query<GetAgentsQuery>,
) -> Result<extract::Json<Vec<AgentConfigResponse>>, StatusCode> {
    // Hybrid path: when the workspace has been promoted on its default
    // branch, serve the listing from `agent_definitions`. `llm.ref` is
    // pulled from the JSONB to keep the same response shape without
    // re-parsing every `.agentic.yml`.
    //
    // The revision comes off the manager rather than being re-derived from
    // `query.branch`: the middleware already applied the branch gate and the
    // last-known-good walk to produce it, so this reads the revision the rest of
    // the request is pinned to. `None` means the middleware chose the working
    // copy, which is the same fall-through as an empty listing.
    let agents = workspace_manager
        .config_manager
        .list_analytics_agents()
        .await
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_manager.workspace_id,
                error = %e,
                "get_agents failed"
            );
            if e.retryable() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    Ok(extract::Json(
        agents
            .into_iter()
            .map(|entry| {
                let mut resp = AgentConfigResponse::new(entry.name, entry.file_path, true);
                resp.model = entry.model_ref;
                resp.timezone = entry.timezone;
                resp
            })
            .collect(),
    ))
}
