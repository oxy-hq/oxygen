//! Unit tests for [`Caller`]. The two role caps' tests moved with them to
//! `contracts-authz` (`role_cap.rs`) in C3a.

use super::*;
use oxy_auth::token::StoredKind;
use oxy_authz::{RoleCeiling, TokenGrant};

fn user() -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::from_u128(1),
        email: Some("staff@oxy.tech".into()),
        name: "Staff".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    }
}

fn credential(kind: StoredKind) -> CredentialContext {
    CredentialContext {
        token_id: Uuid::from_u128(9),
        kind,
        principal_user_id: Uuid::from_u128(1),
        all_access: false,
        platform: false,
        partner: false,
        name: "t".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        blocked_orgs: Vec::new(),
        expires_at: None,
        service_account: None,
        grants: vec![TokenGrant {
            org_id: Uuid::from_u128(7),
            workspace_id: None,
            ceiling: RoleCeiling::Member,
        }],
        app_publish: Vec::new(),
        app_sandbox: Vec::new(),
        app_access: None,
    }
}

/// A session carries no credential, so it narrows nothing and keeps the
/// user's own standing. A legacy key used to reach here and behave the same
/// way; it cannot any more, so a session is the only caller that does.
#[test]
fn a_session_narrows_nothing() {
    let caller = Caller::of(&user(), None);
    assert_eq!(caller.reach(), None);
    assert_eq!(caller.assume_binding(), None);
    assert_eq!(caller.standing_email(), "staff@oxy.tech");
    assert!(caller.carries_platform() && caller.carries_partner());
    assert_eq!(
        caller.org_ceiling(Uuid::from_u128(123)),
        Some(RoleCeiling::Owner)
    );
    assert!(caller.touches_org(Uuid::from_u128(123)));
    assert!(!caller.bound_to_grants() && !caller.blocked_somewhere());
}

#[test]
fn an_orgs_block_does_not_bind_an_all_access_token_to_grants() {
    let mut blocked = credential(StoredKind::Personal);
    blocked.all_access = true;
    blocked.grants = Vec::new();
    blocked.blocked_orgs = vec![Uuid::from_u128(7)];
    let caller = Caller::of(&user(), Some(&blocked));
    assert!(!caller.bound_to_grants(), "it keeps the flat routes");
    assert!(caller.blocked_somewhere());
    assert!(!caller.touches_org(Uuid::from_u128(7)), "less that org");
    assert!(caller.touches_org(Uuid::from_u128(8)));

    // A grant-bound token is bound by its grants, blocked or not.
    let mut bound = credential(StoredKind::Personal);
    bound.blocked_orgs = vec![Uuid::from_u128(7)];
    let caller = Caller::of(&user(), Some(&bound));
    assert!(caller.bound_to_grants() && !caller.blocked_somewhere());
}

#[test]
fn a_new_format_token_narrows_and_owns_its_assume_sessions() {
    let token = credential(StoredKind::Personal);
    let caller = Caller::of(&user(), Some(&token));
    assert_eq!(caller.assume_binding(), Some(Uuid::from_u128(9)));
    assert_eq!(caller.standing_email(), "", "platform=false is nobody");
    assert_eq!(caller.email(), "staff@oxy.tech");
    assert!(!caller.carries_platform() && !caller.carries_partner());
    assert_eq!(
        caller.org_ceiling(Uuid::from_u128(7)),
        Some(RoleCeiling::Member)
    );
    assert_eq!(caller.org_ceiling(Uuid::from_u128(8)), None);
    assert!(!caller.touches_org(Uuid::from_u128(8)));
    assert!(caller.bound_to_grants());

    let mut all_access = credential(StoredKind::Personal);
    all_access.all_access = true;
    assert!(
        !Caller::of(&user(), Some(&all_access)).bound_to_grants(),
        "all-access has no grant to be bound by, whatever its standing flags"
    );
}

/// The old paths still name the caps that moved.
#[test]
fn the_role_caps_resolve_at_their_old_path() {
    use entity::org_members::OrgRole;
    use entity::workspace_members::WorkspaceRole;
    assert_eq!(
        cap_org_role(OrgRole::Owner, RoleCeiling::Admin),
        OrgRole::Admin
    );
    assert_eq!(
        crate::cap_workspace_role(WorkspaceRole::Owner, RoleCeiling::Member),
        WorkspaceRole::Member
    );
}

/// What code acting for a person again outside the request reads: a login
/// session is one and holds no stored token; every stored token is named by
/// its id and is not a session; a service account's token too.
#[test]
fn a_caller_says_which_stored_credential_it_arrived_on() {
    let session = Caller::of(&user(), None);
    assert!(session.is_login_session());
    assert_eq!(session.stored_token_id(), None);
    for kind in [
        StoredKind::Personal,
        StoredKind::SandboxAgent,
        StoredKind::ServiceAccount,
        StoredKind::Ci,
    ] {
        let caller = Caller::of(&user(), Some(&credential(kind)));
        assert!(!caller.is_login_session(), "{kind:?}");
        assert_eq!(
            caller.stored_token_id(),
            Some(Uuid::from_u128(9)),
            "{kind:?}"
        );
    }
}
