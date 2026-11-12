use crate::services::installations::InstallationsService;
use crate::services::user_links::UserLinksService;
use crate::types::InteractivityPayload;
use oxy_shared::errors::OxyError;

pub async fn handle(payload: &InteractivityPayload) -> Result<(), OxyError> {
    let Some(inst) = InstallationsService::find_active_by_team(&payload.team.id).await? else {
        return Ok(());
    };
    let Some(link) = UserLinksService::find(inst.id, &payload.user.id).await? else {
        return Ok(());
    };
    UserLinksService::delete(link.id).await?;
    Ok(())
}
