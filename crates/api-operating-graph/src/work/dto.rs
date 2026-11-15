use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct WorkItemDto {
    pub id: Uuid,
    pub title: String,
    pub body: Option<String>,
    pub org_id: Uuid,
    pub location_id: Option<Uuid>,
    pub location_name: Option<String>,
    pub assignee_user_id: Option<Uuid>,
    pub assignee_name: Option<String>,
    pub assignee_role_id: Option<Uuid>,
    pub assignee_role_name: Option<String>,
    pub supervisor_id: Option<Uuid>,
    pub due_at: Option<String>,
    pub status: String,
    pub priority: i16,
    /// Why this item exists. Surfaced rather than kept internal because "a
    /// failed sanitiser check opened this" is the single most useful thing a
    /// person can be told about a task they did not create.
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub overdue: bool,
    pub created_at: String,
    pub completed_at: Option<String>,
}

/// Which of the two views to return.
///
/// An enum rather than two endpoints: they differ by one WHERE clause and share
/// every join, and splitting them is how the two drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    AssignedToMe,
    SupervisedByMe,
}

impl Default for Scope {
    fn default() -> Self {
        Self::AssignedToMe
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub scope: Scope,
    pub location_id: Option<Uuid>,
    /// Include finished work. Off by default: both screens ask for what is
    /// outstanding, and a year-old store's closed tail dwarfs its open set.
    #[serde(default)]
    pub include_done: bool,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkItem {
    pub org_id: Uuid,
    pub title: String,
    pub body: Option<String>,
    pub location_id: Option<Uuid>,
    pub assignee_user_id: Option<Uuid>,
    pub assignee_role_id: Option<Uuid>,
    pub supervisor_id: Option<Uuid>,
    /// RFC 3339.
    pub due_at: Option<String>,
    #[serde(default)]
    pub priority: i16,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkItem {
    pub status: Option<String>,
    pub assignee_user_id: Option<Uuid>,
    pub due_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateLocation {
    pub name: String,
    pub status: Option<String>,
    pub timezone: Option<String>,
    pub external_id: Option<String>,
    /// The place this one sits inside; must be this org's and not a descendant.
    pub parent_id: Option<Uuid>,
    /// The tenant's word for this level: `region`, `store`. Lowercased.
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRole {
    pub name: String,
    /// `location` or `franchisor`.
    pub scope: String,
}

pub const MAX_PAGE: u64 = 200;
pub const DEFAULT_PAGE: u64 = 100;

/// A page is always bounded. A store that has been open two years has an
/// unbounded closed tail, and an uncapped list is how one request becomes a
/// scan that takes the replica down.
pub fn clamp_limit(requested: Option<u64>) -> u64 {
    requested.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

/// The statuses a caller may set.
///
/// Validated here rather than left to the database's CHECK constraint: the
/// constraint returns a 500-shaped error, and "in_progres" deserves a 400 that
/// names the problem.
pub fn is_settable_status(s: &str) -> bool {
    matches!(s, "open" | "in_progress" | "done" | "cancelled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_always_bounded() {
        assert_eq!(clamp_limit(None), DEFAULT_PAGE);
        assert_eq!(clamp_limit(Some(10)), 10);
        assert_eq!(clamp_limit(Some(1_000_000)), MAX_PAGE);
        // Zero would loop a paging client forever.
        assert_eq!(clamp_limit(Some(0)), 1);
    }

    #[test]
    fn only_the_four_real_statuses_are_settable() {
        for s in ["open", "in_progress", "done", "cancelled"] {
            assert!(is_settable_status(s));
        }
        // The near-misses that would otherwise reach the CHECK constraint and
        // come back as a 500.
        for s in ["", "Done", "in progress", "in_progres", "deleted"] {
            assert!(!is_settable_status(s), "{s} must not be settable");
        }
    }

    #[test]
    fn a_create_request_names_its_org_in_the_body() {
        // Recorded because it is the reason `create` carries an explicit org
        // gate. `/work` is deliberately NOT nested under `/orgs/{org_id}` —
        // `org_middleware` rejects a frontline worker, who holds no membership
        // row by design — so the gate middleware would have provided has to be
        // made in the handler. A future refactor that drops
        // `has_standing_in_org` re-opens a cross-tenant write.
        let json = r#"{"org_id":"00000000-0000-0000-0000-000000000001","title":"x"}"#;
        let parsed: CreateWorkItem = serde_json::from_str(json).expect("parses");
        assert_eq!(parsed.title, "x");
        assert!(parsed.assignee_user_id.is_none() && parsed.assignee_role_id.is_none());
    }

    #[test]
    fn the_default_view_is_my_own_work() {
        // A caller who names no scope is asking "what do I have to do", not
        // "what am I responsible for other people doing".
        assert_eq!(Scope::default(), Scope::AssignedToMe);
    }
}
