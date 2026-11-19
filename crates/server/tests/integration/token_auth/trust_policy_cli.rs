//! A **trust policy** approved in the browser for `oxyc init-ci`: the PKCE
//! exchange of `oxyc login`, carrying a `mint` of `kind: "trust_policy"` — and
//! the device-code twin. Nothing is minted: the exchange answers with the
//! service account and the policy it wrote.
//!
//! What these tests hold:
//!
//! - one approval by an org admin creates the account (when asked) and a
//!   policy granting exactly `app_publish` on the named app, and no token;
//! - an approval of what is already there writes nothing more;
//! - only someone who may manage the org's service accounts can approve, and
//!   a named account that is missing is refused rather than created;
//! - a token still cannot create an account or a policy, directly or by
//!   approving one;
//! - the repository's ids are GitHub's, never the asker's: a repository the
//!   deployment cannot look up is refused, and nothing is written.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use entity::{api_tokens, oidc_trust_policies, oidc_trust_policy_grants, service_accounts};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::agent_token::{VERIFIER, authorize, exchange};
use super::device_login::{approve as approve_request, poll_with, started};
use super::oidc::{OWNER_ID, REPO_ID, RESOLVED_OWNER_ID, RESOLVED_REPO_ID, trust_test_keys};
use super::sandbox_agent::{another_session, assert_refused};
use super::service_accounts::{admin_fixture, create_account, in_session, with_token};
use super::stack::{join_org, published_app};
use super::{Fixture, audit_rows, minted_pat};

const HOST: &str = "dev-laptop";
const WORKFLOW: &str = ".github/workflows/oxy-publish.yml";

/// An approval for `deployer` publishing `app` from `resolvable/app`, with
/// `extra` over it. The stubbed GitHub lookup knows only `resolvable/*`, and
/// answers it as `Resolvable/*`.
fn policy_mint(fx: &Fixture, app: Uuid, extra: Value) -> Value {
    let mut mint = json!({
        "kind": "trust_policy",
        "org": fx.org_id.to_string(),
        "account": "deployer",
        "create_account": true,
        "app": app.to_string(),
        "repository": "resolvable/app",
        "workflow_path": WORKFLOW,
        "environment": "oxy-publish",
    });
    if let (Value::Object(mint), Value::Object(extra)) = (&mut mint, extra) {
        mint.extend(extra);
    }
    mint
}

/// Approve under `cookie` and redeem the code: the exchange's body.
async fn registered(cookie: &str, mint: Value) -> Value {
    let (status, approved) = authorize(cookie, HOST, Some(mint)).await;
    assert_eq!(status, StatusCode::OK, "authorize: {approved}");
    let (status, body) = exchange(approved["code"].as_str().expect("a code"), VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "exchange: {body}");
    body
}

async fn app_of(fx: &Fixture) -> Uuid {
    published_app(&fx.db, fx.org_id, fx.workspace_id).await.id
}

async fn accounts_named(
    db: &DatabaseConnection,
    org_id: Uuid,
    name: &str,
) -> Vec<service_accounts::Model> {
    service_accounts::Entity::find()
        .filter(service_accounts::Column::OrgId.eq(org_id))
        .filter(service_accounts::Column::Name.eq(name))
        .all(db)
        .await
        .unwrap()
}

async fn policies_of(db: &DatabaseConnection, sa_id: Uuid) -> Vec<oidc_trust_policies::Model> {
    oidc_trust_policies::Entity::find()
        .filter(oidc_trust_policies::Column::ServiceAccountId.eq(sa_id))
        .all(db)
        .await
        .unwrap()
}

