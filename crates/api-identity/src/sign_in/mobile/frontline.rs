//! Frontline crew are out of scope for the mobile app.
//!
//! A crew member signs in by PIN on a store's enrolled kiosk, and the twelve-
//! hour session that mints is meant to stay on that device
//! (`internal-docs/frontline-identity.md`). Approving a mobile sign-in from it
//! would carry the crew identity off the kiosk for thirty days — the boundary
//! PIN sign-in exists to enforce. So a **frontline-only** principal is refused
//! at `authorize` (403 `frontline_not_supported`), and again at `exchange`
//! (400 `invalid_code`) for a code minted before that refusal existed.
//!
//! Frontline-only is read off the facts every authorization ring reads
//! (`server::authz::loader`): frontline standing (`frontline_orgs`, active
//! `org_frontline_members` rows, empty for anyone who signed in with an email
//! address) and no org membership (`member_orgs`, any role).

use oxy_app_core::audit::RequestActor;
use oxy_authz::PrincipalFacts;
use sea_orm::DatabaseConnection;

use super::error::MobileAuthError;
use crate::authz;

/// Holds frontline standing and nothing an org member holds.
pub(super) fn is_frontline_only(facts: &PrincipalFacts) -> bool {
    !facts.frontline_orgs.is_empty() && facts.member_orgs.is_empty()
}

/// Whether `actor` is frontline-only. Unknown standing is an error, never a
/// "no": an unreadable fact must not wave a crew session through.
pub(super) async fn frontline_only(
    db: &DatabaseConnection,
    actor: &RequestActor,
) -> Result<bool, MobileAuthError> {
    let caller = authz::caller_of(actor);
    let facts = authz::loader::load_principal_facts_scoped(db, &caller, false)
        .await
        .ok_or_else(|| {
            MobileAuthError::Internal("the caller's standing could not be resolved".into())
        })?;
    Ok(is_frontline_only(&facts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn facts(frontline: &[u128], member: &[u128]) -> PrincipalFacts {
        PrincipalFacts {
            frontline_orgs: frontline.iter().copied().map(Uuid::from_u128).collect(),
            member_orgs: member.iter().copied().map(Uuid::from_u128).collect(),
            ..PrincipalFacts::default()
        }
    }

    #[test]
    fn a_crew_member_with_no_membership_is_frontline_only() {
        assert!(is_frontline_only(&facts(&[1], &[])));
        assert!(is_frontline_only(&facts(&[1, 2], &[])));
    }

    #[test]
    fn a_member_or_a_principal_without_frontline_standing_is_not() {
        // An ordinary member, and a member who is also on a roster.
        assert!(!is_frontline_only(&facts(&[], &[1])));
        assert!(!is_frontline_only(&facts(&[2], &[1])));
        // A signed-in user with no org at all is not crew either.
        assert!(!is_frontline_only(&facts(&[], &[])));
    }
}
