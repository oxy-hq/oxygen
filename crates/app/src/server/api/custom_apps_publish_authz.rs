//! Publish authorization — the three gates (design
//! `internal-docs/partner-platform.md` §7).
//!
//! A publish of app `A` into org `X` is allowed only if all three hold, and they
//! are checked **at publish time** (not just at credential mint) so revoking
//! consent or detaching a partner mid-CI-run denies the in-flight publish:
//!
//!   1. Publisher / credential match — resolved before we get here (the caller has
//!      already established *which* app and *which* actor from the OIDC claims or
//!      the token).
//!   2. Actor may publish — staff, an org Admin+ of `X`, or a partner with
//!      `manage_apps` assigned to `X`.
//!   3. Client consent — `X` has partner-publish enabled. Skipped for an org
//!      member of `X` and for staff: consent governs the *third-party* edge only.
//!
//! The **decision** (`authorize_publish`) is a pure function of resolved facts, so
//! it is exhaustively unit-tested here. The DB-backed resolver that builds those
//! facts lives alongside it.

use entity::org_members::OrgRole;
use entity::prelude::{OrgMembers, PartnerPublishConsent};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use oxy_server_authz::partner_authz::{PartnerCapability, partner_for_org, resolve_scope};

/// Who is trying to publish, reduced to what the decision needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishActor {
    /// Oxy staff (Global Owner / Global Admin).
    Staff,
    /// A real member of the TARGET org, with this role.
    OrgMember(OrgRole),
    /// A partner acting on a client. `manages_target` is "this person is assigned
    /// this client"; `can_manage_apps` is "their role ∩ ceiling grants manage_apps".
    Partner {
        manages_target: bool,
        can_manage_apps: bool,
    },
    /// Authenticated, but neither staff, a member of the target org, nor a partner
    /// of it. The common "wrong tenant" case.
    Outsider,
}

/// Why a publish was refused. Distinct variants so the caller can map to the right
/// status + message rather than a blanket 403.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishDenied {
    /// Not staff, not an org officer, not a partner of this org.
    NotAuthorized,
    /// A partner, but not assigned this client.
    NotAssignedToClient,
    /// A partner assigned the client, but their role/ceiling lacks `manage_apps`.
    MissingManageApps,
    /// A partner with the capability, but the client has not consented.
    NoClientConsent,
    /// Staff, but the target workspace has locked Oxy staff out.
    StaffLockedOut,
}

/// The pure decision. `consent_enabled` is the client's opt-in state (§5); it is
/// only consulted for the partner path.
pub fn authorize_publish(actor: &PublishActor, consent_enabled: bool) -> Result<(), PublishDenied> {
    match actor {
        // Staff have always been able to publish; unchanged.
        PublishActor::Staff => Ok(()),

        // A tenant's own Owner/Admin publishing their own app needs no consent —
        // consent is about *third parties*, and you are not a third party to
        // yourself. A plain Member cannot publish.
        PublishActor::OrgMember(role) if role.level() >= OrgRole::Admin.level() => Ok(()),
        PublishActor::OrgMember(_) => Err(PublishDenied::NotAuthorized),

        // The third-party edge — all three sub-conditions, in the order that gives
        // the most informative error (assignment, then capability, then consent).
        PublishActor::Partner {
            manages_target,
            can_manage_apps,
        } => {
            if !*manages_target {
                return Err(PublishDenied::NotAssignedToClient);
            }
            if !*can_manage_apps {
                return Err(PublishDenied::MissingManageApps);
            }
            if !consent_enabled {
                return Err(PublishDenied::NoClientConsent);
            }
            Ok(())
        }

        PublishActor::Outsider => Err(PublishDenied::NotAuthorized),
    }
}

