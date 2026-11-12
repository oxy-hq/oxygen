use crate::services::installations::InstallationsService;
use axum::Json;
use axum::http::StatusCode;
use oxy_app::surface::OrgContextExtractor;
use serde::Serialize;

#[derive(Serialize)]
pub struct InstallationStatus {
    pub connected: bool,
    pub team_id: Option<String>,
    pub team_name: Option<String>,
    pub installed_at: Option<String>,
    pub installed_by: Option<uuid::Uuid>,
    pub bot_user_id: Option<String>,
}

pub async fn get_status(
    OrgContextExtractor(ctx): OrgContextExtractor,
) -> Result<Json<InstallationStatus>, (StatusCode, String)> {
    let inst = InstallationsService::find_active_by_org(ctx.org.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(match inst {
        Some(i) => InstallationStatus {
            connected: true,
            team_id: Some(i.slack_team_id),
            team_name: Some(i.slack_team_name),
            installed_at: Some(i.installed_at.to_rfc3339()),
            installed_by: Some(i.installed_by_user_id),
            bot_user_id: Some(i.bot_user_id),
        },
        None => InstallationStatus {
            connected: false,
            team_id: None,
            team_name: None,
            installed_at: None,
            installed_by: None,
            bot_user_id: None,
        },
    }))
}
