//! World-model "Apps" — Toast / OpenWeatherMap / BestTime configuration
//! surfaced through the Workspace Settings UI.
//!
//! Each app is stored in `config.yml` as an entry under `integrations:`
//! using the existing tagged-enum schema. Credentials are referenced via
//! `*_var: SECRET_NAME` and resolved through the workspace secrets store
//! at request time. This module only lists them: an integration is changed in
//! the workspace's repository (the write routes were removed with the IDE).
//! Secret values are managed through the separate `/workspaces/.../secrets`
//! endpoints.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use oxy::config::model::{Integration, IntegrationType};
use serde::Serialize;
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;

/// Subset of the integration entry returned to the settings UI. Never
/// includes resolved secret values — only the `*_var` reference so the
/// frontend can show what secret name is wired up.
#[derive(Debug, Serialize)]
pub struct AppSummary {
    pub kind: String,
    pub name: String,
    #[serde(flatten)]
    pub config: AppConfigSummary,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AppConfigSummary {
    Toast {
        webhook_secret_var: String,
        restaurant_guids: Vec<String>,
    },
    OpenWeatherMap {
        api_key_var: String,
    },
    BestTime {
        api_key_var: String,
    },
    Unifi {
        api_key_var: String,
    },
}

/// `GET /api/{workspace_id}/apps`
pub async fn list_apps(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<Vec<AppSummary>>, (StatusCode, String)> {
    let _ = workspace_id;
    let config = workspace_manager.config_manager.get_config();
    let summaries = config
        .integrations
        .iter()
        .filter_map(summarize_app)
        .collect();
    Ok(Json(summaries))
}

fn summarize_app(integration: &Integration) -> Option<AppSummary> {
    let (kind, config) = match &integration.integration_type {
        IntegrationType::Toast(t) => (
            "toast",
            AppConfigSummary::Toast {
                webhook_secret_var: t.webhook_secret_var.clone(),
                restaurant_guids: t.restaurant_guids.clone(),
            },
        ),
        IntegrationType::OpenWeatherMap(o) => (
            "openweathermap",
            AppConfigSummary::OpenWeatherMap {
                api_key_var: o.api_key_var.clone(),
            },
        ),
        IntegrationType::BestTime(b) => (
            "besttime",
            AppConfigSummary::BestTime {
                api_key_var: b.api_key_var.clone(),
            },
        ),
        IntegrationType::Unifi(u) => (
            "unifi",
            AppConfigSummary::Unifi {
                api_key_var: u.api_key_var.clone(),
            },
        ),
        // Omni and Looker are surfaced through other settings panels;
        // toast_analytics is an admin-only reconciliation source (config.yml).
        IntegrationType::Omni(_)
        | IntegrationType::Looker(_)
        | IntegrationType::ToastAnalytics(_) => return None,
    };
    Some(AppSummary {
        kind: kind.to_string(),
        name: integration.name.clone(),
        config,
    })
}
