//! `POST /orgs/{org_id}/github/namespaces/installation` — the picker's route.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

use crate::installation_fixture::{
    attach, connect_github, fixture, github_answers, github_lists, github_lists_installations,
    installation, installations_page, login_of, namespaces_of, seed_person, user_installation,
};

#[tokio::test]
async fn a_plain_member_cannot_attach_an_installation() {
    let fx = fixture().await;
    let member = seed_person(&fx.db, fx.org, Some(OrgRole::Member)).await;
    connect_github(&fx.db, member.id, "tok-member").await;
    github_lists(&fx.github, "tok-member", &[101]).await;

    let (status, _) = attach(&member, fx.org, 101).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "adding a namespace is org admin"
    );
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_admin_cannot_attach_an_installation_their_github_account_cannot_see() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-admin").await;
    github_lists(&fx.github, "tok-admin", &[101]).await;

    // 202 is somebody else's; 999 exists nowhere. From here they must look the same.
    let (foreign, foreign_body) = attach(&admin, fx.org, 202).await;
    let (missing, missing_body) = attach(&admin, fx.org, 999).await;

    assert_eq!(foreign, StatusCode::NOT_FOUND, "{foreign_body}");
    assert_eq!(
        (foreign, foreign_body),
        (missing, missing_body),
        "a foreign installation answers exactly as a missing one"
    );
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_admin_attaches_an_installation_from_their_own_list() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Owner)).await;
    connect_github(&fx.db, admin.id, "tok-owner").await;
    github_lists(&fx.github, "tok-owner", &[101, 102]).await;

    let (status, body) = attach(&admin, fx.org, 102).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["slug"], "acct-102");
    assert_eq!(body["owner_type"], "Organization");
    let rows = namespaces_of(&fx.db, fx.org).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].installation_id, 102);
    assert_eq!(rows[0].created_by, admin.id);
    assert_eq!(rows[0].name, "acct-102");
}

/// GitHub lists a personal account's installation to anyone the owner made a
/// collaborator on one repository. Listing it is not owning it.
#[tokio::test]
async fn an_admin_cannot_attach_another_persons_personal_installation_they_can_see() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-admin").await;
    github_lists_installations(
        &fx.github,
        "tok-admin",
        vec![user_installation(505, "someone-else")],
    )
    .await;

    let (foreign, foreign_body) = attach(&admin, fx.org, 505).await;
    let (missing, missing_body) = attach(&admin, fx.org, 999).await;

    assert_eq!(foreign, StatusCode::NOT_FOUND, "{foreign_body}");
    assert_eq!(
        (foreign, foreign_body),
        (missing, missing_body),
        "someone else's personal installation answers exactly as a missing one"
    );
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_admin_attaches_their_own_personal_installation() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-admin").await;
    // GitHub logins are case-insensitive; the stored one may differ in case.
    let own = login_of("tok-admin").to_uppercase();
    github_lists_installations(&fx.github, "tok-admin", vec![user_installation(606, &own)]).await;

    let (status, body) = attach(&admin, fx.org, 606).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["owner_type"], "User");
    assert_eq!(namespaces_of(&fx.db, fx.org).await.len(), 1);
}

#[tokio::test]
async fn an_admin_with_no_github_account_is_asked_to_connect_one() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;

    let (status, body) = attach(&admin, fx.org, 101).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "github_account_not_connected", "{body}");
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

/// A token GitHub no longer accepts is a reconnect, not a sign-out: `409`
/// with the code the UI keys on, never a `401`.
#[tokio::test]
async fn an_admin_whose_github_token_is_rejected_is_asked_to_reconnect() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-revoked").await;
    let rejected = ResponseTemplate::new(401).set_body_json(serde_json::json!({
        "message": "Bad credentials",
    }));
    github_answers(&fx.github, "tok-revoked", rejected).await;

    let (status, body) = attach(&admin, fx.org, 101).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "github_account_not_connected", "{body}");
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

/// When GitHub cannot answer, nothing shows the installation is the
/// caller's, so nothing is attached.
#[tokio::test]
async fn a_github_outage_attaches_nothing() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-admin").await;
    github_answers(&fx.github, "tok-admin", ResponseTemplate::new(503)).await;

    let (status, body) = attach(&admin, fx.org, 101).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(namespaces_of(&fx.db, fx.org).await.is_empty());
}

#[tokio::test]
async fn an_installation_past_the_first_page_of_the_callers_list_is_found() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;
    connect_github(&fx.db, admin.id, "tok-many").await;
    let first: Vec<_> = (1..=100).map(installation).collect();
    Mock::given(method("GET"))
        .and(path("/user/installations"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(installations_page(first)))
        .mount(&fx.github)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/installations"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(installations_page(vec![installation(4242)])),
        )
        .mount(&fx.github)
        .await;

    let (status, body) = attach(&admin, fx.org, 4242).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(namespaces_of(&fx.db, fx.org).await.len(), 1);
}
