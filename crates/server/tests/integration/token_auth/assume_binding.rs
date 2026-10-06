//! An assume-role session belongs to the credential that opened it (design
//! §3.2). A new-format token neither inherits the browser's session nor lends
//! its own; a legacy key keeps inheriting the browser's, exactly as before
//! (§3.5). And a token opens one only when it carries `platform`.
//!
//! The org under test is one the staff member is **not** a member of, so the
//! only way in is an assume session: `GET /orgs/{org}/workspaces` answers 403
//! without one and 200 with one.

use axum::http::StatusCode;
use entity::admin_assume_sessions;
use sea_orm::EntityTrait;
use serde_json::json;
use uuid::Uuid;

use super::stack::{Reach, flat_api, get_as, get_in_session, make_staff, mint};
use super::{Fixture, call, endpoint_key, fixture, legacy_key};

/// A staff member and a tenant org they hold no membership in.
async fn staff_fixture() -> Fixture {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    fx
}

fn listing(fx: &Fixture) -> String {
    format!("/orgs/{}/workspaces", fx.org_id)
}

async fn open_assume(fx: &Fixture, header: (&str, &str)) -> StatusCode {
    call(
        flat_api(),
        "POST",
        "/assume",
        &[header],
        Some(json!({ "org_id": fx.org_id, "reason": "support ticket 4127" })),
    )
    .await
    .0
}

async fn sessions(fx: &Fixture) -> Vec<Option<Uuid>> {
    admin_assume_sessions::Entity::find()
        .all(&fx.db)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.token_id)
        .collect()
}

#[tokio::test]
async fn a_browser_opened_session_is_inherited_by_legacy_keys_and_by_no_new_token() {
    let fx = staff_fixture().await;
    let (_, key) = legacy_key(&fx, None).await;
    let (_, endpoint_pat) = endpoint_key(&fx, None).await;
    let (_, token) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;

    // Standing alone does not enter a tenant.
    assert_eq!(
        get_in_session(&fx, &listing(&fx)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(get_as(&key, &listing(&fx)).await.0, StatusCode::FORBIDDEN);

    assert_eq!(
        open_assume(&fx, ("cookie", &fx.cookie)).await,
        StatusCode::OK
    );
    assert_eq!(sessions(&fx).await, vec![None], "bound to no token");
    assert_eq!(get_in_session(&fx, &listing(&fx)).await.0, StatusCode::OK);

    for (what, secret) in [
        ("an oxy_<hex> key", &key),
        ("a legacy-endpoint key", &endpoint_pat),
    ] {
        assert_eq!(
            get_as(secret, &listing(&fx)).await.0,
            StatusCode::OK,
            "{what} inherits the browser's assume session, as it always has"
        );
    }

    assert_eq!(
        get_as(&token, &listing(&fx)).await.0,
        StatusCode::FORBIDDEN,
        "a new-format token does not — not even all-access with platform"
    );
}

#[tokio::test]
async fn a_token_without_platform_cannot_open_a_session() {
    let fx = staff_fixture().await;
    let (_, token) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let bearer = format!("Bearer {token}");

    assert_eq!(
        open_assume(&fx, ("authorization", &bearer)).await,
        StatusCode::FORBIDDEN
    );
    assert!(sessions(&fx).await.is_empty(), "nothing was opened");
    assert_eq!(get_as(&token, &listing(&fx)).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_platform_token_opens_a_session_only_it_can_use() {
    let fx = staff_fixture().await;
    let (id, token) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    let (_, other) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    let (_, key) = legacy_key(&fx, None).await;
    let bearer = format!("Bearer {token}");

    assert_eq!(
        open_assume(&fx, ("authorization", &bearer)).await,
        StatusCode::OK
    );
    assert_eq!(sessions(&fx).await, vec![Some(id)], "bound to the token");
    assert_eq!(get_as(&token, &listing(&fx)).await.0, StatusCode::OK);

    for (who, status) in [
        (
            "the browser session",
            get_in_session(&fx, &listing(&fx)).await.0,
        ),
        (
            "another token of the same user",
            get_as(&other, &listing(&fx)).await.0,
        ),
        ("a legacy key", get_as(&key, &listing(&fx)).await.0),
    ] {
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{who} does not ride a session a token opened"
        );
    }
}
