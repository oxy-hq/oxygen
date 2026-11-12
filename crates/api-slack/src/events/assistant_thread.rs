//! Handlers for Slack assistant thread lifecycle events.
//!
//! `AssistantThreadStarted` — sent when a user opens the Oxy bot in the
//!   Slack AI sidebar. We clear any loading state and post a greeting.
//!
//! `AssistantThreadContextChanged` — sent when context (e.g. the channel
//!   the sidebar is anchored to) changes. No action needed for now.

use crate::client::SlackClient;
use entity::slack_installations::Model as InstallationRow;
use oxy_shared::errors::OxyError;

/// Returns the welcome message text shown at the start of every assistant thread.
pub fn welcome_message_text() -> &'static str {
    "👋 Hi! I'm Oxygen — your data analytics assistant.\n\n\
     Ask me anything about your data:\n\
     • \"What was revenue last month?\"\n\
     • \"Build a chart of daily active users\"\n\n\
     Reply in this thread to follow up on any answer."
}

/// Starter prompts surfaced as clickable buttons in the assistant thread.
const SUGGESTED_PROMPTS: &[(&str, &str)] = &[
    (
        "What's in my data?",
        "Give me an overview of the tables and data available",
    ),
    (
        "Trending metrics",
        "What are the key metrics trending this week?",
    ),
    ("Recent activity", "Show me recent customer activity"),
    ("Ask a question", "How many users signed up last month?"),
];

pub async fn started(
    _installation: InstallationRow,
    bot_token: String,
    channel: Option<String>,
    thread_ts: Option<String>,
) -> Result<(), OxyError> {
    let (Some(channel), Some(thread_ts)) = (channel, thread_ts) else {
        return Ok(());
    };
    let token = bot_token;
    let client = SlackClient::new();

    // Clear any loading indicator the sidebar may show.
    let _ = client
        .assistant_threads_set_status(&token, &channel, &thread_ts, "", None)
        .await;

    // Post a welcome message explaining the usage pattern.
    let post_result = client
        .chat_post_message(&token, &channel, welcome_message_text(), Some(&thread_ts))
        .await;

    // Only set suggested prompts if the message posted successfully —
    // Slack may reject the call if the thread isn't fully initialized yet.
    if post_result.is_ok() {
        let _ = client
            .assistant_threads_set_suggested_prompts(
                &token,
                &channel,
                &thread_ts,
                SUGGESTED_PROMPTS,
            )
            .await;
    }

    Ok(())
}

/// No action required when the assistant thread context changes.
pub async fn context_changed(_installation: InstallationRow) -> Result<(), OxyError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_message_contains_oxy_and_revenue() {
        let msg = welcome_message_text();
        assert!(
            msg.contains("Oxygen"),
            "welcome message must mention Oxygen"
        );
        assert!(
            msg.contains("revenue"),
            "welcome message must include revenue example"
        );
    }

    #[test]
    fn welcome_message_mentions_follow_up() {
        let msg = welcome_message_text();
        assert!(
            msg.contains("follow up"),
            "welcome message must teach thread iteration"
        );
    }
}