async fn tokens_of(db: &DatabaseConnection, user_id: Uuid) -> u64 {
    api_tokens::Entity::find()
        .filter(api_tokens::Column::PrincipalUserId.eq(user_id))
        .count(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn one_approval_creates_the_account_and_a_policy_publishing_one_app_and_mints_nothing() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;
    let body = registered(&fx.cookie, policy_mint(&fx, app, json!({}))).await;

    // No credential in the answer: the account and the policy, and what was made.
    assert!(
        body.get("secret").is_none() && body.get("token").is_none(),
        "{body}"
    );
    let registration = &body["registration"];
    assert_eq!(registration["created_account"], true);
    assert_eq!(registration["created_policy"], true);
    assert_eq!(registration["service_account"]["name"], "deployer");
    assert_eq!(registration["service_account"]["org_role"], "member");
    let policy = &registration["trust_policy"];
    // GitHub's name and ids for it, not the asker's.
    assert_eq!(policy["repository"], "Resolvable/app");
    assert_eq!(policy["repository_id"], RESOLVED_REPO_ID);
    assert_eq!(policy["repository_owner_id"], RESOLVED_OWNER_ID);
    assert_eq!(policy["workflow_path"], WORKFLOW);
    assert_eq!(policy["environment"], "oxy-publish");
    assert_eq!(policy["ref_pattern"], Value::Null);
    assert_eq!(policy["allow_self_hosted"], false);

    // The rows: one member account, one policy, one grant — publishing the app.
    let accounts = accounts_named(&fx.db, fx.org_id, "deployer").await;
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].org_role, "member");
    let policies = policies_of(&fx.db, accounts[0].user_id).await;
    assert_eq!(policies.len(), 1);
    let grants = oidc_trust_policy_grants::Entity::find()
        .filter(oidc_trust_policy_grants::Column::PolicyId.eq(policies[0].id))
        .all(&fx.db)
        .await
        .unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].kind, "app_publish");
    assert_eq!(grants[0].app_id, Some(app));
    assert_eq!(policies[0].created_by, Some(fx.user.id));
    // And no token for the account, or for anyone.
    assert_eq!(tokens_of(&fx.db, accounts[0].user_id).await, 0);

    // The web flow's audit rows, as the approver.
    let created = audit_rows(&fx.db, "service_account.created").await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].actor_user_id, Some(fx.user.id));
    let policy_rows = audit_rows(&fx.db, "trust_policy.created").await;
    assert_eq!(policy_rows.len(), 1);
    assert_eq!(policy_rows[0].actor_user_id, Some(fx.user.id));
    assert!(audit_rows(&fx.db, "token.created").await.is_empty());
}

#[tokio::test]
async fn an_app_named_by_slug_is_resolved_in_the_org_and_named_in_the_answer() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    // The asker may not list the org's apps; the slug is what it knows.
    let body = registered(
        &fx.cookie,
        policy_mint(&fx, app.id, json!({ "app": app.slug })),
    )
    .await;
    assert_eq!(body["registration"]["app"]["id"], json!(app.id));
    assert_eq!(body["registration"]["app"]["slug"], app.slug.as_str());
    let grants = body["registration"]["trust_policy"]["grants"]
        .as_array()
        .unwrap();
    assert_eq!(grants[0]["app_id"], json!(app.id));

    // A slug the org has no app of is refused, named as asked.
    let (status, refused) = authorize(
        &fx.cookie,
        HOST,
        Some(policy_mint(&fx, app.id, json!({ "app": "no-such-app" }))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["code"], "app_not_found");
    assert_eq!(refused["app_id"], "no-such-app");
}

#[tokio::test]
async fn approving_what_is_already_there_writes_nothing_more() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;
    let first = registered(&fx.cookie, policy_mint(&fx, app, json!({}))).await;
    let second = registered(&fx.cookie, policy_mint(&fx, app, json!({}))).await;
    assert_eq!(second["registration"]["created_account"], false);
    assert_eq!(second["registration"]["created_policy"], false);
    assert_eq!(
        second["registration"]["trust_policy"]["id"],
        first["registration"]["trust_policy"]["id"]
    );
    let accounts = accounts_named(&fx.db, fx.org_id, "deployer").await;
    assert_eq!(policies_of(&fx.db, accounts[0].user_id).await.len(), 1);

    // Another app of the org is another policy on the same account.
    let other = app_of(&fx).await;
    let third = registered(&fx.cookie, policy_mint(&fx, other, json!({}))).await;
    assert_eq!(third["registration"]["created_account"], false);
    assert_eq!(third["registration"]["created_policy"], true);
    assert_eq!(policies_of(&fx.db, accounts[0].user_id).await.len(), 2);
}

