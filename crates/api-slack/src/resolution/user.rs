use crate::linking::auto_match::{AutoMatchResult, try_auto_match};
use crate::services::user_links::UserLinksService;
use entity::slack_installations::Model as InstallationRow;
use entity::slack_user_links::Model as UserLinkRow;
use oxy_shared::errors::OxyError;

pub enum ResolvedUser {
    Linked(UserLinkRow),
    Unlinked,
}

/// Resolve the Oxy user for an incoming Slack event. Uses the link cache, then
/// tries auto-match by email, otherwise returns Unlinked (caller should prompt).
pub async fn resolve(
    installation: &InstallationRow,
    slack_user_id: &str,
) -> Result<ResolvedUser, OxyError> {
    if let Some(link) = UserLinksService::find(installation.id, slack_user_id).await? {
        UserLinksService::touch_last_seen(link.id).await?;
        return Ok(ResolvedUser::Linked(link));
    }
    match try_auto_match(installation, slack_user_id).await? {
        AutoMatchResult::Linked(link) => Ok(ResolvedUser::Linked(link)),
        AutoMatchResult::NoEmail | AutoMatchResult::EmailNotInOrg => Ok(ResolvedUser::Unlinked),
    }
}
