//! `GET /api/orgs` for a credential that acts as a service account (design
//! §3.3, §4.5): its own token, or the `ci` token a trust policy minted.
//!
//! An account holds no `org_members` row, so the list used to come back empty
//! and `oxyc --org <slug>` / `oxyc init-ci` — which resolve a slug from this
//! one route — answered "no organization with slug … is visible to you" about
//! the org the token was minted for. It lists the account's org, at the role
//! the account holds, capped by the token like any other.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::oidc::{Run, exchange_ok, policy_body, register_policy, trust_test_keys, whole_org};
use super::service_accounts::{
    admin_fixture, create_account, in_session, mint_account_token, session_of,
};
use super::stack::{get_as, get_in_session, ids};
use super::{Fixture, seed_org, seed_user, seed_workspace_in};

/// The one org a token lists, as `(id, slug, role)`.
async fn only_org(secret: &str) -> (String, String, String) {
    let (status, orgs) = get_as(secret, "/orgs").await;
    assert_eq!(status, StatusCode::OK, "{orgs}");
    let orgs = orgs.as_array().expect("a list of orgs");
    assert_eq!(orgs.len(), 1, "exactly the account's org: {orgs:?}");
    let field = |name: &str| orgs[0][name].as_str().expect(name).to_string();
    (field("id"), field("slug"), field("role"))
}

async fn slug_of(fx: &Fixture) -> String {
    let (_, orgs) = get_in_session(fx, "/orgs").await;
    let own = orgs
        .as_array()
        .expect("orgs")
        .iter()
        .find(|o| o["id"] == fx.org_id.to_string())
        .expect("the admin's own org");
    own["slug"].as_str().expect("a slug").to_string()
}

async fn account_token(fx: &Fixture, name: &str, role: &str, body: Value) -> String {
    let account = create_account(fx, name, role).await;
    mint_account_token(fx, account, body).await.1
}

#[tokio::test]
async fn a_service_account_token_lists_its_own_org_at_the_accounts_role() {
    let fx = admin_fixture().await;
    // Another org exists and is none of the account's business.
    seed_workspace_in(&fx.db, seed_org(&fx.db).await).await;
    let slug = slug_of(&fx).await;

    for role in ["admin", "member"] {
        let name = format!("deploy-{role}");
        let secret = account_token(&fx, &name, role, json!({ "name": "t" })).await;
        assert_eq!(
            only_org(&secret).await,
            (fx.org_id.to_string(), slug.clone(), role.to_string()),
            "a {role} account"
        );
    }

    // The ceiling caps it as it caps a person: an admin account's token
    // granted the org at `member` lists it as a member…
    let capped = json!({ "name": "t", "grants": [{ "role_ceiling": "member" }] });
    let secret = account_token(&fx, "capped-bot", "admin", capped).await;
    assert_eq!(only_org(&secret).await.2, "member");
    // …and one granted a single workspace reaches no org route, so it reads
    // as a member too. The org is still listed: that is what discovery is for.
    let one_workspace = json!({
        "name": "t",
        "grants": [{ "workspace_id": fx.workspace_id, "role_ceiling": "admin" }],
    });
    let secret = account_token(&fx, "workspace-bot", "admin", one_workspace).await;
    assert_eq!(
        only_org(&secret).await,
        (fx.org_id.to_string(), slug, "member".to_string())
    );
}

#[tokio::test]
async fn a_ci_token_lists_the_org_of_the_account_it_acts_as() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let slug = slug_of(&fx).await;
    let account = create_account(&fx, "deployer", "member").await;
    register_policy(&fx, account, policy_body(json!([whole_org("member")]))).await;

    let (_, secret) = exchange_ok(&fx, &Run::new()).await;
    assert_eq!(
        only_org(&secret).await,
        (fx.org_id.to_string(), slug, "member".to_string())
    );
}

#[tokio::test]
async fn a_person_lists_what_they_always_did() {
    // The account branch adds nothing for a person: a member's session lists
    // their memberships, and somebody in no org lists none.
    let fx = admin_fixture().await;
    let (_, orgs) = get_in_session(&fx, "/orgs").await;
    let listed: Vec<Uuid> = ids(&orgs);
    assert_eq!(listed, [fx.org_id]);
    assert_eq!(orgs[0]["role"], "owner");

    let stranger = seed_user(&fx.db, "stranger").await;
    let cookie = session_of(&stranger).await;
    let (status, orgs) = in_session(&cookie, "GET", "/orgs", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(orgs, json!([]));
}
