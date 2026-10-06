//! Phase 4: trust policies on a service account (design §3.4), through the
//! served routes — who may manage them, what a body may ask for, how the
//! repository's ids are found, and what each change writes to the org's chain.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use entity::{oidc_trust_policies, oidc_trust_policy_grants};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::oidc::{
    OWNER_ID, REPO_ID, RESOLVED_OWNER_ID, RESOLVED_REPO_ID, policies_uri, policy_body,
    register_policy, trust_test_keys, whole_org,
};
use super::service_accounts::{admin_fixture, create_account, in_session, session_of, with_token};
use super::stack::{Reach, join_org, mint, published_app};
use super::{Fixture, audit_rows, seed_org, seed_user, seed_workspace_in};

async fn policy_rows(db: &DatabaseConnection, sa_id: Uuid) -> Vec<oidc_trust_policies::Model> {
    oidc_trust_policies::Entity::find()
        .filter(oidc_trust_policies::Column::ServiceAccountId.eq(sa_id))
        .all(db)
        .await
        .unwrap()
}

async fn grant_rows(db: &DatabaseConnection, policy: Uuid) -> Vec<oidc_trust_policy_grants::Model> {
    oidc_trust_policy_grants::Entity::find()
        .filter(oidc_trust_policy_grants::Column::PolicyId.eq(policy))
        .all(db)
        .await
        .unwrap()
}

async fn post(fx: &Fixture, sa_id: Uuid, body: Value) -> (StatusCode, Value) {
    in_session(
        &fx.cookie,
        "POST",
        &policies_uri(fx.org_id, sa_id),
        Some(body),
    )
    .await
}

fn without(mut body: Value, key: &str) -> Value {
    body.as_object_mut().unwrap().remove(key);
    body
}

fn with(mut body: Value, key: &str, value: Value) -> Value {
    body[key] = value;
    body
}