#[tokio::test]
async fn an_existing_account_keeps_its_role_and_a_named_missing_one_is_never_created() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;

    // Asked not to create, with no account of the name: refused, nothing made.
    let named = policy_mint(
        &fx,
        app,
        json!({ "account": "release-bot", "create_account": false }),
    );
    let (status, body) = authorize(&fx.cookie, HOST, Some(named)).await;
    assert_refused(
        (status, body.clone()),
        StatusCode::NOT_FOUND,
        "service_account_not_found",
        "a named account that is missing",
    );
    assert_eq!(body["account"], "release-bot");
    assert!(
        accounts_named(&fx.db, fx.org_id, "release-bot")
            .await
            .is_empty()
    );

    // An admin account is reused as an admin: publishing needs no change to it.
    let sa = create_account(&fx, "release-bot", "admin").await;
    let body = registered(
        &fx.cookie,
        policy_mint(
            &fx,
            app,
            json!({ "account": "release-bot", "create_account": false }),
        ),
    )
    .await;
    assert_eq!(body["registration"]["service_account"]["id"], json!(sa));
    assert_eq!(body["registration"]["service_account"]["org_role"], "admin");
    assert_eq!(body["registration"]["created_account"], false);

    // A disabled one is refused: what it is given authenticates nothing.
    let uri = format!("/orgs/{}/service-accounts/{sa}", fx.org_id);
    let (status, _) =
        in_session(&fx.cookie, "PATCH", &uri, Some(json!({ "disabled": true }))).await;
    assert_eq!(status, StatusCode::OK);
    let mint = policy_mint(
        &fx,
        app,
        json!({ "account": "release-bot", "create_account": false }),
    );
    assert_refused(
        authorize(&fx.cookie, HOST, Some(mint)).await,
        StatusCode::CONFLICT,
        "service_account_disabled",
        "a disabled account",
    );
}

