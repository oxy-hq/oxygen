//! Handler for Slack `app_mention` events.
//!
//! Called when a user @-mentions the bot in a public/private channel.
//! Resolves the Oxy user (returning `SlackError::NotAuthenticated` if unlinked),
//! then delegates to `run_or_prompt`. All Slack error reporting is owned by the
//! `dispatch_user_event` wrapper in `webhooks/events.rs` — this handler never
//! calls any Slack client method directly.

use crate::error::SlackError;
use crate::linking::magic_link::new_link_url;
use crate::resolution::entrypoint::run_or_prompt;
use crate::resolution::user::{ResolvedUser, resolve};
use entity::slack_installations::Model as InstallationRow;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;

pub async fn handle(
    installation: InstallationRow,
    bot_token: String,
    user: String,
    text: String,
    channel: String,
    thread_ts: Option<String>,
    event_ts: String,
    // The node's Layer-1 pre-aggregation cache. A default carries no cache, so
    // the agent's semantic queries compile to warehouse SQL — correct, just
    // without the rollup shortcut.
    preagg: PreaggCacheCtx,
) -> Result<(), SlackError> {
    let effective_ts = thread_ts.unwrap_or_else(|| event_ts.clone());
    let cleaned = strip_bot_mention(&text);

    let link = match resolve(&installation, &user).await? {
        ResolvedUser::Linked(l) => l,
        ResolvedUser::Unlinked => {
            let connect_url = new_link_url(
                &installation.slack_team_id,
                &user,
                Some(&channel),
                Some(&effective_ts),
            )
            .await?;
            return Err(SlackError::NotAuthenticated { connect_url });
        }
    };

    run_or_prompt(
        installation,
        bot_token,
        link,
        cleaned,
        channel,
        effective_ts,
        false,
        preagg,
    )
    .await
}

/// Strip the leading `<@BOT_ID>` mention from the message text.
pub(crate) fn strip_bot_mention(text: &str) -> String {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<@")
        && let Some(end) = rest.find('>')
    {
        return rest[end + 1..].trim().to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_bot_mention;

    #[test]
    fn strips_leading_bot_mention() {
        assert_eq!(strip_bot_mention("<@U12345> what is MRR?"), "what is MRR?");
    }

    #[test]
    fn handles_leading_whitespace() {
        assert_eq!(strip_bot_mention("   <@U12345> hi"), "hi");
    }

    #[test]
    fn strips_only_leading_mention() {
        // Only the first mention is stripped; subsequent ones are preserved.
        assert_eq!(strip_bot_mention("<@U12345> hi <@U99999>"), "hi <@U99999>");
    }

    #[test]
    fn passes_through_without_mention() {
        assert_eq!(strip_bot_mention("hello"), "hello");
    }

    #[test]
    fn handles_malformed_mention() {
        // No closing `>` — falls through without panicking.
        let input = "<@ incomplete";
        let result = strip_bot_mention(input);
        assert_eq!(result, input.trim());
    }

    #[test]
    fn handles_empty_string() {
        assert_eq!(strip_bot_mention(""), "");
    }
}
