//! What the owner of a token may put on it: the standing a flag needs, and the
//! orgs and workspaces a grant may name.
//!
//! A token only ever narrows its bearer, so none of this is an access decision
//! — a grant on an org the owner cannot reach would reach nothing. It is
//! checked anyway, and answers **404**, so that naming an id is not a way to
//! learn whether an org or workspace exists.
//!
//! The standing and the org reach are asked of `oxy-authz`'s facts for the
//! **session** making the request (management is session-only), never read
//! from the membership tables by hand.

use std::collections::HashSet;

use entity::prelude::{Organizations, Workspaces};
use entity::{organizations, workspaces};
use oxy_auth::token::personal::GrantSpec;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::error::TokenError;
use crate::server::authz::{self, Action, Caller, PrincipalFacts, Resource};

/// The session's facts. Unknown facts fail the request rather than reading as
/// "holds nothing" — a blip must not turn into a confident 403 or 404.
pub(super) async fn facts(
    db: &DatabaseConnection,
    caller: &Caller,
) -> Result<PrincipalFacts, TokenError> {
    authz::loader::load_principal_facts(db, caller)
        .await
        .ok_or_else(|| TokenError::Internal("the caller's standing could not be resolved".into()))
}

/// `platform` and `partner` each need the standing they carry (contract: 403
/// `standing_required`).
pub(super) fn require_standing(
    facts: &PrincipalFacts,
    platform: bool,
    partner: bool,
) -> Result<(), TokenError> {
    if platform && !facts.is_staff() {
        return Err(TokenError::StandingRequired("platform"));
    }
    if partner && !facts.is_partner() {
        return Err(TokenError::StandingRequired("partner"));
    }
    Ok(())
}

/// `(platform, partner)`: the standings the caller holds, and so the flags an
/// `oxyc login` token carries — each one only if it is held.
pub(super) fn standing_held(facts: &PrincipalFacts) -> (bool, bool) {
    (facts.is_staff(), facts.is_partner())
}

/// Can the caller reach `org_id` at all — as a member, as a partner managing
/// it, or as staff whose grant's scope covers it?
///
/// Scope, not a capability: this asks *where* a standing reaches, which is the
/// row-filter question `platform_scope` exists for. What the token may then do
/// there is decided per request, by the rings.
pub(super) fn reaches_org(facts: &PrincipalFacts, org_id: Uuid) -> bool {
    authz::allows(facts, Action::OrgRead, &Resource::org(org_id))
        || facts.manages(org_id)
        || facts
            .platform_scope()
            .is_some_and(|scope| scope.covers(org_id))
}

/// Every new grant names an org the caller can reach and — when it names a
/// workspace — a workspace of that org. Anything else is 404.
pub(super) async fn check_grants(
    db: &DatabaseConnection,
    facts: &PrincipalFacts,
    grants: &[GrantSpec],
) -> Result<(), TokenError> {
    if grants.is_empty() {
        return Ok(());
    }
    if grants.iter().any(|g| !reaches_org(facts, g.org_id)) {
        return Err(TokenError::NotFound);
    }
    // Staff standing over every org reaches an id that is no org at all, so
    // existence is checked on its own.
    let org_ids: HashSet<Uuid> = grants.iter().map(|g| g.org_id).collect();
    let orgs = Organizations::find()
        .filter(organizations::Column::Id.is_in(org_ids.iter().copied()))
        .all(db)
        .await?;
    if orgs.len() != org_ids.len() {
        return Err(TokenError::NotFound);
    }
    let workspace_ids: HashSet<Uuid> = grants.iter().filter_map(|g| g.workspace_id).collect();
    if workspace_ids.is_empty() {
        return Ok(());
    }
    let rows = Workspaces::find()
        .filter(workspaces::Column::Id.is_in(workspace_ids.iter().copied()))
        .all(db)
        .await?;
    let in_its_org = |grant: &GrantSpec| match grant.workspace_id {
        None => true,
        Some(id) => rows
            .iter()
            .any(|w| w.id == id && w.org_id == Some(grant.org_id)),
    };
    if grants.iter().all(in_its_org) {
        Ok(())
    } else {
        Err(TokenError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_authz::{Cap, PartnerStanding, PlatformRole, PlatformStanding, Scope};

    const ORG: Uuid = Uuid::from_u128(0xA);
    const CLIENT: Uuid = Uuid::from_u128(0xB);
    const ELSEWHERE: Uuid = Uuid::from_u128(0xC);

    fn member() -> PrincipalFacts {
        PrincipalFacts {
            user_id: Uuid::from_u128(1),
            member_orgs: vec![ORG],
            ..Default::default()
        }
    }

    fn partner() -> PrincipalFacts {
        PrincipalFacts {
            partners: vec![PartnerStanding {
                partner_id: Uuid::from_u128(0xD),
                client_orgs: vec![CLIENT],
                caps: vec![Cap::ManageMembers],
            }],
            ..member()
        }
    }

    fn staff_as(role: PlatformRole, scope: Scope) -> PrincipalFacts {
        PrincipalFacts {
            platform: Some(PlatformStanding::from_role(role, scope)),
            ..member()
        }
    }

    fn staff(scope: Scope) -> PrincipalFacts {
        staff_as(PlatformRole::GlobalAdmin, scope)
    }

    #[test]
    fn a_standing_flag_needs_the_standing() {
        let plain = member();
        assert!(require_standing(&plain, false, false).is_ok());
        assert!(matches!(
            require_standing(&plain, true, false),
            Err(TokenError::StandingRequired("platform"))
        ));
        assert!(matches!(
            require_standing(&plain, false, true),
            Err(TokenError::StandingRequired("partner"))
        ));
        assert!(require_standing(&staff(Scope::All), true, false).is_ok());
        assert!(require_standing(&partner(), false, true).is_ok());
        // One standing does not stand in for the other.
        assert!(require_standing(&partner(), true, false).is_err());
        assert!(require_standing(&staff(Scope::All), false, true).is_err());
    }

    #[test]
    fn a_grant_may_name_an_org_the_caller_reaches() {
        assert!(reaches_org(&member(), ORG));
        assert!(!reaches_org(&member(), ELSEWHERE));
        // A partner reaches its clients, staff the orgs its grant covers.
        assert!(reaches_org(&partner(), CLIENT));
        assert!(!reaches_org(&partner(), ELSEWHERE));
        assert!(reaches_org(&staff(Scope::All), ELSEWHERE));
        assert!(reaches_org(&staff(Scope::Orgs(vec![CLIENT])), CLIENT));
        assert!(!reaches_org(&staff(Scope::Orgs(vec![CLIENT])), ELSEWHERE));
        // An App Operator holds no tenant-reading capability, and still reaches
        // exactly the orgs its grant is scoped to.
        let operator = staff_as(PlatformRole::AppOperator, Scope::Orgs(vec![CLIENT]));
        assert!(reaches_org(&operator, CLIENT));
        assert!(!reaches_org(&operator, ELSEWHERE));
    }
}
