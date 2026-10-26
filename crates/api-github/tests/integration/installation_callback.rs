//! `POST /user/github/callback` — completing a flow GitHub sent the browser
//! back from. The org and the person ride in the signed `state`.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use oxy_api_github::state::{Flow, StatePayload, encode_state_with_timestamp};
use serde_json::json;

use crate::installation_fixture::{
    complete, connect_github, fixture, github_lists, github_lists_installations, install_url,
    namespaces_of, oauth_url, seed_person, set_role, state_of, user_installation,
};

#[tokio::test]
async fn an_install_state_minted_for_another_user_is_refused() {
    let fx = fixture().await;
    let alice = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    let bob = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, bob.id, "tok-bob").await;
    github_lists(&fx.github, "tok-bob", &[303]).await;
    let (_, minted) = install_url(&alice, fx.org).await;

    let (status, body) = complete(
        &bob,
        json!({ "state": state_of(&minted), "installation_id": 303 }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_oauth_state_minted_for_another_user_is_refused() {
    let fx = fixture().await;
    let alice = seed_person(&fx.db, fx.org, Some(OrgRole::Member)).await;
    let bob = seed_person(&fx.db, fx.org, Some(OrgRole::Member)).await;
    let (minted_status, minted) = oauth_url(&alice, fx.org).await;
    assert_eq!(minted_status, StatusCode::OK, "{minted}");

    let (status, body) = complete(
        &bob,
        json!({ "state": state_of(&minted), "code": "gh-code" }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// A state that is stale, tampered with or in the old format is a bad
/// request, not a dead session: the web app reads any `401` as "signed out",
/// clears the session and sends the person to log in again.
#[tokio::test]
async fn a_stale_tampered_or_old_format_state_is_a_bad_request_not_a_sign_out() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    let (_, minted) = install_url(&admin, fx.org).await;
    let mut tampered = state_of(&minted);
    let last = tampered.pop().expect("a non-empty state");
    tampered.push(if last == 'a' { 'b' } else { 'a' });
    let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).timestamp();
    let expired = encode_state_with_timestamp(
        &StatePayload {
            org_id: fx.org,
            user_id: admin.id,
            flow: Flow::Install,
        },
        two_hours_ago,
    )
    .expect("signs");
    let old_format = format!("{}:install:{two_hours_ago}:deadbeef", fx.org);

    for (what, state) in [
        ("tampered", tampered),
        ("expired", expired),
        ("old 4-part", old_format),
        ("garbage", "not-a-state".to_string()),
    ] {
        let (status, body) =
            complete(&admin, json!({ "state": state, "installation_id": 101 })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what} state: {body}");
    }
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_install_callback_for_an_installation_not_in_the_callers_list_is_refused() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-admin").await;
    github_lists(&fx.github, "tok-admin", &[101]).await;
    let (_, minted) = install_url(&admin, fx.org).await;

    let (status, body) = complete(
        &admin,
        json!({ "state": state_of(&minted), "installation_id": 202 }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_install_callback_for_another_persons_personal_installation_is_refused() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-admin").await;
    github_lists_installations(
        &fx.github,
        "tok-admin",
        vec![user_installation(505, "someone-else")],
    )
    .await;
    let (_, minted) = install_url(&admin, fx.org).await;

    let (status, body) = complete(
        &admin,
        json!({ "state": state_of(&minted), "installation_id": 505 }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_install_callback_asks_again_whether_the_caller_is_an_admin() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-admin").await;
    github_lists(&fx.github, "tok-admin", &[101]).await;
    let (_, minted) = install_url(&admin, fx.org).await;
    // Demoted between starting the install and GitHub sending them back.
    set_role(&fx.db, fx.org, admin.id, OrgRole::Member).await;

    let (status, body) = complete(
        &admin,
        json!({ "state": state_of(&minted), "installation_id": 101 }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_install_callback_attaches_the_callers_own_installation() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-admin").await;
    github_lists(&fx.github, "tok-admin", &[101]).await;
    let (_, minted) = install_url(&admin, fx.org).await;

    let (status, body) = complete(
        &admin,
        json!({ "state": state_of(&minted), "installation_id": 101 }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["flow"], "install", "{body}");
    let rows = namespaces_of(&fx.db, fx.org).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(body["namespace_id"], rows[0].id.to_string());
    assert_eq!(rows[0].installation_id, 101);
    assert_eq!(rows[0].slug, "acct-101");
}
