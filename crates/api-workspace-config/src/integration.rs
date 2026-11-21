//! The integrations a workspace's `config.yml` declares, as the Workspace
//! Settings → Connections page lists them, and the credentials check for one:
//!
//! - `GET /{workspace_id}/integrations/looker` lists the Looker integrations.
//! - `POST /{workspace_id}/integrations/looker/{name}/test` checks one
//!   integration's credentials against Looker ([`looker_test`]). It is what
//!   `oxyc looker test` calls, and it replaced the Rust `oxy looker test`
//!   (2026-11-21).

use axum::{extract::Path, http::StatusCode, response::Json};
use oxy::config::model::IntegrationType;
use oxy_app::surface::WorkspaceManagerReadOnly;
use oxy_app::surface::role_guards::WorkspaceAdmin;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::Serialize;
use tracing::{error, instrument};
use uuid::Uuid;

pub mod looker_test;

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
/// one names. Read from the served revision's compiled config, not from Looker
/// or a working copy, so any replica answers. The field lists that used to come
/// back here were read from a cache `oxy looker sync` wrote to the Factory's
/// disk, and that cache is gone.
pub async fn list_looker_integrations(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
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

/// Check one Looker integration's credentials: resolve its client id and
/// secret from the workspace's secrets, sign in to Looker and list its models.
///
/// `WorkspaceAdmin` first, as the OAuth connects beside it take it, so a caller
/// without it is refused before a 404 could confirm an integration's name.
/// The integration comes from the served revision's compiled config and its
/// credentials from the secrets store, never a working copy, so any replica
/// answers (FleetOk). Statuses follow `…/oltp/test-connection`: a check that
/// ran is a `200` whose `ok` is the verdict (a missing secret and a refusal
/// from Looker included); `404` is no Looker integration by that name; only a
/// fault reading Oxy's own secrets store is a `500`.
#[instrument(skip(workspace_manager))]
pub async fn test_looker_integration(
    _: WorkspaceAdmin,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
) -> Result<Json<looker_test::LookerConnectionTest>, (StatusCode, String)> {
    let looker = workspace_manager
        .config_manager
        .list_looker_integrations()
        .into_iter()
        .find(|integration| integration.name == name)
        .and_then(|integration| match &integration.integration_type {
            IntegrationType::Looker(looker) => Some(looker.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("no Looker integration named '{name}' in the served config"),
            )
        })?;

    let secrets = &workspace_manager.secrets_manager;
    let resolve = |var: String| async move {
        secrets.resolve_secret(&var).await.map_err(|e| {
            error!("looker test: resolve secret {var}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not read the workspace's secrets".to_string(),
            )
        })
    };
    let client_id = resolve(looker.client_id_var.clone()).await?;
    let client_secret = resolve(looker.client_secret_var.clone()).await?;

    Ok(Json(
        looker_test::check(
            &name,
            &looker,
            looker_test::Credentials {
                client_id,
                client_secret,
            },
        )
        .await,
    ))
}
