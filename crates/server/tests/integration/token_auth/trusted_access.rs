//! Phase 4: trusted access (design §3.4). A GitHub Actions run trades its OIDC
//! token at `POST /auth/oidc/exchange` for a 15-minute `oxy_ci_` token, and
//! that token is then used against the served stacks.
//!
//! The tokens are real JWTs, signed RS256 with a throwaway key the process is
//! told to trust (`super::oidc`), so the whole verifier runs: signature,
//! issuer, audience, expiry, the `jti` burn, and the claim match.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::org_members::OrgRole;
use entity::{api_tokens, oidc_trust_policies, organizations};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::oidc::{
    ACCOUNT, OWNER_ID, REPO_ID, Run, account_name, exchange, exchange_as, exchange_ok,
    exchange_with, policies_uri, policy_body, register_policy, trust_test_keys, whole_org,
};
use super::service_accounts::{
    admin_fixture, create_account, in_session, with_token, workspace_status,
};
use super::stack::{flat_api, join_org};
use super::{Fixture, audit_rows, call, seed_org, seed_workspace_in};

pub(crate) async fn token_row(db: &DatabaseConnection, id: Uuid) -> api_tokens::Model {
    api_tokens::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("the token row")
}

pub(crate) async fn policy_row(db: &DatabaseConnection, id: Uuid) -> oidc_trust_policies::Model {
    oidc_trust_policies::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("the policy row")
}

fn workspace_grant(workspace: Uuid, ceiling: &str) -> Value {
    json!({ "kind": "workspace", "workspace_id": workspace, "role_ceiling": ceiling })
}

/// An admin's org with one account and one policy granting `grants`.
pub(crate) async fn deployer(grants: Value) -> (Fixture, Uuid, Uuid) {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let policy = register_policy(&fx, sa, policy_body(grants)).await;
    (fx, sa, policy)
}

async fn refused(fx: &Fixture, run: &Run, status: StatusCode, code: &str) {
    let (got, body) = exchange(fx, run).await;
    assert_eq!(got, status, "{code}: {body}");
    assert_eq!(body["code"], code, "{body}");
    assert!(body.get("token").is_none(), "a refusal mints nothing");
}

