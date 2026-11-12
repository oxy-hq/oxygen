//! Typed error surface for user-triggered Slack events.
//!
//! Every handler that processes a user message (`app_mention`, `message`,
//! `run_for_slack`) returns `Result<(), SlackError>`. The dispatcher in
//! `webhooks/events.rs` owns all error reporting — handlers never call any
//! Slack client method for errors themselves.
//!
//! ## Variant taxonomy
//!
//! - **User-facing** (`NotAuthenticated`, `NotOrgMember`, `NoWorkspaces`,
//!   `NoAgentsInWorkspace`) — rendered as ephemeral messages visible only to
//!   the person who triggered the event. Each variant produces its own Block
//!   Kit blocks with actionable copy.
//!
//! - **Infrastructure** (`Internal`) — any `OxyError` that is not
//!   user-actionable. Rendered as a non-ephemeral thread message (the whole
//!   channel sees it) so the team knows something went wrong. Includes the
//!   Oxy thread URL when the thread was already created before the error hit.

use oxy_shared::errors::OxyError;
use serde_json::{Value, json};

use crate::resolution::workspace_agent::NoWorkspacesReason;

/// All failure states a user-triggered Slack handler can produce.
#[derive(Debug)]
pub enum SlackError {
    // ── User-facing variants ──────────────────────────────────────────────
    /// User has not linked their Oxy account. The `connect_url` is a
    /// fresh magic-link URL minted for this specific interaction so it
    /// never expires mid-flow. A new URL is generated on every unlinked
    /// message — the user just keeps getting the prompt until they finish.
    NotAuthenticated { connect_url: String },

    /// User's Oxy account is no longer a member of the installation's org
    /// (they were removed after the initial link was created).
    NotOrgMember,

    /// The installation's org has no workspaces the user can access.
    NoWorkspaces(NoWorkspacesReason),

    /// The workspace that was resolved (or picked) has no agents configured.
    NoAgentsInWorkspace { workspace_name: String },

    // ── Infrastructure variant ────────────────────────────────────────────
    /// Any `OxyError` that is not user-actionable. The `thread_url` is
    /// `Some` when the Oxy thread was already created before the error
    /// occurred — the rendered block includes a "View thread in Oxy →"
    /// button so the user can navigate to whatever was partially computed.
    Internal {
        source: OxyError,
        thread_url: Option<String>,
    },
}

impl SlackError {
    /// `true` for user-facing errors that should be sent as ephemeral
    /// messages (only visible to the requester). `false` for infrastructure
    /// errors, which post in the thread so the whole channel can see that
    /// something went wrong.
    pub fn is_ephemeral(&self) -> bool {
        !matches!(self, SlackError::Internal { .. })
    }

    /// Short plain-text fallback used as the Slack notification preview and
    /// the `alt_text` / `text` field for clients that can't render blocks.
    pub fn fallback_text(&self) -> &'static str {
        match self {
            Self::NotAuthenticated { .. } => "Connect your Oxygen account to continue",
            Self::NotOrgMember => "You're no longer a member of this Oxygen org",
            Self::NoWorkspaces(_) => "No workspaces available",
            Self::NoAgentsInWorkspace { .. } => "No agents configured in this workspace",
            Self::Internal { .. } => "Something went wrong",
        }
    }

    /// Full Block Kit `blocks` array for this error state.
    pub fn to_blocks(&self) -> Value {
        match self {
            Self::NotAuthenticated { connect_url } => connect_prompt_blocks(connect_url),
            Self::NotOrgMember => not_org_member_blocks(),
            Self::NoWorkspaces(reason) => no_workspaces_blocks(reason),
            Self::NoAgentsInWorkspace { workspace_name } => no_agents_blocks(workspace_name),
            Self::Internal { thread_url, .. } => internal_error_blocks(thread_url.as_deref()),
        }
    }

    /// Attach a thread URL to an `Internal` error. No-op for user-facing
    /// errors (they don't render thread links). Convenience method for the
    /// `?` operator shorthand — callers can write:
    /// ```
    /// some_fallible_op().map_err(|e| SlackError::from(e).with_thread_url(&url))?;
    /// ```
    pub fn with_thread_url(mut self, url: &str) -> Self {
        if let Self::Internal {
            ref mut thread_url, ..
        } = self
        {
            *thread_url = Some(url.to_string());
        }
        self
    }
}

impl From<OxyError> for SlackError {
    fn from(e: OxyError) -> Self {
        Self::Internal {
            source: e,
            thread_url: None,
        }
    }
}

impl std::fmt::Display for SlackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAuthenticated { .. } => write!(f, "user not authenticated"),
            Self::NotOrgMember => write!(f, "user is not an org member"),
            Self::NoWorkspaces(r) => write!(f, "no workspaces: {}", r.user_message()),
            Self::NoAgentsInWorkspace { workspace_name } => {
                write!(f, "no agents in workspace '{workspace_name}'")
            }
            Self::Internal { source, .. } => write!(f, "internal error: {source}"),
        }
    }
}

