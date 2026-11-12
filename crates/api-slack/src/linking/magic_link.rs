use crate::config::SlackConfig;
use crate::oauth::state::{CreateUserLinkState, OauthStateService};
use oxy_shared::errors::OxyError;

/// Create a user_link state row and return a URL the user clicks to confirm.
///
/// `channel_id` and `thread_ts` are stored in the state row so that the
/// confirm handler can post a "✅ You're connected!" ephemeral back to the
/// channel where the user originally asked, closing the auth loop visibly.
pub async fn new_link_url(
    slack_team_id: &str,
    slack_user_id: &str,
    channel_id: Option<&str>,
    thread_ts: Option<&str>,
) -> Result<String, OxyError> {
    let nonce = OauthStateService::create_user_link(CreateUserLinkState {
        slack_team_id: slack_team_id.to_string(),
        slack_user_id: slack_user_id.to_string(),
        slack_channel_id: channel_id.map(str::to_string),
        slack_thread_ts: thread_ts.map(str::to_string),
    })
    .await?;
    let base = SlackConfig::cached()
        .as_runtime()
        .map(|c| c.app_base_url.clone())
        .unwrap_or_default();
    Ok(format!(
        "{base}/api/slack/link?token={}",
        urlencoding::encode(&nonce)
    ))
}
