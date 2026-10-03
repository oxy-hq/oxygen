//! Agent listing + builder-availability endpoints.
//!
//! The classic `.agent.yml` execution surface (preview/sync/test) has been
//! removed alongside the `oxy-agent` crate. Only listing — used by the chat
//! panel's agent selector — and the built-in builder readiness check remain
//! here, both serving the agentic-only world.

use crate::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use axum::{extract, http::StatusCode};
use oxy::config::model::BuilderAgentConfig;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct BuilderAvailabilityResponse {
    pub available: bool,
    /// Always `None` now — the legacy path-based builder agent has been
    /// removed. Kept in the response shape so existing clients don't
    /// require a coordinated frontend redeploy.
    pub builder_path: Option<String>,
    /// `true` when the builder is the built-in copilot.
    pub builtin: bool,
    /// Model name for the built-in copilot; `None` when no builder is configured.
    pub model: Option<String>,
}

pub async fn check_builder_availability(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<extract::Json<BuilderAvailabilityResponse>, StatusCode> {
    Ok(extract::Json(builder_availability(
        workspace_manager.config_manager.get_builder_config(),
    )))
}

/// A `builder_agent` whose `model` is blank is a builder that cannot run, and
/// the config accepts one (`builder_agent` is not validated). Reporting it as
/// available let the UI open a builder thread that then never started, so a
/// blank model reads the same as no builder at all.
fn builder_availability(config: Option<&BuilderAgentConfig>) -> BuilderAvailabilityResponse {
    match config {
        Some(BuilderAgentConfig::Builtin { model }) if !model.trim().is_empty() => {
            BuilderAvailabilityResponse {
                available: true,
                builder_path: None,
                builtin: true,
                model: Some(model.clone()),
            }
        }
        _ => BuilderAvailabilityResponse {
            available: false,
            builder_path: None,
            builtin: false,
            model: None,
        },
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(model: &str) -> BuilderAgentConfig {
        BuilderAgentConfig::Builtin {
            model: model.to_string(),
        }
    }

    #[test]
    fn a_builder_with_a_model_is_available() {
        let response = builder_availability(Some(&builtin("claude-sonnet-4-6")));
        assert!(response.available && response.builtin);
        assert_eq!(response.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn a_blank_model_is_no_builder_at_all() {
        for model in ["", "   "] {
            let response = builder_availability(Some(&builtin(model)));
            assert!(!response.available && !response.builtin, "{model:?}");
            assert_eq!(response.model, None);
        }
    }

    #[test]
    fn no_builder_config_is_unavailable() {
        let response = builder_availability(None);
        assert!(!response.available);
        assert_eq!(response.model, None);
    }
}
