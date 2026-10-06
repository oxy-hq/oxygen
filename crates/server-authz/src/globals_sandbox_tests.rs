//! A sandbox agent token's platform grant is read past the 60 s cache, once
//! per request (sandbox agent credential design §4).
//!
//! Each case runs against a disconnected connection, on which any statement
//! panics: so "this read the database" and "this did not" are both observable
//! without one.

use oxy_auth::token::{AppSandboxGrant, CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{RoleCeiling, TokenGrant};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::*;

const ORG: Uuid = Uuid::from_u128(7);

fn minter(email: &str) -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::from_u128(1),
        email: Some(email.to_string()),
        name: "Minter".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    }
}

/// The minter on a sandbox agent token granted one app of [`ORG`].
fn agent(email: &str) -> Caller {
    let credential = CredentialContext {
        token_id: Uuid::from_u128(2),
        kind: StoredKind::SandboxAgent,
        principal_user_id: Uuid::from_u128(1),
        all_access: false,
        platform: true,
        partner: false,
        name: "agent".into(),
        display_prefix: "oxy_sbx_Ab3x".into(),
        legacy_api_key_id: None,
        blocked_orgs: Vec::new(),
        expires_at: None,
        service_account: None,
        grants: vec![TokenGrant {
            org_id: ORG,
            workspace_id: Some(Uuid::from_u128(8)),
            ceiling: RoleCeiling::Admin,
        }],
        app_publish: Vec::new(),
        app_sandbox: vec![AppSandboxGrant {
            org_id: ORG,
            app_id: Uuid::from_u128(9),
        }],
    };
    Caller::of(&minter(email), Some(&credential))
}

fn operator() -> Grant {
    Grant::from_role(PlatformRole::AppOperator, Scope::All)
}

/// Whether asking for `caller`'s grant ran a statement (panicked on the
/// disconnected connection), and the answer when it did not.
async fn asked(caller: Caller) -> Result<Option<Grant>, &'static str> {
    let read = tokio::spawn(async move {
        platform_grant_checked(&DatabaseConnection::default(), &caller).await
    })
    .await;
    match read {
        Ok(Ok(grant)) => Ok(grant),
        // An error, like a panic, can only come from a statement being tried.
        Ok(Err(_)) | Err(_) => Err("a statement ran"),
    }
}

/// The control, and the point: with the grant cache warm, a session is
/// answered from it and no statement runs; the token's caller reads the
/// database anyway. A grant deleted a second ago is therefore gone for the
/// token's next request, while the cache still holds it for a session.
#[tokio::test]
async fn a_sandbox_agent_token_reads_past_a_warm_grant_cache() {
    let email = "warm-cache-minter@oxy.tech";
    set_cached_admin(email.to_string(), Some(operator()));

    let session = Caller::of(&minter(email), None);
    assert_eq!(asked(session).await, Ok(Some(operator())), "from the cache");
    assert_eq!(asked(agent(email)).await, Err("a statement ran"));
}

/// What a request has read, its other guards do not read again: the memo is
/// on the caller and on every clone of it. A separately built caller — the
/// next request — reads for itself.
#[tokio::test]
async fn one_request_reads_the_grant_once() {
    let email = "memo-minter@oxy.tech";
    let caller = agent(email);
    let narrowed = carried(&caller, false, Some(operator())).1;
    caller.grant_memo().set(narrowed.clone());

    assert_eq!(asked(caller.clone()).await, Ok(narrowed), "no statement");
    assert_eq!(asked(agent(email)).await, Err("a statement ran"));
}

/// The memo holds the grant as the token carries it: bounded to the orgs of
/// its grants, whatever the minter's own scope.
#[tokio::test]
async fn the_remembered_grant_is_the_one_the_token_carries() {
    let caller = agent("scoped-minter@oxy.tech");
    let narrowed = carried(&caller, false, Some(operator())).1;
    caller.grant_memo().set(narrowed);

    let db = DatabaseConnection::default();
    assert!(platform_reaches(&db, &caller, oxy_authz::Cap::DevelopApps, ORG).await);
    let elsewhere = Uuid::from_u128(70);
    assert!(!platform_reaches(&db, &caller, oxy_authz::Cap::DevelopApps, elsewhere).await);
}

/// A caller left in the request's extensions is the one every guard gets, so
/// they share its memo. Only a sandbox agent token's is left there.
#[test]
fn only_a_sandbox_agent_caller_is_shared_through_the_extensions() {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(minter("session@oxy.tech"));
    Caller::share_with(&mut extensions);
    assert!(
        extensions.get::<Caller>().is_none(),
        "a session shares none"
    );

    let caller = agent("shared-minter@oxy.tech");
    caller.grant_memo().set(Some(operator()));
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(caller);
    let handed = Caller::from_extensions(&extensions).expect("the shared caller");
    assert_eq!(handed.grant_memo().get(), Some(Some(operator())));
    // A guard that holds the user gets the same one, not one built again.
    let for_guard = Caller::of_request(&extensions, &minter("shared-minter@oxy.tech"));
    assert_eq!(for_guard.grant_memo().get(), Some(Some(operator())));
    assert!(for_guard.is_sandbox_agent());
}

/// With nothing left in the extensions — every credential but a sandbox
/// agent token — a guard's caller is `from_user`, exactly as before.
#[test]
fn a_guards_caller_is_built_from_the_user_when_none_is_shared() {
    let user = minter("session@oxy.tech");
    let extensions = axum::http::Extensions::new();
    assert_eq!(
        Caller::of_request(&extensions, &user),
        Caller::from_user(&user)
    );
}
