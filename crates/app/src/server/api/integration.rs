use crate::server::api::middlewares::workspace_context::WorkspaceManagerWorkingCopy;
use axum::{http::StatusCode, response::Json};
use oxy::config::model::IntegrationType;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::Serialize;

#[derive(Serialize)]
pub struct LookerExploreInfo {
    pub model: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Serialize)]
pub struct LookerIntegrationInfo {
    pub name: String,
    pub explores: Vec<LookerExploreInfo>,
}

/// The Looker integrations declared in `config.yml`, with the explores each
/// one names. Read from the config, not from Looker: the field lists that used
/// to come back here were read from a cache `oxy looker sync` wrote to the
/// Factory's disk, and that cache is gone.
pub async fn list_looker_integrations(
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
) -> Result<Json<Vec<LookerIntegrationInfo>>, StatusCode> {
    let integrations = workspace_manager
        .config_manager
        .list_looker_integrations()
        .into_iter()
        .filter_map(|integration| {
            let IntegrationType::Looker(looker) = &integration.integration_type else {
                return None;
            };
            Some(LookerIntegrationInfo {
                name: integration.name.clone(),
                explores: looker
                    .explores
                    .iter()
                    .map(|explore| LookerExploreInfo {
                        model: explore.model.clone(),
                        name: explore.name.clone(),
                        description: explore.description.clone(),
                    })
                    .collect(),
            })
        })
        .collect();

    Ok(Json(integrations))
}