/// The **composed** publish decision, over already-resolved facts. This is the whole
/// authorization for the (not staff-gated) publish route, made pure so it can't
/// drift: staff clear the workspace lockdown, everyone else defers to
/// [`authorize_publish`] — which denies an outsider and a plain Member. Conflating
/// "staff" with "not a member" is exactly the tenant-isolation break this guards.
///
/// `staff_locked_out` is only meaningful for staff, `consent_enabled` only for a
/// partner; the caller reads each lazily and passes `false` otherwise.
pub fn publish_decision(
    actor: &PublishActor,
    staff_locked_out: bool,
    consent_enabled: bool,
) -> Result<(), PublishDenied> {
    match actor {
        PublishActor::Staff if staff_locked_out => Err(PublishDenied::StaffLockedOut),
        PublishActor::Staff => Ok(()),
        _ => authorize_publish(actor, consent_enabled),
    }
}

/// Read the client's consent state. Default OFF: no row, a `false` row, or any DB
/// error all deny — this gate must never fail *open*.
pub async fn consent_enabled(db: &DatabaseConnection, org_id: Uuid) -> bool {
    PartnerPublishConsent::find_by_id(org_id)
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|c| c.enabled)
        .unwrap_or(false)
}

/// Build the [`PublishActor`] for `user` against `target_org_id`, most-privileged
/// first: staff, then a real member of the org, then a partner of it, else an
/// outsider.
///
/// Staff is checked first because an Oxy staffer who is *also* incidentally a
/// member somewhere should still publish with staff authority, not be narrowed to
/// their membership.
///
/// Resolved for the [`Caller`](oxy_server_authz::Caller), so an API token
/// publishes only where it reaches: outside its grants it is an outsider, its
/// staff and partner standing are what it carries, and a member's role is
/// capped by the token's ceiling in the org.
pub async fn resolve_actor(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    target_org_id: Uuid,
) -> PublishActor {
    let user_id = caller.user_id;
    let Some(ceiling) = caller.org_reach_ceiling(target_org_id) else {
        return PublishActor::Outsider;
    };
    // Staff publish authority is `ManageApps` **over this org** — capability and scope.
    //
    // `is_staff()` alone would let a grant bounded to org A publish into org B, which
    // silently voids scope on the one verb an App Operator exists to perform. There is no
    // `enforce` conjunction here to catch it the way `custom_apps_gates` has: this
    // function resolves WHICH actor you are, and the answer decides the rest.
    if oxy_server_authz::globals::platform_reaches(
        db,
        caller,
        oxy_authz::Cap::ManageApps,
        target_org_id,
    )
    .await
    {
        return PublishActor::Staff;
    }

    // A real member of the target org.
    if let Ok(Some(m)) = OrgMembers::find()
        .filter(entity::org_members::Column::OrgId.eq(target_org_id))
        .filter(entity::org_members::Column::UserId.eq(user_id))
        .one(db)
        .await
    {
        return PublishActor::OrgMember(oxy_server_authz::cap_org_role(m.role, ceiling));
    }

    // A partner of the target org: does a partner manage it, and does THIS user
    // hold a role there that (a) grants manage_apps and (b) is assigned this client?
    if let Some(partner_org_id) = partner_for_org(db, target_org_id).await
        && let Some(scope) = resolve_scope(db, partner_org_id, caller).await
    {
        return PublishActor::Partner {
            manages_target: scope.org_ids.contains(&target_org_id),
            can_manage_apps: scope.allows(PartnerCapability::ManageApps),
        };
    }

    PublishActor::Outsider
}

