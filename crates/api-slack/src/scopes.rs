//! Bot scopes requested by the Oxy Slack app. Kept in sync with
//! internal-docs/slack/manifest.{test,prod}.yml.

pub const BOT_SCOPES: &[&str] = &[
    "app_mentions:read",
    "assistant:write",
    "chat:write",
    "chat:write.public",
    "channels:history",
    "files:write",
    "groups:history",
    "im:history",
    "im:write",
    "mpim:history",
    "reactions:write",
    "users:read",
    "users:read.email",
];

pub fn scopes_csv() -> String {
    BOT_SCOPES.join(",")
}