#[tokio::test]
async fn only_an_owner_or_admin_of_the_org_may_approve_and_the_app_must_be_its_own() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;
    let mint = policy_mint(&fx, app, json!({}));

    let (member, member_cookie) = another_session(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    assert_refused(
        authorize(&member_cookie, HOST, Some(mint.clone())).await,
        StatusCode::FORBIDDEN,
        "org_admin_required",
        "a plain member",
    );
    let (_, outsider) = another_session(&fx.db, "outsider").await;
    assert_refused(
        authorize(&outsider, HOST, Some(mint.clone())).await,
        StatusCode::NOT_FOUND,
        "org_not_found",
        "a non-member",
    );
    // An app of no org of the approver's — here, a made-up id — reads as none.
    let stray = policy_mint(&fx, Uuid::new_v4(), json!({}));
    let (status, _) = authorize(&fx.cookie, HOST, Some(stray)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A run the trust-policy rules refuse is refused at the approval, with why.
    let bad = policy_mint(&fx, app, json!({ "workflow_path": "release.yml" }));
    assert_refused(
        authorize(&fx.cookie, HOST, Some(bad)).await,
        StatusCode::BAD_REQUEST,
        "invalid_trust_policy",
        "a workflow outside .github/workflows",
    );
    assert!(
        accounts_named(&fx.db, fx.org_id, "deployer")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn the_approval_writes_the_rows_and_the_exchange_only_reports_them() {
    use entity::org_members;
    use sea_orm::sea_query::Expr;
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;
    let (status, approved) =
        authorize(&fx.cookie, HOST, Some(policy_mint(&fx, app, json!({})))).await;
    assert_eq!(status, StatusCode::OK, "{approved}");

    // Written at the approval, under the session: an approval nobody collects
    // is not lost.
    let accounts = accounts_named(&fx.db, fx.org_id, "deployer").await;
    assert_eq!(accounts.len(), 1, "the account exists before any exchange");
    assert_eq!(policies_of(&fx.db, accounts[0].user_id).await.len(), 1);

    // Losing the role afterwards changes nothing already written; collecting
    // only reports it, once.
    org_members::Entity::update_many()
        .col_expr(org_members::Column::Role, Expr::value(OrgRole::Member))
        .filter(org_members::Column::OrgId.eq(fx.org_id))
        .filter(org_members::Column::UserId.eq(fx.user.id))
        .exec(&fx.db)
        .await
        .unwrap();
    let code = approved["code"].as_str().unwrap();
    let (status, body) = exchange(code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["registration"]["created_policy"], true);
    assert_eq!(policies_of(&fx.db, accounts[0].user_id).await.len(), 1);
    assert_refused(
        exchange(code, VERIFIER).await,
        StatusCode::BAD_REQUEST,
        "invalid_code",
        "a code collected twice",
    );
}

#[tokio::test]
async fn a_token_cannot_create_an_account_or_a_policy_or_approve_one() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;
    let sa = create_account(&fx, "deployer", "member").await;
    let (_, owner_token) = minted_pat(&fx, None).await;

    let accounts = format!("/orgs/{}/service-accounts", fx.org_id);
    let policies = format!("{accounts}/{sa}/trust-policies");
    assert_refused(
        with_token(
            &owner_token,
            "POST",
            &accounts,
            Some(json!({ "name": "next-bot" })),
        )
        .await,
        StatusCode::FORBIDDEN,
        "session_required",
        "a token creating an account",
    );
    let body = json!({
        "repository": "acme/app",
        "workflow_path": WORKFLOW,
        "environment": "oxy-publish",
        "repository_id": REPO_ID,
        "repository_owner_id": OWNER_ID,
        "grants": [{ "kind": "app_publish", "app_id": app }],
    });
    assert_refused(
        with_token(&owner_token, "POST", &policies, Some(body)).await,
        StatusCode::FORBIDDEN,
        "session_required",
        "a token creating a policy",
    );
    let approve = json!({
        "code_challenge": oxy_auth::token::cli_login::challenge_of(VERIFIER),
        "hostname": HOST,
        "mint": policy_mint(&fx, app, json!({})),
    });
    assert_refused(
        with_token(&owner_token, "POST", "/auth/cli/authorize", Some(approve)).await,
        StatusCode::FORBIDDEN,
        "session_required",
        "a token approving a trust policy",
    );
    let preview = json!({ "mint": policy_mint(&fx, app, json!({})) });
    assert_refused(
        with_token(
            &owner_token,
            "POST",
            "/auth/cli/trust-policy/preview",
            Some(preview),
        )
        .await,
        StatusCode::FORBIDDEN,
        "session_required",
        "a token previewing a trust policy",
    );
    assert!(policies_of(&fx.db, sa).await.is_empty());
}

#[tokio::test]
async fn the_preview_is_what_approving_would_write_and_writes_nothing() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;
    let (status, preview) = in_session(
        &fx.cookie,
        "POST",
        "/auth/cli/trust-policy/preview",
        Some(json!({ "mint": policy_mint(&fx, app, json!({})) })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["org"]["id"], json!(fx.org_id));
    assert_eq!(
        preview["account"],
        json!({ "name": "deployer", "role": "member", "exists": false })
    );
    assert_eq!(preview["app"]["id"], json!(app));
    assert_eq!(preview["policy"]["repository"], "Resolvable/app");
    assert_eq!(preview["policy"]["workflow_path"], WORKFLOW);
    assert_eq!(preview["policy"]["exists"], false);
    assert!(
        accounts_named(&fx.db, fx.org_id, "deployer")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_device_code_request_registers_the_same_and_records_the_approver() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;
    let ask = json!({
        "kind": "trust_policy",
        "org": fx.org_id.to_string(),
        "account": "deployer",
        "create": "1",
        "app": app.to_string(),
        "repository": "resolvable/app",
        "workflow": WORKFLOW,
        "environment": "oxy-publish",
    });
    let (device_code, user_code) = started(Some(ask)).await;

    // The approval must be of the kind asked for: never a token for this request.
    let agent = json!({ "kind": "agent", "expires_in_hours": 2 });
    let (status, _) = approve_request(&fx.cookie, &user_code, Some(agent)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = approve_request(
        &fx.cookie,
        &user_code,
        Some(policy_mint(&fx, app, json!({}))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Already written: the poll only reports it.
    assert_eq!(accounts_named(&fx.db, fx.org_id, "deployer").await.len(), 1);
    let (status, polled) = poll_with(&device_code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(polled["status"], "approved");
    assert!(polled.get("secret").is_none(), "{polled}");
    assert_eq!(polled["registration"]["created_policy"], true);
    let accounts = accounts_named(&fx.db, fx.org_id, "deployer").await;
    assert_eq!(policies_of(&fx.db, accounts[0].user_id).await.len(), 1);
}

/// A run is matched on the repository's ids, and the approver is shown only
/// its name: ids the asker chose would decide which repository is trusted with
/// nobody having checked them. So they are refused outright, and a repository
/// GitHub will not show the deployment is refused at the preview and at the
/// approval alike — the page never arms, and nothing is written.
#[tokio::test]
async fn the_repository_s_ids_are_github_s_and_never_the_asker_s() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = app_of(&fx).await;

    let with_ids = policy_mint(
        &fx,
        app,
        json!({ "repository_id": REPO_ID, "repository_owner_id": OWNER_ID }),
    );
    assert_refused(
        authorize(&fx.cookie, HOST, Some(with_ids)).await,
        StatusCode::BAD_REQUEST,
        "invalid_trust_policy",
        "ids sent with the approval",
    );

    let unresolvable = policy_mint(&fx, app, json!({ "repository": "acme/private" }));
    let (status, preview) = in_session(
        &fx.cookie,
        "POST",
        "/auth/cli/trust-policy/preview",
        Some(json!({ "mint": unresolvable.clone() })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{preview}");
    assert_eq!(preview["code"], "repository_unresolved");
    assert!(
        preview["error"]
            .as_str()
            .is_some_and(|why| why.contains("acme/private")),
        "{preview}"
    );
    assert_refused(
        authorize(&fx.cookie, HOST, Some(unresolvable.clone())).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "repository_unresolved",
        "a repository GitHub would not show",
    );

    // The device-code twin: refused under the session, and the CLI told where
    // an admin adds it instead.
    let ask = json!({
        "kind": "trust_policy",
        "org": fx.org_id.to_string(),
        "account": "deployer",
        "create": "1",
        "app": app.to_string(),
        "repository": "acme/private",
        "workflow": WORKFLOW,
        "environment": "oxy-publish",
    });
    let (device_code, user_code) = started(Some(ask)).await;
    let (status, _) = approve_request(&fx.cookie, &user_code, Some(unresolvable)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let denied = poll_with(&device_code, VERIFIER).await;
    let reason = denied.1["error"].as_str().unwrap_or_default().to_string();
    assert!(reason.contains("Trusted access"), "{reason}");
    assert_refused(
        denied,
        StatusCode::BAD_REQUEST,
        "access_denied",
        "an unresolvable repository",
    );

    assert!(
        accounts_named(&fx.db, fx.org_id, "deployer")
            .await
            .is_empty()
    );
}
