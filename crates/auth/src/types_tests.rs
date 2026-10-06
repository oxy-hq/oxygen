use super::*;
use entity::users::{self, UserStatus};
use uuid::Uuid;

#[test]
fn authenticated_user_from_model_preserves_fields() {
    let now = chrono::Utc::now().into();
    let model = users::Model {
        id: Uuid::new_v4(),
        email: Some("test@example.com".to_string()),
        name: "Test User".to_string(),
        picture: Some("https://example.com/pic.jpg".to_string()),
        email_verified: true,
        magic_link_token: None,
        magic_link_token_expires_at: None,
        status: UserStatus::Active,
        created_at: now,
        last_login_at: now,
    };

    let auth_user = AuthenticatedUser::from(model.clone());

    assert_eq!(auth_user.id, model.id);
    assert_eq!(auth_user.email.as_deref(), Some("test@example.com"));
    assert_eq!(auth_user.name, "Test User");
    assert_eq!(
        auth_user.picture,
        Some("https://example.com/pic.jpg".to_string())
    );
    assert_eq!(auth_user.status, UserStatus::Active);
}

#[test]
fn a_user_with_no_email_labels_as_their_name() {
    // Frontline identity: the address is optional, `name` is not. `label()` is
    // the display fallback, and the reason no `handle` column was needed.
    let now = chrono::Utc::now().into();
    let model = users::Model {
        id: Uuid::new_v4(),
        email: None,
        name: "Maria S.".to_string(),
        picture: None,
        email_verified: false,
        magic_link_token: None,
        magic_link_token_expires_at: None,
        status: UserStatus::Active,
        created_at: now,
        last_login_at: now,
    };
    assert_eq!(model.label(), "Maria S.");

    let auth_user = AuthenticatedUser::from(model);
    assert_eq!(
        auth_user.email, None,
        "no address must stay None, never \"\""
    );
    assert_eq!(auth_user.label(), "Maria S.");
}

fn user_with(credential: Option<crate::token::CredentialContext>) -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::new_v4(),
        email: None,
        name: "deploy-bot".to_string(),
        picture: None,
        status: UserStatus::Active,
        credential,
    }
}

fn credential_of(kind: crate::token::StoredKind) -> crate::token::CredentialContext {
    crate::token::CredentialContext {
        token_id: Uuid::new_v4(),
        kind,
        principal_user_id: Uuid::new_v4(),
        all_access: kind != crate::token::StoredKind::ServiceAccount,
        platform: false,
        partner: false,
        name: "t".to_string(),
        display_prefix: "oxy_".to_string(),
        legacy_api_key_id: None,
        grants: Vec::new(),
        blocked_orgs: Vec::new(),
        expires_at: None,
        service_account: None,
        app_publish: Vec::new(),
    }
}

#[test]
fn a_surface_that_acts_as_the_caller_refuses_a_service_account_only() {
    use crate::token::StoredKind;
    use axum::http::StatusCode;

    // API-tokens design §3.3: the Airhouse credential mint, `/oltp/me/*` and
    // the flat work handlers call this first.
    let account = user_with(Some(credential_of(StoredKind::ServiceAccount)));
    assert!(account.is_service_account());
    assert_eq!(account.refuse_service_account(), Err(StatusCode::FORBIDDEN));

    // A person passes, whatever they signed in with — a legacy key included.
    for person in [
        user_with(None),
        user_with(Some(credential_of(StoredKind::Personal))),
        user_with(Some(credential_of(StoredKind::LegacyKey))),
    ] {
        assert!(!person.is_service_account());
        assert_eq!(person.refuse_service_account(), Ok(()));
    }
}

#[test]
fn an_orgs_block_takes_that_org_away_from_a_token_and_nothing_else() {
    use crate::token::StoredKind;
    use axum::http::StatusCode;

    let (blocking, other) = (Uuid::new_v4(), Uuid::new_v4());
    let blocked = |kind: StoredKind| crate::token::CredentialContext {
        blocked_orgs: vec![blocking],
        ..credential_of(kind)
    };

    // An all-access token an org blocked: that org is gone, every other stays.
    let token = user_with(Some(blocked(StoredKind::Personal)));
    assert!(!token.reaches_org(blocking));
    assert_eq!(
        token.require_org_reach(blocking),
        Err(StatusCode::NOT_FOUND)
    );
    assert!(token.reaches_org(other));
    assert_eq!(token.require_org_reach(other), Ok(()));
    assert_eq!(token.blocked_orgs(), [blocking]);

    // A session, and an all-access token nobody blocked, leave nothing out.
    for whole in [
        user_with(None),
        user_with(Some(credential_of(StoredKind::Personal))),
    ] {
        assert!(whole.reaches_org(blocking) && whole.reaches_org(other));
        assert!(whole.blocked_orgs().is_empty());
    }
}

#[test]
fn a_legacy_key_reaches_every_org_whatever_its_marker_says() {
    use crate::token::StoredKind;

    // §3.5: nothing narrows a legacy key. A block beside one — which admission
    // never writes — is not read, for either shape of legacy credential.
    let org = Uuid::new_v4();
    let seeded = crate::token::CredentialContext {
        blocked_orgs: vec![org],
        ..credential_of(StoredKind::LegacyKey)
    };
    let minted_by_the_legacy_endpoint = crate::token::CredentialContext {
        blocked_orgs: vec![org],
        legacy_api_key_id: Some(Uuid::new_v4()),
        ..credential_of(StoredKind::Personal)
    };
    for credential in [seeded, minted_by_the_legacy_endpoint] {
        let key = user_with(Some(credential));
        assert!(key.reaches_org(org));
        assert_eq!(key.require_org_reach(org), Ok(()));
        assert!(key.blocked_orgs().is_empty());
    }
}

#[test]
fn a_grant_bound_token_reaches_only_the_orgs_its_grants_name() {
    use crate::token::StoredKind;
    use oxy_authz::{RoleCeiling, TokenGrant};

    let (granted, other) = (Uuid::new_v4(), Uuid::new_v4());
    let token = user_with(Some(crate::token::CredentialContext {
        all_access: false,
        grants: vec![TokenGrant {
            org_id: granted,
            workspace_id: Some(Uuid::new_v4()),
            ceiling: RoleCeiling::Viewer,
        }],
        ..credential_of(StoredKind::Personal)
    }));
    assert!(token.reaches_org(granted));
    assert!(!token.reaches_org(other));
}