#[tokio::test]
async fn an_admin_registers_lists_edits_and_deletes_a_policy() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let uri = policies_uri(fx.org_id, sa);

    let (status, policy) = post(&fx, sa, policy_body(json!([whole_org("member")]))).await;
    assert_eq!(status, StatusCode::CREATED, "{policy}");
    let id = policy["id"].as_str().unwrap().to_string();
    assert_eq!(policy["org_id"], fx.org_id.to_string());
    assert_eq!(policy["service_account_id"], sa.to_string());
    assert_eq!(policy["provider"], "github_actions");
    assert_eq!(policy["repository"], "acme/app");
    assert_eq!(policy["repository_id"], REPO_ID);
    assert_eq!(policy["repository_owner_id"], OWNER_ID);
    assert_eq!(policy["workflow_path"], ".github/workflows/release.yml");
    assert_eq!(policy["environment"], "production");
    assert_eq!(policy["ref_pattern"], Value::Null);
    assert_eq!(policy["allow_self_hosted"], false);
    assert_eq!(policy["created_by"]["id"], fx.user.id.to_string());
    assert_eq!(policy["last_used_at"], Value::Null);
    assert_eq!(policy["disabled_at"], Value::Null);
    let grants = policy["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["kind"], "workspace");
    assert_eq!(grants[0]["org_id"], fx.org_id.to_string());
    assert_eq!(grants[0]["workspace_id"], Value::Null);
    assert_eq!(grants[0]["role_ceiling"], "member");

    // Listed, and counted on its account.
    let (status, listed) = in_session(&fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["trust_policies"].as_array().unwrap().len(), 1);
    let account_uri = format!("/orgs/{}/service-accounts/{sa}", fx.org_id);
    let (_, account) = in_session(&fx.cookie, "GET", &account_uri, None).await;
    assert_eq!(account["trust_policy_count"], 1);

    // PATCH replaces the grant set, and clears what it sends as null.
    let edit = json!({
        "workflow_path": ".github/workflows/deploy.yml",
        "environment": "staging",
        "ref_pattern": "refs/tags/v*",
        "allow_self_hosted": true,
        "grants": [{ "kind": "workspace", "workspace_id": fx.workspace_id, "role_ceiling": "viewer" }],
    });
    let one = format!("{uri}/{id}");
    let (status, edited) = in_session(&fx.cookie, "PATCH", &one, Some(edit)).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["workflow_path"], ".github/workflows/deploy.yml");
    assert_eq!(edited["environment"], "staging");
    assert_eq!(edited["ref_pattern"], "refs/tags/v*");
    assert_eq!(edited["allow_self_hosted"], true);
    let grants = edited["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 1, "the set was replaced, not added to");
    assert_eq!(grants[0]["workspace_id"], fx.workspace_id.to_string());
    assert_eq!(grants[0]["role_ceiling"], "viewer");
    let cleared = json!({ "ref_pattern": null });
    let (_, edited) = in_session(&fx.cookie, "PATCH", &one, Some(cleared)).await;
    assert_eq!(edited["ref_pattern"], Value::Null);
    assert_eq!(edited["environment"], "staging", "left out is left alone");

    // Disabling stamps it; the policy stays listed.
    let (_, disabled) =
        in_session(&fx.cookie, "PATCH", &one, Some(json!({ "disabled": true }))).await;
    assert!(disabled["disabled_at"].is_string());

    let (status, _) = in_session(&fx.cookie, "DELETE", &one, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(policy_rows(&fx.db, sa).await.is_empty());
    let (status, _) = in_session(&fx.cookie, "DELETE", &one, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn each_change_is_written_to_the_orgs_chain() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let id = register_policy(&fx, sa, policy_body(json!([whole_org("member")]))).await;
    let one = format!("{}/{id}", policies_uri(fx.org_id, sa));
    in_session(
        &fx.cookie,
        "PATCH",
        &one,
        Some(json!({ "allow_self_hosted": true })),
    )
    .await;
    // An edit that changes nothing writes nothing.
    in_session(
        &fx.cookie,
        "PATCH",
        &one,
        Some(json!({ "allow_self_hosted": true })),
    )
    .await;
    in_session(&fx.cookie, "DELETE", &one, None).await;

    for action in [
        "trust_policy.created",
        "trust_policy.updated",
        "trust_policy.deleted",
    ] {
        let rows = audit_rows(&fx.db, action).await;
        assert_eq!(rows.len(), 1, "{action}");
        let row = &rows[0];
        assert_eq!(
            row.org_id,
            Some(fx.org_id),
            "{action} is in the org's chain"
        );
        assert!(row.hash.is_some(), "{action} is chained");
        assert_eq!(row.actor_user_id, Some(fx.user.id));
        assert_eq!(row.target_type.as_deref(), Some("trust_policy"));
        assert_eq!(row.target_id.as_deref(), Some(id.to_string().as_str()));
    }
    let created = &audit_rows(&fx.db, "trust_policy.created").await[0];
    assert_eq!(created.metadata["repository_id"], REPO_ID);
    assert_eq!(created.metadata["service_account"], "deployer");
    let updated = &audit_rows(&fx.db, "trust_policy.updated").await[0];
    assert_eq!(updated.before.as_ref().unwrap()["allow_self_hosted"], false);
    assert_eq!(updated.after.as_ref().unwrap()["allow_self_hosted"], true);
}

#[tokio::test]
async fn only_an_org_admin_in_a_browser_session_manages_policies() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let uri = policies_uri(fx.org_id, sa);
    let id = register_policy(&fx, sa, policy_body(json!([whole_org("member")]))).await;
    let one = format!("{uri}/{id}");
    let body = policy_body(json!([whole_org("member")]));

    // A plain member: 403 on every route.
    let member = seed_user(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    let member_cookie = session_of(&member).await;
    for (method, target, payload) in [
        ("GET", &uri, None),
        ("POST", &uri, Some(body.clone())),
        ("PATCH", &one, Some(json!({ "disabled": true }))),
        ("DELETE", &one, None),
    ] {
        let (status, _) = in_session(&member_cookie, method, target, payload).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a member's {method}");
    }

    // The admin's own all-access token reads, and may not write.
    let (_, token) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let (status, _) = with_token(&token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    for (method, target, payload) in [
        ("POST", &uri, Some(body.clone())),
        ("PATCH", &one, Some(json!({ "disabled": true }))),
        ("DELETE", &one, None),
    ] {
        let (status, refused) = with_token(&token, method, target, payload).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a token's {method}");
        assert_eq!(refused["code"], "session_required", "a token's {method}");
    }
    assert_eq!(
        policy_rows(&fx.db, sa).await.len(),
        1,
        "nothing was written"
    );

    // Another org's admin cannot reach this org's account at all.
    let outsider = seed_user(&fx.db, "outsider").await;
    let elsewhere = seed_org(&fx.db).await;
    join_org(&fx.db, elsewhere, outsider.id, OrgRole::Owner).await;
    let outsider_cookie = session_of(&outsider).await;
    let (status, _) = in_session(&outsider_cookie, "GET", &uri, None).await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "another org's admin got {status}"
    );
    // And an account id from another org reads as none in theirs.
    let foreign = policies_uri(elsewhere, sa);
    let (status, _) = in_session(&outsider_cookie, "GET", &foreign, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_body_is_refused_for_what_the_contract_names() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let good = policy_body(json!([whole_org("member")]));

    // No environment, while one is required.
    for body in [
        without(good.clone(), "environment"),
        with(good.clone(), "environment", Value::Null),
        with(good.clone(), "environment", json!("  ")),
    ] {
        let (status, refused) = post(&fx, sa, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["code"], "environment_required");
    }

    // Grants: always explicit, never above the account's role, never `owner`.
    for grants in [
        Value::Null,
        json!([]),
        json!([whole_org("admin")]),
        json!([whole_org("owner")]),
        json!([{ "kind": "workspace", "workspace_id": null }]),
    ] {
        let body = if grants.is_null() {
            without(good.clone(), "grants")
        } else {
            with(good.clone(), "grants", grants.clone())
        };
        let (status, refused) = post(&fx, sa, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "grants {grants}: {refused}"
        );
    }

    // A workspace or an app of another org reads as none.
    let elsewhere = seed_org(&fx.db).await;
    let foreign_ws = seed_workspace_in(&fx.db, elsewhere).await;
    let foreign_app = published_app(&fx.db, elsewhere, foreign_ws).await;
    for grants in [
        json!([{ "kind": "workspace", "workspace_id": foreign_ws, "role_ceiling": "member" }]),
        json!([{ "kind": "app_publish", "app_id": foreign_app.id }]),
    ] {
        let (status, _) = post(&fx, sa, with(good.clone(), "grants", grants)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // Shapes that are simply wrong.
    for (key, value) in [
        ("repository", json!("acme")),
        ("repository", json!("acme/app/../../x")),
        ("workflow_path", json!("release.yml")),
        ("ref_pattern", json!("main")),
    ] {
        let (status, refused) = post(&fx, sa, with(good.clone(), key, value.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}={value}: {refused}");
    }
    assert!(policy_rows(&fx.db, sa).await.is_empty());

    // Clearing the environment later is refused the same way.
    let id = register_policy(&fx, sa, good).await;
    let one = format!("{}/{id}", policies_uri(fx.org_id, sa));
    let (status, refused) = in_session(
        &fx.cookie,
        "PATCH",
        &one,
        Some(json!({ "environment": null })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["code"], "environment_required");
    // And a policy cannot be moved to another repository.
    let (status, _) = in_session(
        &fx.cookie,
        "PATCH",
        &one,
        Some(json!({ "repository": "acme/other" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_repositorys_ids_are_resolved_and_the_bodys_are_the_fallback() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let grants = json!([whole_org("member")]);

    // GitHub answers: its ids win over whatever the body carried, and the
    // name shown is GitHub's own.
    let resolvable = with(
        policy_body(grants.clone()),
        "repository",
        json!("resolvable/repo"),
    );
    let (status, policy) = post(&fx, sa, resolvable).await;
    assert_eq!(status, StatusCode::CREATED, "{policy}");
    assert_eq!(policy["repository_id"], RESOLVED_REPO_ID);
    assert_eq!(policy["repository_owner_id"], RESOLVED_OWNER_ID);
    assert_eq!(policy["repository"], "Resolvable/repo");

    // GitHub does not answer: the body's ids are used.
    let (status, policy) = post(&fx, sa, policy_body(grants.clone())).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(policy["repository_id"], REPO_ID);
    assert_eq!(policy["repository"], "acme/app");

    // Neither: 422.
    let unresolved = without(
        without(policy_body(grants.clone()), "repository_id"),
        "repository_owner_id",
    );
    let (status, refused) = post(&fx, sa, unresolved).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused["code"], "repository_unresolved");
    // One id alone is a malformed body, not a fallback.
    let half = without(policy_body(grants), "repository_owner_id");
    let (status, _) = post(&fx, sa, half).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_app_publish_grant_is_stored_as_one() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "publisher", "member").await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let grants = json!([{ "kind": "app_publish", "app_id": app.id }]);
    let (status, policy) = post(&fx, sa, policy_body(grants)).await;
    assert_eq!(status, StatusCode::CREATED, "{policy}");
    let grant = &policy["grants"][0];
    assert_eq!(grant["kind"], "app_publish");
    assert_eq!(grant["app_id"], app.id.to_string());
    assert_eq!(grant["app_name"], app.name);
    assert_eq!(grant["role_ceiling"], Value::Null);
    let id = Uuid::parse_str(policy["id"].as_str().unwrap()).unwrap();
    let rows = grant_rows(&fx.db, id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].app_id, Some(app.id));
    assert_eq!(rows[0].org_id, fx.org_id);
}

#[tokio::test]
async fn deleting_the_account_takes_its_policies_with_it() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let id = register_policy(&fx, sa, policy_body(json!([whole_org("member")]))).await;
    let account_uri = format!("/orgs/{}/service-accounts/{sa}", fx.org_id);
    let (status, _) = in_session(&fx.cookie, "DELETE", &account_uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(policy_rows(&fx.db, sa).await.is_empty());
    assert!(grant_rows(&fx.db, id).await.is_empty());
}
