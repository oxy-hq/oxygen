//! Wire types for the org-team and app-access surfaces.
//!
//! The one shape worth explaining is [`GrantDto`]: a grant is rendered as a tagged
//! union over its GRANTEE (`kind: "user" | "team"`) rather than as two parallel
//! lists. The two grant kinds are one concept — the UI shows one access list, and a
//! future third kind lands as a variant instead of a third endpoint.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// ── Teams ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct TeamDto {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    /// How many people are in the team. Batched for the list view — never an N+1.
    pub member_count: u64,
    pub created_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TeamMemberDto {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    /// The member's ORG role, shown so an admin building a team can see who they're
    /// adding. Nothing about team membership changes it.
    pub org_role: String,
    pub added_at: String,
}

/// A person the caller could grant an app to. Field-for-field the partner
/// console's `OrgMemberDto`, so one frontend picker consumes either.
#[derive(Debug, Serialize, ToSchema)]
pub struct OrgMemberOptionDto {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TeamDetailDto {
    #[serde(flatten)]
    pub team: TeamDto,
    pub members: Vec<TeamMemberDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTeamRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A full replace of the editable fields, not a sparse patch — the edit dialog
/// always has both in hand, and a replace has no "was it omitted or cleared?"
/// ambiguity to encode.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTeamRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AddTeamMemberRequest {
    pub user_id: Uuid,
}

// ── App access ──────────────────────────────────────────────────────────────

/// Who holds a grant on an app, and what it buys them.
///
/// `kind` discriminates the grantee. `name` is the team name or the user's display
/// name; `email` is populated only for the user variant, so the UI can render a
/// person and a team in one list without a second lookup.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrantDto {
    /// `"user"` or `"team"`.
    pub kind: &'static str,
    /// The user id or the team id, per `kind`.
    pub id: Uuid,
    pub name: String,
    pub email: Option<String>,
    /// `"admin"` or `"member"`.
    pub role: String,
    /// Team grants only — how many people the grant actually reaches. An admin
    /// about to grant `admin` to a 40-person team should see the 40.
    pub member_count: Option<u64>,
}

/// One row in the org's "who can open what" list. Carries just enough to render
/// the row and its badge — the full grant list is fetched when a row is opened.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppAccessSummaryDto {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub visibility: String,
    /// Grants on the app, both kinds. Zero on a restricted app is the state worth
    /// surfacing: nobody but org officers can open it.
    pub grant_count: u64,
    pub published: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppAccessDto {
    pub app_id: Uuid,
    /// `"org"` (any org member) or `"members"` (grants only).
    pub visibility: String,
    pub grants: Vec<GrantDto>,
}

/// The grantee half of a write — the request mirror of [`GrantDto`].
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GranteeRef {
    User { id: Uuid, role: String },
    Team { id: Uuid, role: String },
}

impl GranteeRef {
    pub fn role(&self) -> &str {
        match self {
            Self::User { role, .. } | Self::Team { role, .. } => role,
        }
    }
}

/// Replaces an app's whole access configuration in one call.
///
/// A full replace rather than incremental add/remove: the UI edits one list and
/// saves it, and a replace has no lost-update window between two admins editing the
/// same app — the second save wins wholesale instead of interleaving.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetAppAccessRequest {
    pub visibility: String,
    #[serde(default)]
    pub grants: Vec<GranteeRef>,
}