impl std::error::Error for SlackError {}

// ── Block Kit renderers ───────────────────────────────────────────────────────

/// "🔗 Connect your Oxy account" ephemeral — shown every time an unlinked
/// user sends a message. A fresh magic-link URL is embedded so the button
/// never leads to an expired token.
fn connect_prompt_blocks(connect_url: &str) -> Value {
    json!([
        {
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": "*🔗 Connect your Oxygen account*\n\nTo query your data from Slack, connect your Oxygen account first. This takes less than a minute."
            }
        },
        {
            "type": "actions",
            "elements": [
                {
                    "type": "button",
                    "action_id": "slack_connect_oxy",
                    "text": { "type": "plain_text", "text": "Connect to Oxygen →", "emoji": false },
                    "url": connect_url,
                    "style": "primary"
                }
            ]
        },
        {
            "type": "context",
            "elements": [
                { "type": "mrkdwn", "text": "Need access? Contact your Oxygen admin." }
            ]
        }
    ])
}

fn not_org_member_blocks() -> Value {
    json!([{
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": "⚠️ You're no longer a member of this Oxygen org. Ask an org admin to re-invite you."
        }
    }])
}

fn no_workspaces_blocks(reason: &NoWorkspacesReason) -> Value {
    json!([{
        "type": "section",
        "text": { "type": "mrkdwn", "text": reason.user_message() }
    }])
}

fn no_agents_blocks(workspace_name: &str) -> Value {
    json!([{
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": format!(
                "⚠️ Workspace _{workspace_name}_ doesn't have any agents configured yet. \
                 Add one to this workspace's config and try again."
            )
        }
    }])
}

fn internal_error_blocks(thread_url: Option<&str>) -> Value {
    let mut blocks = vec![json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": "⚠️ *Something went wrong on our end.* Please try again in a moment."
        }
    })];
    if let Some(url) = thread_url {
        blocks.push(json!({
            "type": "actions",
            "elements": [{
                "type": "button",
                "text": { "type": "plain_text", "text": "View thread in Oxygen →", "emoji": false },
                "url": url
            }]
        }));
    }
    json!(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_authenticated_is_ephemeral() {
        let e = SlackError::NotAuthenticated {
            connect_url: "https://example.com".into(),
        };
        assert!(e.is_ephemeral());
    }

    #[test]
    fn internal_is_not_ephemeral() {
        let e = SlackError::from(OxyError::RuntimeError("boom".into()));
        assert!(!e.is_ephemeral());
    }

    #[test]
    fn with_thread_url_attaches_to_internal() {
        let e = SlackError::from(OxyError::RuntimeError("boom".into()))
            .with_thread_url("https://app.oxygen-hq.com/thread/123");
        if let SlackError::Internal { thread_url, .. } = e {
            assert_eq!(
                thread_url.as_deref(),
                Some("https://app.oxygen-hq.com/thread/123")
            );
        } else {
            panic!("expected Internal variant");
        }
    }

    #[test]
    fn with_thread_url_is_noop_for_user_errors() {
        let e = SlackError::NotOrgMember.with_thread_url("https://example.com");
        assert!(matches!(e, SlackError::NotOrgMember));
    }

    #[test]
    fn internal_error_blocks_include_view_thread_button_when_url_provided() {
        let e = SlackError::Internal {
            source: OxyError::RuntimeError("x".into()),
            thread_url: Some("https://t.co/abc".into()),
        };
        let blocks = e.to_blocks();
        let text = blocks.to_string();
        assert!(text.contains("View thread in Oxygen"));
        assert!(text.contains("https://t.co/abc"));
    }

    #[test]
    fn internal_error_blocks_omit_button_when_no_url() {
        let e = SlackError::Internal {
            source: OxyError::RuntimeError("x".into()),
            thread_url: None,
        };
        let blocks = e.to_blocks();
        assert!(!blocks.to_string().contains("View thread"));
    }

    #[test]
    fn all_variants_have_fallback_text() {
        let errors: Vec<SlackError> = vec![
            SlackError::NotAuthenticated {
                connect_url: "u".into(),
            },
            SlackError::NotOrgMember,
            SlackError::NoWorkspaces(NoWorkspacesReason::OrgHasNoWorkspaces {
                org_id: uuid::Uuid::nil(),
            }),
            SlackError::NoAgentsInWorkspace {
                workspace_name: "ws".into(),
            },
            SlackError::Internal {
                source: OxyError::RuntimeError("e".into()),
                thread_url: None,
            },
        ];
        for e in &errors {
            assert!(!e.fallback_text().is_empty(), "{e} has empty fallback_text");
        }
    }
}