/// Resolve + decide in one call — the entry point a handler uses. Consent is read
/// only for the partner path (it's the sole actor whose decision consults it), so
/// the org/staff paths pay no extra query.
pub async fn authorize_publish_for(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    target_org_id: Uuid,
) -> Result<(), PublishDenied> {
    let actor = resolve_actor(db, caller, target_org_id).await;
    let consent = match actor {
        PublishActor::Partner { .. } => consent_enabled(db, target_org_id).await,
        _ => false,
    };
    authorize_publish(&actor, consent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staff_may_publish_without_consent() {
        assert!(authorize_publish(&PublishActor::Staff, false).is_ok());
    }

    #[test]
    fn org_admin_publishes_own_app_without_consent() {
        // Consent is about third parties; the org's own officer is not one.
        assert!(authorize_publish(&PublishActor::OrgMember(OrgRole::Owner), false).is_ok());
        assert!(authorize_publish(&PublishActor::OrgMember(OrgRole::Admin), false).is_ok());
    }

    #[test]
    fn org_member_may_not_publish() {
        assert_eq!(
            authorize_publish(&PublishActor::OrgMember(OrgRole::Member), true),
            Err(PublishDenied::NotAuthorized)
        );
    }

    #[test]
    fn partner_needs_all_three_gates() {
        let ok = PublishActor::Partner {
            manages_target: true,
            can_manage_apps: true,
        };
        // All three: assigned, capable, consented.
        assert!(authorize_publish(&ok, true).is_ok());
        // Consent is the gate that distinguishes this from the org path: capable
        // and assigned, but the client hasn't opted in.
        assert_eq!(
            authorize_publish(&ok, false),
            Err(PublishDenied::NoClientConsent)
        );
    }

    #[test]
    fn partner_errors_are_ordered_most_informative() {
        // Not assigned beats missing-capability beats no-consent.
        assert_eq!(
            authorize_publish(
                &PublishActor::Partner {
                    manages_target: false,
                    can_manage_apps: false,
                },
                false
            ),
            Err(PublishDenied::NotAssignedToClient)
        );
        assert_eq!(
            authorize_publish(
                &PublishActor::Partner {
                    manages_target: true,
                    can_manage_apps: false,
                },
                false
            ),
            Err(PublishDenied::MissingManageApps)
        );
    }

    #[test]
    fn consent_never_rescues_a_non_partner_non_officer() {
        // A stray consent=true must not let an outsider or a mere member through.
        assert_eq!(
            authorize_publish(&PublishActor::Outsider, true),
            Err(PublishDenied::NotAuthorized)
        );
        assert_eq!(
            authorize_publish(&PublishActor::OrgMember(OrgRole::Member), true),
            Err(PublishDenied::NotAuthorized)
        );
    }

    // ── the composed route decision (`publish_decision`) ─────────────────────
    // These guard the tenant-isolation break the wrapper had: it treated "not a
    // member" as "staff" and let an outsider publish into another tenant when the
    // (default-off) lockdown was off.

    #[test]
    fn composed_outsider_denied_even_when_unlocked() {
        // The regression: an outsider with lockdown off (the default) must NOT pass.
        assert_eq!(
            publish_decision(&PublishActor::Outsider, false, false),
            Err(PublishDenied::NotAuthorized)
        );
        // …and a stray consent doesn't rescue them either.
        assert_eq!(
            publish_decision(&PublishActor::Outsider, false, true),
            Err(PublishDenied::NotAuthorized)
        );
    }

    #[test]
    fn composed_plain_member_denied() {
        assert_eq!(
            publish_decision(&PublishActor::OrgMember(OrgRole::Member), false, false),
            Err(PublishDenied::NotAuthorized)
        );
    }

    #[test]
    fn composed_admin_and_partner_still_publish() {
        assert!(publish_decision(&PublishActor::OrgMember(OrgRole::Admin), false, false).is_ok());
        assert!(
            publish_decision(
                &PublishActor::Partner {
                    manages_target: true,
                    can_manage_apps: true,
                },
                false,
                true,
            )
            .is_ok()
        );
    }

    #[test]
    fn composed_staff_publishes_unless_locked() {
        assert!(publish_decision(&PublishActor::Staff, false, false).is_ok());
        assert_eq!(
            publish_decision(&PublishActor::Staff, true, false),
            Err(PublishDenied::StaffLockedOut)
        );
    }
}
