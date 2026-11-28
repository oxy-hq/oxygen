//! Assume-role's authority, as the port answers it. The decision
//! (`may_act_as`) and session liveness are `oxy-server-authz`'s; this is the
//! value they hand back. Moved here from `oxy_app_core::assume`, which
//! re-exports it.

/// Which authority a live session rests on — it decides the role the request runs
/// with, so it must not be guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActingAs {
    Staff,
    Partner,
}

impl ActingAs {
    /// Staff get Owner (what the old silent override gave them). A partner gets
    /// **Admin**: enough to work inside the client's product, but the org's own
    /// Owner remains untouchable — a partner administers a client, it never owns
    /// one. Both are marked `is_global_override`, so `OrgAdminStrict` still shuts
    /// billing and admin-promotion to them.
    pub fn org_role(self) -> entity::org_members::OrgRole {
        match self {
            ActingAs::Staff => entity::org_members::OrgRole::Owner,
            ActingAs::Partner => entity::org_members::OrgRole::Admin,
        }
    }
}
