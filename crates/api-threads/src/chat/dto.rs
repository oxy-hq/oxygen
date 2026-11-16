use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct ChannelSummary {
    pub id: Uuid,
    pub kind: String,
    /// Present for a named channel; a DM's title is derived client-side from
    /// its members, so the server does not invent one.
    pub name: Option<String>,
    pub topic: Option<String>,
    pub archived: bool,
    pub members: u64,
    /// Messages since this member's `last_read_at`. Derived per request rather
    /// than stored — a counter column has to be maintained by every writer and
    /// drifts the first time one fails.
    pub unread: u64,
    pub last_message_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MessageDto {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub author_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub body: String,
    pub created_at: String,
    pub edited: bool,
    pub deleted: bool,
}

#[derive(Debug, Deserialize)]
pub struct ListMessagesQuery {
    /// Keyset cursor — the `created_at` of the oldest message already held,
    /// as RFC 3339. Keyset rather than OFFSET because a conversation grows at
    /// the end a reader is paging away from, and OFFSET silently repeats rows
    /// when it does.
    pub before: Option<String>,
    /// The `id` of that same message. Required for the cursor to be total: the
    /// query orders by `(created_at DESC, id DESC)`, so a timestamp alone cannot
    /// separate two messages posted in the same instant — and a page boundary
    /// falling between them drops one forever. Optional only so an older client
    /// keeps working; without it the cursor is the timestamp alone, as before.
    pub before_id: Option<Uuid>,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PostMessageRequest {
    pub body: String,
}

/// The cap on one page of messages.
///
/// A client asking for more gets this. The limit is not politeness: a channel
/// is unbounded, and an uncapped page is how one request becomes a table scan
/// that takes the replica down — the same failure the observability backend
/// already had once.
pub const MAX_PAGE: u64 = 100;
pub const DEFAULT_PAGE: u64 = 50;

pub fn clamp_limit(requested: Option<u64>) -> u64 {
    requested.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_always_bounded() {
        assert_eq!(clamp_limit(None), DEFAULT_PAGE);
        assert_eq!(clamp_limit(Some(10)), 10);
        // The case that matters: a caller asking for everything gets a page.
        assert_eq!(clamp_limit(Some(100_000)), MAX_PAGE);
        // And zero is not a page — it would loop a paging client forever.
        assert_eq!(clamp_limit(Some(0)), 1);
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    /// Which org the channel belongs to. In the body rather than the path
    /// because `/chat` is mounted outside `/orgs/{org_id}` — nesting it would
    /// put `org_middleware` in front, which rejects the frontline workers these
    /// channels exist for. The handler makes the standing check the middleware
    /// would have made.
    pub org_id: Uuid,
    /// Required, and trimmed. The schema's `chat_channels_name_matches_kind`
    /// enforces that a `channel` has one; this endpoint only creates named
    /// channels, so a DM's nameless shape is not reachable here.
    pub name: String,
    pub topic: Option<String>,
    /// Extra members to add alongside the creator, who is always added.
    #[serde(default)]
    pub members: Vec<Uuid>,
}
