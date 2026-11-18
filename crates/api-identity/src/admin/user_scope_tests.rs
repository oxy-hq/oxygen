//! Unit tests for `super` (`admin::user_scope`): the pure decisions, no database.

use super::*;
use oxy_authz::{PlatformRole, PlatformStanding, Scope};

fn org() -> Uuid {
    Uuid::new_v4()
}

fn admin(scope: Scope) -> PrincipalFacts {
    PrincipalFacts {
        user_id: Uuid::new_v4(),
        platform: Some(PlatformStanding::from_role(
            PlatformRole::GlobalAdmin,
            scope,
        )),
        ..Default::default()
    }
}

fn owner() -> PrincipalFacts {
    PrincipalFacts {
        user_id: Uuid::new_v4(),
        is_global_owner: true,
        ..Default::default()
    }
}

fn staff(role: &str, scope_org_ids: Option<Vec<Uuid>>) -> StatusTarget {
    StatusTarget {
        is_global_owner: false,
        grant: Some(StoredGrant {
            role: role.to_string(),
            scope_all: scope_org_ids.is_none(),
            scope_org_ids: scope_org_ids.unwrap_or_default(),
        }),
    }
}

const NOBODY: StatusTarget = StatusTarget {
    is_global_owner: false,
    grant: None,
};
const ROOT: StatusTarget = StatusTarget {
    is_global_owner: true,
    grant: None,
};

#[test]
fn changing_a_user_takes_every_org_they_are_in() {
    let (a, b) = (org(), org());
    assert!(
        reaches_all_of(None, &[]),
        "unbounded reaches a user in no org"
    );
    assert!(reaches_all_of(None, &[a, b]));
    assert!(reaches_all_of(Some(&[a]), &[a]));
    assert!(
        !reaches_all_of(Some(&[a]), &[a, b]),
        "org B is past the grant"
    );
    assert!(!reaches_all_of(Some(&[a]), &[b]));
    assert!(
        !reaches_all_of(Some(&[a]), &[]),
        "a user in no org is platform-level"
    );
    assert!(!reaches_all_of(Some(&[]), &[]));
}

#[test]
fn seeing_a_user_takes_one_org_they_are_in() {
    let (a, b) = (org(), org());
    assert!(reaches_one_of(None, &[]), "unbounded sees a user in no org");
    assert!(
        reaches_one_of(Some(&[a]), &[a, b]),
        "a shared user is visible"
    );
    assert!(!reaches_one_of(Some(&[a]), &[b]));
    assert!(
        !reaches_one_of(Some(&[a]), &[]),
        "a user in no org is platform-level"
    );
}

#[test]
fn an_org_less_row_is_seen_only_unbounded() {
    let a = org();
    assert!(row_in_scope(None, None));
    assert!(row_in_scope(Some(&[a]), Some(a)));
    assert!(!row_in_scope(Some(&[a]), Some(org())));
    assert!(!row_in_scope(Some(&[a]), None));
}

#[test]
fn nobody_locks_out_root_and_anyone_may_restore_it() {
    for actor in [owner(), admin(Scope::All)] {
        assert_eq!(
            may_change_status(&actor, &ROOT, &UserStatus::Deleted),
            Err(StatusRefusal::GlobalOwner)
        );
        assert_eq!(
            may_change_status(&actor, &ROOT, &UserStatus::Active),
            Ok(())
        );
    }
}

#[test]
fn a_global_admin_cannot_change_a_peer_or_itself() {
    let a = org();
    for target in [
        staff("global_admin", None),
        staff("global_admin", Some(vec![a])),
    ] {
        for to in [UserStatus::Deleted, UserStatus::Active] {
            assert_eq!(
                may_change_status(&admin(Scope::All), &target, &to),
                Err(StatusRefusal::Delegation(DelegationDenial::RoleNotBelow))
            );
            assert_eq!(may_change_status(&owner(), &target, &to), Ok(()));
        }
    }
}

#[test]
fn an_app_operator_is_the_writers_only_within_the_writers_scope() {
    let a = org();
    let bounded = admin(Scope::Orgs(vec![a]));
    let within = staff("app_operator", Some(vec![a]));
    let wider = staff("app_operator", None);
    assert_eq!(
        may_change_status(&bounded, &within, &UserStatus::Deleted),
        Ok(())
    );
    assert_eq!(
        may_change_status(&bounded, &wider, &UserStatus::Deleted),
        Err(StatusRefusal::Delegation(
            DelegationDenial::ScopeNotContained
        ))
    );
    assert_eq!(
        may_change_status(&admin(Scope::All), &wider, &UserStatus::Deleted),
        Ok(())
    );
}

#[test]
fn a_role_this_build_cannot_name_is_the_owners_alone() {
    let target = staff("a_role_from_the_future", None);
    assert_eq!(
        may_change_status(&admin(Scope::All), &target, &UserStatus::Deleted),
        Err(StatusRefusal::Delegation(DelegationDenial::RoleNotBelow))
    );
    assert_eq!(
        may_change_status(&owner(), &target, &UserStatus::Deleted),
        Ok(())
    );
}

#[test]
fn a_target_without_standing_needs_no_delegation() {
    for to in [UserStatus::Deleted, UserStatus::Active] {
        assert_eq!(
            may_change_status(&admin(Scope::Orgs(vec![])), &NOBODY, &to),
            Ok(())
        );
    }
}
