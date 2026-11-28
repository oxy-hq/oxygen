//! A tenant role under a token's ceiling: the two caps the role resolutions
//! apply after reading a membership.
//!
//! Moved here from `oxy_server_authz::caller` (C3a, 2026-11-28), which
//! re-exports both at their old paths, so that publish authorization
//! (`oxy-custom-apps`) and the workspace middleware (`oxy-workspace-runtime`)
//! can cap a role without the enforcement crate. The bodies are unchanged.
//!
//! **Not a decision.** A cap transforms a role a membership row already gave
//! into the role the credential carries; nothing here says who may do what.
//! `crates/guards/tests/integration/authz_boundaries.rs` lists this file as a
//! deliberate non-merge for that reason: `cap_org_role`'s Admin-ceiling arm
//! names the set of officer roles that an Admin ceiling caps to Admin, which is
//! the shape that test bans as the hand-rolled OrgAdmin ring.

use entity::org_members::OrgRole;
use entity::workspace_members::WorkspaceRole;
use oxy_authz::RoleCeiling;

/// `min(role, ceiling)` for a workspace role.
pub fn cap_workspace_role(role: WorkspaceRole, ceiling: RoleCeiling) -> WorkspaceRole {
    let cap = match ceiling {
        RoleCeiling::Viewer => WorkspaceRole::Viewer,
        RoleCeiling::Member => WorkspaceRole::Member,
        RoleCeiling::Admin => WorkspaceRole::Admin,
        RoleCeiling::Owner => WorkspaceRole::Owner,
    };
    std::cmp::min(role, cap)
}

/// An org role under a ceiling: Owner and Admin are capped at the ceiling, and
/// anything below admin is Member — an org has no lower role (design §3.2).
pub fn cap_org_role(role: OrgRole, ceiling: RoleCeiling) -> OrgRole {
    match (role, ceiling) {
        (role, RoleCeiling::Owner) => role,
        (OrgRole::Owner | OrgRole::Admin, RoleCeiling::Admin) => OrgRole::Admin,
        _ => OrgRole::Member,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_role_is_capped_at_the_ceiling() {
        use WorkspaceRole::*;
        assert_eq!(cap_workspace_role(Owner, RoleCeiling::Owner), Owner);
        assert_eq!(cap_workspace_role(Owner, RoleCeiling::Admin), Admin);
        assert_eq!(cap_workspace_role(Admin, RoleCeiling::Member), Member);
        assert_eq!(cap_workspace_role(Member, RoleCeiling::Viewer), Viewer);
        // A ceiling never raises a role.
        assert_eq!(cap_workspace_role(Member, RoleCeiling::Owner), Member);
        assert_eq!(cap_workspace_role(Viewer, RoleCeiling::Admin), Viewer);
    }

    #[test]
    fn an_org_role_is_admin_or_owner_only_under_that_ceiling() {
        use OrgRole::*;
        assert_eq!(cap_org_role(Owner, RoleCeiling::Owner), Owner);
        assert_eq!(cap_org_role(Owner, RoleCeiling::Admin), Admin);
        assert_eq!(cap_org_role(Admin, RoleCeiling::Admin), Admin);
        assert_eq!(cap_org_role(Owner, RoleCeiling::Member), Member);
        assert_eq!(cap_org_role(Admin, RoleCeiling::Viewer), Member);
        assert_eq!(cap_org_role(Member, RoleCeiling::Owner), Member);
        assert_eq!(cap_org_role(Member, RoleCeiling::Admin), Member);
    }
}
