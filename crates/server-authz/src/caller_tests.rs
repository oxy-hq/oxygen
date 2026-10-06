//! Unit tests for [`Caller`] and the two role caps.

use super::*;
use oxy_auth::token::StoredKind;
use oxy_authz::TokenGrant;

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

fn credential(kind: StoredKind, legacy_api_key_id: Option<Uuid>) -> CredentialContext {
    CredentialContext {
        token_id: Uuid::from_u128(9),
        kind,
        principal_user_id: Uuid::from_u128(1),
        all_access: false,
        platform: false,
        partner: false,
        name: "t".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        legacy_api_key_id,
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
    }
}

#[test]
fn a_session_and_a_legacy_credential_narrow_nothing() {
    // The marker's flags are irrelevant for a legacy credential: it is
    // all-access with both standings by definition (design §3.5), and it
    // shares the browser's assume-role sessions.
    let legacy_key = credential(StoredKind::LegacyKey, Some(Uuid::from_u128(9)));
    let legacy_endpoint_token = credential(StoredKind::Personal, Some(Uuid::from_u128(9)));
    for caller in [
        Caller::of(&user(), None),
        Caller::of(&user(), Some(&legacy_key)),
        Caller::of(&user(), Some(&legacy_endpoint_token)),
    ] {
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
}

#[test]
fn an_orgs_block_does_not_bind_an_all_access_token_to_grants() {
    let mut blocked = credential(StoredKind::Personal, None);
    blocked.all_access = true;
    blocked.grants = Vec::new();
    blocked.blocked_orgs = vec![Uuid::from_u128(7)];
    let caller = Caller::of(&user(), Some(&blocked));
    assert!(!caller.bound_to_grants(), "it keeps the flat routes");
    assert!(caller.blocked_somewhere());
    assert!(!caller.touches_org(Uuid::from_u128(7)), "less that org");
    assert!(caller.touches_org(Uuid::from_u128(8)));

    // A legacy key is never blocked, whatever rides beside it.
    let mut legacy = credential(StoredKind::LegacyKey, Some(Uuid::from_u128(9)));
    legacy.blocked_orgs = vec![Uuid::from_u128(7)];
    let caller = Caller::of(&user(), Some(&legacy));
    assert!(!caller.blocked_somewhere() && caller.touches_org(Uuid::from_u128(7)));

    // A grant-bound token is bound by its grants, blocked or not.
    let mut bound = credential(StoredKind::Personal, None);
    bound.blocked_orgs = vec![Uuid::from_u128(7)];
    let caller = Caller::of(&user(), Some(&bound));
    assert!(caller.bound_to_grants() && !caller.blocked_somewhere());
}

#[test]
fn a_new_format_token_narrows_and_owns_its_assume_sessions() {
    let token = credential(StoredKind::Personal, None);
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

    let mut all_access = credential(StoredKind::Personal, None);
    all_access.all_access = true;
    assert!(
        !Caller::of(&user(), Some(&all_access)).bound_to_grants(),
        "all-access has no grant to be bound by, whatever its standing flags"
    );
}

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
