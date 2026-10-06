//! `/api/admin/sandbox-agent-tokens`: every minter's sandbox agent tokens,
//! for staff who hold `operate_platform` (sandbox agent credential design §2,
//! "Revoked by"). The capability admits; the grant's scope narrows the rows.

use axum::http::StatusCode;
use oxy_authz::PlatformRole;
use serde_json::json;
use uuid::Uuid;

use super::sandbox_agent::{
    another_session, grant_staff, mint_body, minted, post_as, staff_with_app,
};
use super::stack::{Reach, flat_api, get_in_session, mint};
use super::{audit_rows, call, pat_row, seed_org};

#[tokio::test]
async fn staff_who_operate_the_platform_list_and_revoke_every_minters_tokens() {
    let (fx, app) = staff_with_app().await;
    // Minted by an App Operator; the fixture's Global Admin is the staff view.
    let (operator, operator_cookie) = another_session(&fx.db, "operator").await;
    grant_staff(
        &fx.db,
        operator.email.as_deref().unwrap(),
        PlatformRole::AppOperator,
        &[fx.org_id],
    )
    .await;
    let (status, body) = post_as(&operator_cookie, "/user/tokens", mint_body(&[app.id])).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = Uuid::parse_str(body["token"]["id"].as_str().unwrap()).unwrap();

    // The operator holds `manage_apps`, not `operate_platform`: no staff view.
    let (status, _) = call(
        flat_api(),
        "GET",
        "/admin/sandbox-agent-tokens",
        &[("cookie", &operator_cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let revoke = format!("/admin/sandbox-agent-tokens/{id}/revoke");
    let (status, _) = post_as(&operator_cookie, &revoke, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, list) = get_in_session(&fx, "/admin/sandbox-agent-tokens").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let tokens = list["tokens"].as_array().expect("tokens");
    assert_eq!(tokens.len(), 1, "{list}");
    assert_eq!(tokens[0]["id"], json!(id));
    assert_eq!(tokens[0]["kind"], "sandbox_agent");
    assert_eq!(tokens[0]["owner"]["id"], json!(operator.id));
    assert_eq!(tokens[0]["owner"]["label"], json!(operator.email));
    assert!(
        list.to_string().find("oxy_sbx_").is_some(),
        "the display prefix"
    );
    assert!(tokens[0].get("secret").is_none() && tokens[0].get("token_hash").is_none());

    let (status, revoked) = post_as(&fx.cookie, &revoke, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["status"], "revoked");
    let row = pat_row(&fx.db, id).await;
    assert_eq!(row.revoke_reason.as_deref(), Some("staff"));
    assert_eq!(row.revoked_by, Some(fx.user.id));
    let audit = audit_rows(&fx.db, "token.revoked").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].org_id, Some(fx.org_id));
    assert_eq!(
        audit[0].actor_user_id,
        Some(fx.user.id),
        "audited as the staff member"
    );
    assert_eq!(audit[0].metadata["reason"], "staff");

    // Idempotent, and it records nothing the second time.
    let (status, _) = post_as(&fx.cookie, &revoke, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(audit_rows(&fx.db, "token.revoked").await.len(), 1);

    // The staff routes reach sandbox agent tokens only.
    let (pat_id, _) = mint(&fx.db, operator.id, Reach::all_access()).await;
    let (status, _) = post_as(
        &fx.cookie,
        &format!("/admin/sandbox-agent-tokens/{pat_id}/revoke"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(pat_row(&fx.db, pat_id).await.revoked_at.is_none());
}

#[tokio::test]
async fn a_bounded_staff_grant_sees_only_tokens_touching_its_orgs() {
    let (fx, app) = staff_with_app().await;
    let (id, _) = minted(&fx, &[app.id]).await;

    // A Global Admin whose grant is bounded to another org.
    let other_org = seed_org(&fx.db).await;
    let (bounded, bounded_cookie) = another_session(&fx.db, "bounded").await;
    grant_staff(
        &fx.db,
        bounded.email.as_deref().unwrap(),
        PlatformRole::GlobalAdmin,
        &[other_org],
    )
    .await;

    let (status, list) = call(
        flat_api(),
        "GET",
        "/admin/sandbox-agent-tokens",
        &[("cookie", &bounded_cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["tokens"], json!([]), "outside the grant's scope");
    let (status, _) = post_as(
        &bounded_cookie,
        &format!("/admin/sandbox-agent-tokens/{id}/revoke"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "not found, not forbidden");
    assert!(pat_row(&fx.db, id).await.revoked_at.is_none());
}