#[tokio::test]
async fn a_matching_run_gets_a_scoped_fifteen_minute_token() {
    let fx = admin_fixture().await;
    trust_test_keys();
    let granted = fx.workspace_id;
    let sibling = seed_workspace_in(&fx.db, fx.org_id).await;
    let other_org = seed_org(&fx.db).await;
    let foreign = seed_workspace_in(&fx.db, other_org).await;
    let sa = create_account(&fx, "deployer", "member").await;
    let body = policy_body(json!([workspace_grant(granted, "member")]));
    let policy = register_policy(&fx, sa, body).await;

    let before = Utc::now();
    let (status, minted) = exchange(&fx, &Run::new()).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let secret = minted["token"].as_str().unwrap();
    assert!(secret.starts_with("oxy_ci_"), "{secret}");
    let token_id = Uuid::parse_str(minted["token_id"].as_str().unwrap()).unwrap();
    let org_slug = format!("acme-{}", fx.org_id.simple());
    assert_eq!(minted["service_account"], format!("{org_slug}/deployer"));
    let expires: DateTime<Utc> = minted["expires_at"].as_str().unwrap().parse().unwrap();
    let lifetime = expires - before;
    assert!(
        lifetime > Duration::minutes(14) && lifetime <= Duration::minutes(16),
        "the token lasts 15 minutes, not {lifetime}"
    );
    let grants = minted["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["workspace_id"], granted.to_string());
    assert_eq!(grants[0]["role_ceiling"], "member");

    // Inside the grant it works, at the ceiling; outside it — a sibling
    // workspace, another org — the answer is 404, never 403.
    assert_eq!(
        workspace_status(secret, granted, "GET", "read").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        workspace_status(secret, granted, "POST", "write").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        workspace_status(secret, granted, "POST", "manage").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        workspace_status(secret, sibling, "GET", "read").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        workspace_status(secret, foreign, "GET", "read").await,
        StatusCode::NOT_FOUND
    );
    let (status, _) = with_token(secret, "GET", &format!("/orgs/{other_org}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A token cannot manage policies, tokens or accounts — its own included.
    let (status, _) = with_token(secret, "GET", &policies_uri(fx.org_id, sa), None).await;
    assert_ne!(status, StatusCode::OK);
    let (status, _) = with_token(secret, "GET", "/user/tokens", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The token, about itself.
    let (status, me) = with_token(secret, "GET", "/auth/token", None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["kind"], "ci");
    assert_eq!(me["source"], "oidc");
    assert_eq!(me["all_access"], false);
    assert_eq!(me["platform"], false);
    assert_eq!(me["owner"]["type"], "service_account");
    assert_eq!(me["owner"]["id"], sa.to_string());

    // The row records the policy and the run — and not the JWT.
    let row = token_row(&fx.db, token_id).await;
    assert_eq!(row.kind, "ci");
    assert_eq!(row.principal_user_id, sa);
    assert_eq!(row.trust_policy_id, Some(policy));
    assert_eq!(row.created_by, None, "no person minted it");
    let claims = row.oidc_claims.expect("the verified claims");
    for (claim, want) in [
        ("run_id", "7001"),
        ("run_attempt", "1"),
        ("sha", "0123abcd0123abcd"),
        ("ref", "refs/heads/main"),
        ("environment", "production"),
        (
            "job_workflow_ref",
            "acme/app/.github/workflows/release.yml@refs/heads/main",
        ),
        ("actor_id", "5005"),
        ("repository_id", "987"),
    ] {
        assert_eq!(claims[claim], want, "{claim}");
    }
    assert!(claims["jti"].is_string());
    assert!(
        !claims.to_string().contains("eyJ"),
        "the JWT is never stored"
    );

    // The policy was stamped, and the exchange is in the org's chain.
    assert!(policy_row(&fx.db, policy).await.last_used_at.is_some());
    let events = audit_rows(&fx.db, "oidc.token_exchanged").await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.org_id, Some(fx.org_id));
    assert_eq!(event.actor_user_id, Some(sa));
    assert_eq!(
        event.target_id.as_deref(),
        Some(token_id.to_string().as_str())
    );
    assert_eq!(event.metadata["trust_policy_id"], policy.to_string());
    assert_eq!(event.metadata["claims"]["run_id"], "7001");
    let recorded = event.metadata.to_string();
    assert!(!recorded.contains("eyJ") && !recorded.contains(secret));
}

#[tokio::test]
async fn a_token_is_single_use_and_must_be_fresh_and_for_this_audience() {
    let (fx, sa, _policy) = deployer(json!([whole_org("member")])).await;

    // The same JWT twice: the second is a replay.
    let run = Run::new().with("jti", json!("one-and-only"));
    let (status, _) = exchange(&fx, &run).await;
    assert_eq!(status, StatusCode::OK);
    refused(&fx, &run, StatusCode::UNAUTHORIZED, "replayed").await;

    // Expired, well past the leeway.
    let expired = Run::new().with("exp", json!(Utc::now().timestamp() - 300));
    refused(&fx, &expired, StatusCode::UNAUTHORIZED, "expired").await;

    // Minted for the publish exchange, or for nobody in particular.
    let named = sa.to_string();
    for audience in ["oxy-publish", "https://github.com/acme"] {
        let body = json!({ "token": Run::new().jwt(audience), "service_account": named });
        let (status, refusal) = exchange_with(body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{audience}");
        assert_eq!(refusal["code"], "wrong_audience", "{audience}");
        // The refusal names the audience to ask for instead.
        assert_eq!(refusal["audience"], "oxy", "{audience}");
    }

    // Not a token GitHub signed.
    for junk in ["not.a.jwt", "eyJhbGciOiJub25lIn0.e30."] {
        let body = json!({ "token": junk, "service_account": named });
        let (status, refusal) = exchange_with(body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{junk}");
        assert_eq!(refusal["code"], "invalid_token", "{junk}");
    }
    let tampered = {
        let good = Run::new().jwt("oxy");
        let other = Run::new().with("repository_id", json!("1")).jwt("oxy");
        let (mut parts, forged): (Vec<&str>, Vec<&str>) =
            (good.split('.').collect(), other.split('.').collect());
        parts[1] = forged[1];
        parts.join(".")
    };
    let body = json!({ "token": tampered, "service_account": named });
    let (status, refusal) = exchange_with(body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refusal["code"], "invalid_token");

    // A body that names no token is a plain 400.
    let (status, refusal) = exchange_with(json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refusal.get("code").is_none());

    // Each refusal is audited, unchained, with its reason and never the JWT.
    let rejected = audit_rows(&fx.db, "oidc.exchange_rejected").await;
    let reasons: Vec<&str> = rejected
        .iter()
        .map(|r| r.metadata["reason"].as_str().unwrap())
        .collect();
    for reason in ["replayed", "expired", "wrong_audience", "invalid_token"] {
        assert!(
            reasons.contains(&reason),
            "{reason} was audited: {reasons:?}"
        );
    }
    for row in &rejected {
        assert_eq!(row.org_id, None, "a refusal joins no org's chain");
        assert_eq!(row.outcome, "failure");
        assert!(!row.metadata.to_string().contains("eyJ"));
    }
    // Only one token was ever minted.
    assert_eq!(audit_rows(&fx.db, "oidc.token_exchanged").await.len(), 1);
}

/// The audience this deployment says it takes, to a run that asked for
/// another: the `audience` of a `wrong_audience` refusal.
async fn audience_it_takes(sa: Uuid) -> Value {
    let token = Run::new().jwt("https://github.com/acme");
    let body = json!({ "token": token, "service_account": sa });
    let (status, refusal) = exchange_with(body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refusal}");
    assert_eq!(refusal["code"], "wrong_audience", "{refusal}");
    refusal["audience"].clone()
}

#[tokio::test]
async fn a_token_asked_for_another_deployment_is_refused_here() {
    let (fx, sa, _policy) = deployer(json!([whole_org("member")])).await;

    // A deployment with no public URL — a dev box — has nothing to tell
    // itself apart by: it takes the plain audience, which no client asks for.
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var("OXY_API_URL") };
    assert_eq!(audience_it_takes(sa).await, "oxy");

    // This one is production. Its audience is its own: the host of the same
    // public URL its mails link through — and so the audience a client that
    // was pointed at that address derives for itself. A path on the URL, and
    // the scheme's default port, are no part of it.
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_API_URL", "https://App.Oxy.Example:443/api") };
    let ours = "oxy:app.oxy.example";
    assert_eq!(audience_it_takes(sa).await, ours);

    // There is no route that tells a client what to ask for: one that did
    // could name another deployment's audience and be handed its token.
    let (status, _) = call(flat_api(), "GET", "/auth/oidc/audience", &[], None).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "the audience is derived, never served"
    );

    // A token asked for staging's audience is good at staging — and spent
    // tokens are remembered per database, so nothing else here would stop it.
    // The audience does: it is refused, as is the plain one every deployment
    // once shared, and the publish exchange's. The refusal says what to ask for.
    for theirs in ["oxy:app-staging.oxy.example", "oxy", "oxy-publish"] {
        let body = json!({ "token": Run::new().jwt(theirs), "service_account": sa });
        let (status, refusal) = exchange_with(body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{theirs}: {refusal}");
        assert_eq!(refusal["code"], "wrong_audience", "{theirs}");
        assert_eq!(refusal["audience"], ours, "{theirs}");
        assert!(refusal.get("token").is_none(), "{theirs}");
    }
    assert!(ci_tokens(&fx.db).await.is_empty(), "nothing was minted");

    // Asked for this deployment's, the same run mints.
    let body = json!({ "token": Run::new().jwt(ours), "service_account": sa });
    let (status, minted) = exchange_with(body).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(ci_tokens(&fx.db).await.len(), 1);
}

#[tokio::test]
async fn a_run_that_matches_no_policy_is_refused_with_the_reason() {
    let (fx, sa, _policy) = deployer(json!([whole_org("member")])).await;

    refused(
        &fx,
        &Run::new().with("event_name", json!("pull_request_target")),
        StatusCode::FORBIDDEN,
        "pull_request_target",
    )
    .await;
    refused(
        &fx,
        &Run::new().with("runner_environment", json!("self-hosted")),
        StatusCode::FORBIDDEN,
        "self_hosted_runner",
    )
    .await;
    refused(
        &fx,
        &Run::new().with("environment", Value::Null),
        StatusCode::FORBIDDEN,
        "missing_environment",
    )
    .await;
    for (claim, value) in [
        // The same names under a re-registered owner, or a recreated repo.
        ("repository_owner_id", json!((OWNER_ID + 1).to_string())),
        ("repository_id", json!((REPO_ID + 1).to_string())),
        ("environment", json!("staging")),
        (
            "job_workflow_ref",
            json!("acme/app/.github/workflows/ci.yml@refs/heads/main"),
        ),
    ] {
        refused(
            &fx,
            &Run::new().with(claim, value),
            StatusCode::FORBIDDEN,
            "no_matching_policy",
        )
        .await;
    }

    // A policy that allows self-hosted runners, on a tag pattern.
    let body = json!({
        "repository": "acme/app",
        "workflow_path": ".github/workflows/release.yml",
        "environment": "production",
        "ref_pattern": "refs/tags/v*",
        "allow_self_hosted": true,
        "grants": [whole_org("member")],
        "repository_id": REPO_ID,
        "repository_owner_id": OWNER_ID,
    });
    register_policy(&fx, sa, body).await;
    let tagged = Run::new()
        .with("runner_environment", json!("self-hosted"))
        .with("ref", json!("refs/tags/v1.2.0"));
    exchange_ok(&fx, &tagged).await;
    // Self-hosted on a branch still matches neither policy.
    refused(
        &fx,
        &Run::new().with("runner_environment", json!("self-hosted")),
        StatusCode::FORBIDDEN,
        "self_hosted_runner",
    )
    .await;
}

#[tokio::test]
async fn a_run_acts_as_the_account_it_names_and_no_other() {
    let (fx, _sa, _policy) = deployer(json!([whole_org("member")])).await;
    let auditor = create_account(&fx, "auditor", "member").await;
    register_policy(&fx, auditor, policy_body(json!([whole_org("viewer")]))).await;

    // Two accounts of the org trust this run. That is not a conflict: the run
    // says which it acts as, and gets that account's policy and grants alone.
    let (status, minted) = exchange(&fx, &Run::new()).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(
        minted["service_account"],
        account_name(fx.org_id, "deployer")
    );
    assert_eq!(minted["grants"][0]["role_ceiling"], "member");

    let (status, minted) = exchange_as(&fx, "auditor", &Run::new()).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(
        minted["service_account"],
        account_name(fx.org_id, "auditor")
    );
    assert_eq!(minted["grants"][0]["role_ceiling"], "viewer");
    let secret = minted["token"].as_str().unwrap();
    assert_eq!(
        workspace_status(secret, fx.workspace_id, "POST", "write").await,
        StatusCode::FORBIDDEN,
        "the auditor's token reads and does not write"
    );

    // An account with no policy on this repository mints nothing — and an id
    // that names no account at all is answered word for word the same, so the
    // route cannot be used to learn which ids exist.
    let idle = create_account(&fx, "idle", "member").await;
    let mut answers = Vec::new();
    for id in [idle, Uuid::new_v4()] {
        let body = json!({ "token": Run::new().jwt("oxy"), "service_account": id });
        let (status, refusal) = exchange_with(body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refusal}");
        assert_eq!(refusal["code"], "no_matching_policy");
        answers.push(refusal);
    }
    assert_eq!(answers[0], answers[1], "an unknown id reads as no policy");
}

#[tokio::test]
async fn a_run_that_names_no_account_is_refused_and_its_token_is_not_spent() {
    let (fx, _sa, _policy) = deployer(json!([whole_org("member")])).await;

    // The policy matches this run exactly. Unnamed, it still mints nothing:
    // the exchange never goes looking for an account on a run's behalf. And
    // the account's readable name is not a name for it here — only its id is.
    let run = Run::new().with("jti", json!("kept-for-later"));
    for body in [
        json!({ "token": run.jwt("oxy") }),
        json!({ "token": run.jwt("oxy"), "service_account": null }),
        json!({ "token": run.jwt("oxy"), "service_account": "" }),
        json!({ "token": run.jwt("oxy"), "service_account": "deployer" }),
        json!({ "token": run.jwt("oxy"), "service_account": account_name(fx.org_id, ACCOUNT) }),
    ] {
        let (status, refusal) = exchange_with(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refusal}");
        assert_eq!(refusal["code"], "service_account_required", "{refusal}");
        assert!(refusal.get("token").is_none(), "{refusal}");
        assert!(refusal.get("candidates").is_none(), "it lists no accounts");
        let said = refusal["error"].as_str().unwrap_or_default();
        assert!(said.contains("account's id"), "it asks for the id: {said}");
    }
    assert!(
        audit_rows(&fx.db, "oidc.token_exchanged").await.is_empty(),
        "nothing was minted"
    );
    // Refused before the token was read: the same token, named, still works.
    exchange_ok(&fx, &run).await;
}

/// A service account in `org`, with a policy on the fixture run's repository
/// granting the whole org. `fx`'s user must own `org`.
async fn account_with_policy_in(fx: &Fixture, org: Uuid, name: &str) -> Uuid {
    let body = json!({ "name": name, "org_role": "member" });
    let uri = format!("/orgs/{org}/service-accounts");
    let (status, account) = in_session(&fx.cookie, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "create {name}: {account}");
    let sa = Uuid::parse_str(account["id"].as_str().expect("an id")).expect("a uuid");
    let body = policy_body(json!([whole_org("member")]));
    let (status, policy) = in_session(&fx.cookie, "POST", &policies_uri(org, sa), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "register a policy: {policy}");
    sa
}

pub(crate) async fn ci_tokens(db: &DatabaseConnection) -> Vec<api_tokens::Model> {
    api_tokens::Entity::find()
        .filter(api_tokens::Column::Kind.eq("ci"))
        .all(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn another_orgs_policy_on_the_same_repository_has_no_effect() {
    trust_test_keys();
    // Org A owns the repository's workflow. Org B is somebody else: nothing
    // ties a repository to an org, so B registers a policy on A's repository
    // — same numeric ids, same workflow, same environment.
    let fx = admin_fixture().await;
    let org_b = seed_org(&fx.db).await;
    join_org(&fx.db, org_b, fx.user.id, OrgRole::Owner).await;
    let theirs = account_with_policy_in(&fx, org_b, "deployer").await;

    // A has no trust policy yet (it publishes the older way). Its run names
    // no account, so B's policy — the only one that matches — mints nothing
    // for it: A's job is never handed a token that acts inside org B.
    let (status, refusal) = exchange_with(json!({ "token": Run::new().jwt("oxy") })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refusal}");
    assert_eq!(refusal["code"], "service_account_required");
    assert!(ci_tokens(&fx.db).await.is_empty(), "nothing was minted");

    // A's run names A's own account, which has no policy: refused on A's
    // account alone. B's matching policy is not read, so it is not a fallback.
    let ours = create_account(&fx, "deployer", "member").await;
    refused(
        &fx,
        &Run::new(),
        StatusCode::FORBIDDEN,
        "no_matching_policy",
    )
    .await;
    assert!(ci_tokens(&fx.db).await.is_empty(), "nothing was minted");

    // A registers its policy. B's is still there and still matches, and A's
    // exchange is not a conflict: it mints, as A's account, inside org A.
    register_policy(&fx, ours, policy_body(json!([whole_org("member")]))).await;
    let (status, minted) = exchange(&fx, &Run::new()).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(
        minted["service_account"],
        account_name(fx.org_id, "deployer")
    );
    let tokens = ci_tokens(&fx.db).await;
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].principal_user_id, ours);
    let exchanged = audit_rows(&fx.db, "oidc.token_exchanged").await;
    assert_eq!(exchanged.len(), 1);
    assert_eq!(
        exchanged[0].org_id,
        Some(fx.org_id),
        "in A's chain, not B's"
    );

    // B's policy is live and does match — it was simply never asked. Only a
    // workflow that names B's account, which the repository's owner writes,
    // acts as it.
    let body = json!({ "token": Run::new().jwt("oxy"), "service_account": theirs });
    let (status, minted) = exchange_with(body).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let tokens = ci_tokens(&fx.db).await;
    assert_eq!(tokens.len(), 2);
    assert!(tokens.iter().any(|t| t.principal_user_id == theirs));
}

/// Give `org` the slug `slug`, as its owner renaming it would.
async fn set_slug(db: &DatabaseConnection, org: Uuid, slug: &str) {
    organizations::Entity::update_many()
        .col_expr(organizations::Column::Slug, Expr::value(slug))
        .filter(organizations::Column::Id.eq(org))
        .exec(db)
        .await
        .expect("rename the org");
}

#[tokio::test]
async fn a_reused_org_slug_cannot_redirect_a_run_to_another_orgs_account() {
    // Org A: an account, and a policy on its repository.
    let (fx, ours, _policy) = deployer(json!([whole_org("member")])).await;
    let old_slug = format!("acme-{}", fx.org_id.simple());
    let new_slug = format!("renamed-{}", fx.org_id.simple());

    // A's slug changes. Nothing keeps the old one: it is free for anyone.
    set_slug(&fx.db, fx.org_id, &new_slug).await;

    // Org B takes it, creates an account with the same name, and registers a
    // policy carrying A's repository ids — which are public. `acme/deployer`
    // now reads as B's account, on a policy that matches A's runs exactly.
    let org_b = seed_org(&fx.db).await;
    join_org(&fx.db, org_b, fx.user.id, OrgRole::Owner).await;
    set_slug(&fx.db, org_b, &old_slug).await;
    let theirs = account_with_policy_in(&fx, org_b, "deployer").await;
    assert_ne!(ours, theirs);

    // A's workflow names A's account by id, and an id cannot be re-pointed:
    // the run mints for A's account, in A's org, whatever the slugs say now.
    let body = json!({ "token": Run::new().jwt("oxy"), "service_account": ours });
    let (status, minted) = exchange_with(body).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(
        minted["service_account"],
        format!("{new_slug}/deployer"),
        "the answer says where the run signed in, under the org's name today"
    );
    let tokens = ci_tokens(&fx.db).await;
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].principal_user_id, ours);
    assert!(tokens.iter().all(|t| t.principal_user_id != theirs));
    let exchanged = audit_rows(&fx.db, "oidc.token_exchanged").await;
    assert_eq!(exchanged.len(), 1);
    assert_eq!(exchanged[0].org_id, Some(fx.org_id), "in A's chain");

    // The readable form — the one that now points at B — is not accepted at
    // all: 400, and nothing minted for anyone.
    for named in [
        format!("{old_slug}/deployer"),
        format!("{new_slug}/deployer"),
    ] {
        let body = json!({ "token": Run::new().jwt("oxy"), "service_account": named });
        let (status, refusal) = exchange_with(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refusal}");
        assert_eq!(refusal["code"], "service_account_required", "{refusal}");
        assert!(refusal.get("token").is_none(), "{refusal}");
    }
    assert_eq!(ci_tokens(&fx.db).await.len(), 1, "nothing more was minted");
}

#[tokio::test]
async fn a_disabled_policy_or_account_never_mints() {
    let (fx, sa, policy) = deployer(json!([whole_org("member")])).await;
    let one = format!("{}/{policy}", policies_uri(fx.org_id, sa));
    let account = format!("/orgs/{}/service-accounts/{sa}", fx.org_id);
    let set = |uri: String, disabled: bool| {
        let cookie = fx.cookie.clone();
        async move {
            let body = Some(json!({ "disabled": disabled }));
            let (status, _) = in_session(&cookie, "PATCH", &uri, body).await;
            assert_eq!(status, StatusCode::OK);
        }
    };
    exchange_ok(&fx, &Run::new()).await;

    set(one.clone(), true).await;
    refused(
        &fx,
        &Run::new(),
        StatusCode::FORBIDDEN,
        "no_matching_policy",
    )
    .await;
    set(one.clone(), false).await;
    exchange_ok(&fx, &Run::new()).await;

    // Disabling the account stops its policies without touching one.
    set(account.clone(), true).await;
    refused(
        &fx,
        &Run::new(),
        StatusCode::FORBIDDEN,
        "no_matching_policy",
    )
    .await;
    set(account.clone(), false).await;
    let (minted, _) = exchange_ok(&fx, &Run::new()).await;
    assert_eq!(
        token_row(&fx.db, minted).await.trust_policy_id,
        Some(policy)
    );

    // Deleted, the policy is gone for good. What it minted is revoked and kept,
    // with the link cleared in code — the column is no foreign key.
    let (status, _) = in_session(&fx.cookie, "DELETE", &one, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    refused(
        &fx,
        &Run::new(),
        StatusCode::FORBIDDEN,
        "no_matching_policy",
    )
    .await;
    let row = token_row(&fx.db, minted).await;
    assert!(row.revoked_at.is_some());
    assert_eq!(row.trust_policy_id, None);
}

#[tokio::test]
async fn a_ci_token_ends_when_it_revokes_itself_or_its_policy_changes() {
    let (fx, sa, policy) = deployer(json!([whole_org("member")])).await;
    let ws = fx.workspace_id;
    let alive = |secret: String| async move { workspace_status(&secret, ws, "GET", "read").await };

    // `DELETE /auth/token`: what a one-shot run does on exit.
    let (id, secret) = exchange_ok(&fx, &Run::new()).await;
    assert_eq!(alive(secret.clone()).await, StatusCode::NO_CONTENT);
    let (status, _) = with_token(&secret, "DELETE", "/auth/token", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(alive(secret.clone()).await, StatusCode::UNAUTHORIZED);
    assert!(token_row(&fx.db, id).await.revoked_at.is_some());

    // Disabling the policy ends the tokens it minted, at once.
    let (id, secret) = exchange_ok(&fx, &Run::new()).await;
    let one = format!("{}/{policy}", policies_uri(fx.org_id, sa));
    in_session(&fx.cookie, "PATCH", &one, Some(json!({ "disabled": true }))).await;
    assert_eq!(alive(secret).await, StatusCode::UNAUTHORIZED);
    assert!(token_row(&fx.db, id).await.revoked_at.is_some());
    in_session(
        &fx.cookie,
        "PATCH",
        &one,
        Some(json!({ "disabled": false })),
    )
    .await;

    // Disabling the account stops a live token on the next request.
    let (_, secret) = exchange_ok(&fx, &Run::new()).await;
    let account = format!("/orgs/{}/service-accounts/{sa}", fx.org_id);
    in_session(
        &fx.cookie,
        "PATCH",
        &account,
        Some(json!({ "disabled": true })),
    )
    .await;
    assert_eq!(alive(secret).await, StatusCode::UNAUTHORIZED);
    in_session(
        &fx.cookie,
        "PATCH",
        &account,
        Some(json!({ "disabled": false })),
    )
    .await;

    // Deleting the account revokes what its policies minted.
    let (id, secret) = exchange_ok(&fx, &Run::new()).await;
    in_session(&fx.cookie, "DELETE", &account, None).await;
    assert_ne!(alive(secret).await, StatusCode::NO_CONTENT);
    assert!(token_row(&fx.db, id).await.revoked_at.is_some());
}

#[tokio::test]
async fn a_grant_is_capped_by_the_accounts_standing_at_the_exchange() {
    // Registered while the account was an admin.
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "admin").await;
    register_policy(&fx, sa, policy_body(json!([whole_org("admin")]))).await;
    let (_, secret) = exchange_ok(&fx, &Run::new()).await;
    assert_eq!(
        workspace_status(&secret, fx.workspace_id, "POST", "manage").await,
        StatusCode::NO_CONTENT
    );

    // Lowered to member: the next token is a member's, whatever the policy says.
    let account = format!("/orgs/{}/service-accounts/{sa}", fx.org_id);
    in_session(
        &fx.cookie,
        "PATCH",
        &account,
        Some(json!({ "org_role": "member" })),
    )
    .await;
    let (status, minted) = exchange(&fx, &Run::new()).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(minted["grants"][0]["role_ceiling"], "member");
    let secret = minted["token"].as_str().unwrap();
    assert_eq!(
        workspace_status(secret, fx.workspace_id, "POST", "manage").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn ci_tokens_are_in_the_org_inventory_under_their_account() {
    let (fx, sa, _policy) = deployer(json!([whole_org("member")])).await;
    let (id, _) = exchange_ok(&fx, &Run::new()).await;
    let uri = format!("/orgs/{}/tokens?kind=ci", fx.org_id);
    let (status, listed) = in_session(&fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let rows = listed["tokens"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["id"], id.to_string());
    assert_eq!(rows[0]["kind"], "ci");
    assert_eq!(rows[0]["owner"]["type"], "service_account");
    assert_eq!(rows[0]["owner"]["id"], sa.to_string());
    // The org cannot end its own account's token through revoke-grant.
    let revoke = format!("/orgs/{}/tokens/{id}/revoke-grant", fx.org_id);
    let (status, refused) = in_session(&fx.cookie, "POST", &revoke, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "use_service_account_routes");
}
