//! `GET /user/github/installations/new-url` — starting an install names the
//! org in a query, so the `OrgAdmin` door is asked here.

use axum::http::StatusCode;
use entity::org_members::OrgRole;

use crate::installation_fixture::{fixture, install_url, seed_person};

#[tokio::test]
async fn a_non_member_cannot_start_an_install_for_an_org() {
    let fx = fixture().await;
    let outsider = seed_person(&fx.db, fx.org, None).await;

    let (status, body) = install_url(&outsider, fx.org).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn a_plain_member_cannot_start_an_install() {
    let fx = fixture().await;
    let member = seed_person(&fx.db, fx.org, Some(OrgRole::Member)).await;

    let (status, body) = install_url(&member, fx.org).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn an_admin_gets_an_install_url() {
    let fx = fixture().await;
    let admin = seed_person(&fx.db, fx.org, Some(OrgRole::Admin)).await;

    let (status, body) = install_url(&admin, fx.org).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["url"]
            .as_str()
            .is_some_and(|u| u.starts_with("https://github.com/apps/oxy-test/installations/new?")),
        "{body}"
    );
}
