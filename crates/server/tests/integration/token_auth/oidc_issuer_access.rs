//! Trusted access from any OIDC issuer an org registers (design §3.4, *Any
//! OIDC issuer*). An org admin registers an issuer and a trust policy whose
//! claim conditions a token must meet; a workload trades its token at
//! `POST /auth/oidc/exchange` for a 15-minute `oxy_ci_` token.
//!
//! The settings form's two presets, as the server sees them — which is to say,
//! as nothing but an issuer and conditions:
//!
//! - **AWS**: an account's IAM outbound identity federation issuer, whose STS
//!   signs a JWT for any role that calls `GetWebIdentityToken` — ferret's ECS
//!   task, say. Keys through discovery, single-use `jti`s, and the condition
//!   `sub` = the role's ARN. No AWS: a locally generated issuer stands in for
//!   the account's. Its discovery document and key set are answered by a stub
//!   in place of the network (`oxy_api_identity::oidc_issuer_fetch::stub_for_tests`).
//! - **Custom OIDC**: an issuer at a private address, its keys pasted inline —
//!   never fetched — with conditions of its own.
//!
//! The tokens are real JWTs signed RS256 with the test key, so the whole
//! verifier runs: the registered issuer, the signature, the audience alone,
//! the lifetime cap, the conditions, exactly one policy, the locked mint.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::oidc_issuer_trust_policies;
use entity::org_members::OrgRole;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::oidc::{audience, whole_org};
use super::service_accounts::{
    admin_fixture, create_account, in_session, with_token, workspace_status,
};
use super::stack::{flat_api, join_org};
use super::trusted_access::token_row;
use super::{Fixture, audit_rows, call, seed_org, seed_workspace_in};

const TEST_RSA_PEM: &str = include_str!("../../../../auth/src/github_oidc/testdata/test_rsa.pem");
const TEST_JWKS: &str = include_str!("../../../../auth/src/github_oidc/testdata/test_jwks.json");
const KID: &str = "oxy-test-key";

/// The stand-in AWS account's outbound identity federation issuer.
const ISSUER: &str = "https://abc123-def456.tokens.sts.global.api.aws";
const JWKS_URI: &str = "https://abc123-def456.tokens.sts.global.api.aws/.well-known/jwks.json";
/// What the org calls the stand-in issuer: the name policies and minted tokens show.
const ISSUER_NAME: &str = "oxy-management";
/// ferret's ECS task role, the `sub` of the tokens STS signs for it.
const ROLE: &str = "arn:aws:iam::123456789012:role/ferret-task";
/// An issuer whose discovery document names another issuer.
const LYING_ISSUER: &str = "https://oidc.lying.example.com";
/// An issuer whose keys are published on another host.
const ELSEWHERE_ISSUER: &str = "https://oidc.elsewhere.example.com";

fn discovery(issuer: &str) -> String {
    format!("{issuer}/.well-known/openid-configuration")
}

/// Answer issuer fetches locally. Idempotent: every call installs the same.
pub(super) fn trust_issuers() {
    oxy_api_identity::oidc_issuer_fetch::stub_for_tests(|url| {
        let jwks: Value = serde_json::from_str(TEST_JWKS).expect("the test JWKS");
        if url == discovery(ISSUER) {
            Some(json!({ "issuer": ISSUER, "jwks_uri": JWKS_URI }))
        } else if url == JWKS_URI {
            Some(jwks)
        } else if url == discovery(LYING_ISSUER) {
            Some(json!({ "issuer": ISSUER, "jwks_uri": JWKS_URI }))
        } else if url == discovery(ELSEWHERE_ISSUER) {
            Some(json!({ "issuer": ELSEWHERE_ISSUER, "jwks_uri": JWKS_URI }))
        } else {
            None
        }
    });
}

/// One workload's token.
#[derive(Clone)]
struct Workload(Value);

impl Workload {
    /// What STS signs for ferret's task role: five minutes, issued half a
    /// minute ago, for this deployment alone. The AWS claims are nested, as
    /// STS nests them.
    fn new() -> Self {
        let now = Utc::now().timestamp();
        Self(json!({
            "iss": ISSUER,
            "sub": ROLE,
            "aud": [audience()],
            "iat": now - 30,
            "exp": now + 270,
            "jti": Uuid::new_v4().to_string(),
            "https://sts.amazonaws.com/": {
                "aws_account": "123456789012",
                "source_region": "us-west-2",
                "principal_id": ROLE,
            },
        }))
    }

    fn with(mut self, claim: &str, value: Value) -> Self {
        let claims = self.0.as_object_mut().expect("an object");
        if value.is_null() {
            claims.remove(claim);
        } else {
            claims.insert(claim.into(), value);
        }
        self
    }

    /// A token STS signed for the role `arn`.
    fn as_role(self, arn: &str) -> Self {
        self.with("sub", json!(arn))
    }

    fn signed_by(&self, kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).expect("the test key");
        encode(&header, &self.0, &key).expect("sign the token")
    }

    fn jwt(&self) -> String {
        self.signed_by(KID)
    }
}

fn issuers_uri(org_id: Uuid) -> String {
    format!("/orgs/{org_id}/oidc-issuers")
}

pub(super) fn policies_uri(org_id: Uuid, sa_id: Uuid) -> String {
    format!("/orgs/{org_id}/service-accounts/{sa_id}/oidc-issuer-trust-policies")
}

/// Register an issuer as `fx`'s admin, as the AWS preset fills the form in:
/// keys through discovery, `jti` single use (left to its default). The answer.
async fn register_issuer(fx: &Fixture, issuer: &str) -> (StatusCode, Value) {
    let body = json!({ "name": ISSUER_NAME, "issuer": issuer, "preset": "aws" });
    in_session(&fx.cookie, "POST", &issuers_uri(fx.org_id), Some(body)).await
}

pub(super) async fn the_issuer(fx: &Fixture) -> Uuid {
    let (status, issuer) = register_issuer(fx, ISSUER).await;
    assert_eq!(status, StatusCode::CREATED, "register the issuer: {issuer}");
    Uuid::parse_str(issuer["id"].as_str().expect("an id")).expect("a uuid")
}

/// A policy as the AWS preset writes one for ferret's task role.
pub(super) fn policy_body(issuer: Uuid, grants: Value) -> Value {
    conditions_body(issuer, json!([{ "claim": "sub", "value": ROLE }]), grants)
}

fn conditions_body(issuer: Uuid, conditions: Value, grants: Value) -> Value {
    json!({ "issuer_id": issuer, "conditions": conditions, "grants": grants })
}

async fn register_policy(fx: &Fixture, sa: Uuid, body: Value) -> Uuid {
    let (status, policy) =
        in_session(&fx.cookie, "POST", &policies_uri(fx.org_id, sa), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "register a policy: {policy}");
    Uuid::parse_str(policy["id"].as_str().expect("an id")).expect("a uuid")
}

async fn exchange_token(token: String, sa: Uuid) -> (StatusCode, Value) {
    let body = json!({ "token": token, "service_account": sa });
    call(flat_api(), "POST", "/auth/oidc/exchange", &[], Some(body)).await
}

async fn exchange(workload: &Workload, sa: Uuid) -> (StatusCode, Value) {
    exchange_token(workload.jwt(), sa).await
}

/// `(token id, secret)` of a successful exchange.
async fn exchange_ok(workload: &Workload, sa: Uuid) -> (Uuid, String) {
    let (status, minted) = exchange(workload, sa).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let id = Uuid::parse_str(minted["token_id"].as_str().unwrap()).unwrap();
    (id, minted["token"].as_str().unwrap().to_string())
}

async fn refused(workload: &Workload, sa: Uuid, status: StatusCode, code: &str) {
    let (got, body) = exchange(workload, sa).await;
    assert_eq!(got, status, "{code}: {body}");
    assert_eq!(body["code"], code, "{body}");
    assert!(body.get("token").is_none(), "a refusal mints nothing");
}

/// An admin's org with the AWS account's issuer registered, one service
/// account, and one policy trusting ferret's task role with `grants`.
async fn deployer(grants: Value) -> (Fixture, Uuid, Uuid, Uuid) {
    trust_issuers();
    let fx = admin_fixture().await;
    let issuer_id = the_issuer(&fx).await;
    let sa = create_account(&fx, "ferret", "member").await;
    let policy = register_policy(&fx, sa, policy_body(issuer_id, grants)).await;
    (fx, issuer_id, sa, policy)
}

async fn policy_row(
    db: &DatabaseConnection,
    id: Uuid,
) -> Option<oidc_issuer_trust_policies::Model> {
    oidc_issuer_trust_policies::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
}

async fn is_live(secret: &str) -> bool {
    with_token(secret, "GET", "/auth/token", None).await.0 == StatusCode::OK
}

#[tokio::test]
async fn a_workload_gets_a_scoped_fifteen_minute_token() {
    trust_issuers();
    let fx = admin_fixture().await;
    let (status, issuer) = register_issuer(&fx, ISSUER).await;
    assert_eq!(status, StatusCode::CREATED, "{issuer}");
    assert_eq!(issuer["preset"], "aws");
    assert_eq!(issuer["key_source"], "discovery");
    assert_eq!(issuer["jti_single_use"], true);
    assert_eq!(issuer["issuer"], ISSUER);
    assert_eq!(
        issuer["jwks_uri"], JWKS_URI,
        "read from the discovery document"
    );
    let issuer_id = Uuid::parse_str(issuer["id"].as_str().unwrap()).unwrap();

    let granted = fx.workspace_id;
    let foreign_org = seed_org(&fx.db).await;
    let foreign = seed_workspace_in(&fx.db, foreign_org).await;
    let sa = create_account(&fx, "ferret", "member").await;
    // One workspace is not a grant this kind of policy takes: workspaces are
    // being retired, so it grants the whole organization or an app.
    let one = json!([{ "kind": "workspace", "workspace_id": granted, "role_ceiling": "member" }]);
    let (status, _) = in_session(
        &fx.cookie,
        "POST",
        &policies_uri(fx.org_id, sa),
        Some(policy_body(issuer_id, one)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let policy = register_policy(
        &fx,
        sa,
        policy_body(issuer_id, json!([whole_org("member")])),
    )
    .await;
    let (_, listed) = in_session(&fx.cookie, "GET", &policies_uri(fx.org_id, sa), None).await;
    let shown = &listed["trust_policies"][0];
    assert_eq!(shown["conditions"][0]["claim"], "sub");
    assert_eq!(shown["conditions"][0]["value"], ROLE);
    assert_eq!(shown["issuer"]["name"], ISSUER_NAME);
    assert_eq!(shown["service_account"]["name"], "ferret");
    // The settings screen's list: every account's policies in the org.
    let org_wide = format!("/orgs/{}/oidc-issuer-trust-policies", fx.org_id);
    let (_, all) = in_session(&fx.cookie, "GET", &org_wide, None).await;
    assert_eq!(all["trust_policies"][0]["id"], policy.to_string());

    let before = Utc::now();
    let workload = Workload::new();
    let (status, minted) = exchange(&workload, sa).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let secret = minted["token"].as_str().unwrap().to_string();
    assert!(secret.starts_with("oxy_ci_"), "{secret}");
    let token_id = Uuid::parse_str(minted["token_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        minted["service_account"],
        format!("acme-{}/ferret", fx.org_id.simple())
    );
    let expires: DateTime<Utc> = minted["expires_at"].as_str().unwrap().parse().unwrap();
    let lifetime = expires - before;
    assert!(lifetime > Duration::minutes(14) && lifetime <= Duration::minutes(16));
    assert_eq!(minted["grants"][0]["workspace_id"], Value::Null);
    assert_eq!(minted["grants"][0]["role_ceiling"], "member");

    // Inside the org, at the ceiling; another org, 404.
    assert_eq!(
        workspace_status(&secret, granted, "POST", "write").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        workspace_status(&secret, granted, "POST", "manage").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        workspace_status(&secret, foreign, "GET", "read").await,
        StatusCode::NOT_FOUND
    );
    // A token cannot manage issuers or policies.
    let (status, _) = with_token(&secret, "GET", &issuers_uri(fx.org_id), None).await;
    assert_ne!(status, StatusCode::OK);

    // The row: the policy, the role, never the JWT.
    let row = token_row(&fx.db, token_id).await;
    assert_eq!(row.kind, "ci");
    assert_eq!(row.principal_user_id, sa);
    assert_eq!(row.trust_policy_id, Some(policy));
    assert_eq!(row.name, format!("oidc:{ISSUER_NAME} {ROLE}"));
    let claims = row.oidc_claims.expect("the verified claims");
    assert_eq!(claims["sub"], ROLE);
    assert_eq!(
        claims["claims"]["https://sts.amazonaws.com/"]["source_region"],
        "us-west-2"
    );
    assert_eq!(claims["iss"], ISSUER);
    assert!(
        !claims.to_string().contains("eyJ"),
        "the JWT is never stored"
    );

    assert!(
        policy_row(&fx.db, policy)
            .await
            .unwrap()
            .last_used_at
            .is_some()
    );
    let events = audit_rows(&fx.db, "oidc.token_exchanged").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].org_id, Some(fx.org_id));
    assert_eq!(events[0].actor_user_id, Some(sa));
    assert_eq!(events[0].metadata["provider"], "oidc_issuer");
    assert_eq!(events[0].metadata["trust_policy_id"], policy.to_string());
    assert_eq!(
        events[0].metadata["claims"]["claims"]["https://sts.amazonaws.com/"]["aws_account"],
        "123456789012"
    );
    let registered = audit_rows(&fx.db, "oidc_issuer.registered").await;
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].metadata["jwks_uri"], JWKS_URI);

    // Single use: the same token is spent. The workload asks STS for another.
    refused(&workload, sa, StatusCode::UNAUTHORIZED, "replayed").await;
    let (again, _) = exchange_ok(&Workload::new(), sa).await;
    assert_ne!(again, token_id);
}

#[tokio::test]
async fn a_token_must_be_this_issuers_for_this_deployment_alone_and_short_lived() {
    let (_fx, _issuer, sa, _policy) = deployer(json!([whole_org("member")])).await;
    let now = Utc::now().timestamp();

    // Another deployment's audience, or ours beside the API server's.
    refused(
        &Workload::new().with("aud", json!(["oxy:app.oxygen-hq.com"])),
        sa,
        StatusCode::UNAUTHORIZED,
        "wrong_audience",
    )
    .await;
    let two = json!([audience(), "sts.amazonaws.com"]);
    refused(
        &Workload::new().with("aud", two),
        sa,
        StatusCode::UNAUTHORIZED,
        "wrong_audience",
    )
    .await;
    // Issued to live past an hour — the admission-injected token, extended.
    let year = Workload::new().with("exp", json!(now + 365 * 24 * 3600));
    refused(
        &year,
        sa,
        StatusCode::UNAUTHORIZED,
        "token_lifetime_too_long",
    )
    .await;
    // Expired.
    let expired = Workload::new()
        .with("iat", json!(now - 900))
        .with("nbf", json!(now - 900))
        .with("exp", json!(now - 300));
    refused(&expired, sa, StatusCode::UNAUTHORIZED, "expired").await;
    // A legacy Secret-based token: no `exp`.
    refused(
        &Workload::new().with("exp", Value::Null),
        sa,
        StatusCode::UNAUTHORIZED,
        "invalid_token",
    )
    .await;
    // Signed by a key the issuer does not publish.
    let (status, body) = exchange_token(Workload::new().signed_by("not-the-issuers"), sa).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_token"))
    );
    // Another AWS account's issuer, which this org never registered.
    let stranger = Workload::new().with(
        "iss",
        json!("https://another-account.tokens.sts.global.api.aws"),
    );
    refused(&stranger, sa, StatusCode::UNAUTHORIZED, "unknown_issuer").await;
    // An account id that names no account reads the same.
    refused(
        &Workload::new(),
        Uuid::new_v4(),
        StatusCode::UNAUTHORIZED,
        "unknown_issuer",
    )
    .await;
}

#[tokio::test]
async fn only_the_exact_role_of_the_policy_matches() {
    let (fx, _issuer, sa, _policy) = deployer(json!([whole_org("member")])).await;
    for role in [
        "arn:aws:iam::123456789012:role/ferret-task-2",
        "arn:aws:iam::123456789012:role/ferret",
        "arn:aws:iam::999999999999:role/ferret-task",
        "arn:aws:iam::123456789012:user/ferret-task",
    ] {
        refused(
            &Workload::new().as_role(role),
            sa,
            StatusCode::FORBIDDEN,
            "no_matching_policy",
        )
        .await;
    }
    let rejections = audit_rows(&fx.db, "oidc.exchange_rejected").await;
    let no_match = rejections
        .iter()
        .find(|r| r.metadata["reason"] == "no_matching_policy")
        .expect("a refusal row");
    assert_eq!(no_match.actor_email, "oidc-issuer");
    assert_eq!(
        no_match.metadata["claims"]["sub"],
        "arn:aws:iam::123456789012:role/ferret-task-2"
    );
}

#[tokio::test]
async fn another_orgs_issuer_and_policies_are_never_consulted() {
    trust_issuers();
    let fx = admin_fixture().await;
    // This org's account, with no issuer registered here.
    let sa = create_account(&fx, "ferret", "member").await;
    // The same AWS account registered by another org, with a policy for the role.
    let other = seed_org(&fx.db).await;
    join_org(&fx.db, other, fx.user.id, OrgRole::Owner).await;
    let body = json!({ "name": ISSUER_NAME, "issuer": ISSUER });
    let (status, issuer) = in_session(&fx.cookie, "POST", &issuers_uri(other), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{issuer}");
    let issuer_id = Uuid::parse_str(issuer["id"].as_str().unwrap()).unwrap();
    let body = json!({ "name": "ferret", "org_role": "member" });
    let accounts = format!("/orgs/{other}/service-accounts");
    let (_, account) = in_session(&fx.cookie, "POST", &accounts, Some(body)).await;
    let theirs = Uuid::parse_str(account["id"].as_str().unwrap()).unwrap();
    let grants = json!([whole_org("member")]);
    let (status, policy) = in_session(
        &fx.cookie,
        "POST",
        &policies_uri(other, theirs),
        Some(policy_body(issuer_id, grants)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{policy}");
    exchange_ok(&Workload::new(), theirs).await;
    // Naming this org's account, the other org's registration counts for nothing.
    refused(
        &Workload::new(),
        sa,
        StatusCode::UNAUTHORIZED,
        "unknown_issuer",
    )
    .await;
    // Nor can a policy here name the other org's issuer.
    let (status, _) = in_session(
        &fx.cookie,
        "POST",
        &policies_uri(fx.org_id, sa),
        Some(policy_body(issuer_id, json!([whole_org("member")]))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn any_change_ends_the_tokens_a_policy_minted() {
    let (fx, issuer_id, sa, policy) = deployer(json!([whole_org("member")])).await;
    let uri = format!("{}/{policy}", policies_uri(fx.org_id, sa));

    // New grants: the token minted under the old ones stops.
    let (_, first) = exchange_ok(&Workload::new(), sa).await;
    let grants = json!({ "grants": [whole_org("viewer")] });
    let (status, _) = in_session(&fx.cookie, "PATCH", &uri, Some(grants)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!is_live(&first).await, "revoked with the edit");

    // Disabled: it stops, and nothing more is minted until it is enabled.
    let (_, second) = exchange_ok(&Workload::new(), sa).await;
    in_session(&fx.cookie, "PATCH", &uri, Some(json!({ "disabled": true }))).await;
    assert!(!is_live(&second).await);
    refused(
        &Workload::new(),
        sa,
        StatusCode::FORBIDDEN,
        "no_matching_policy",
    )
    .await;
    in_session(
        &fx.cookie,
        "PATCH",
        &uri,
        Some(json!({ "disabled": false })),
    )
    .await;

    // What a policy names is never edited.
    let (status, _) = in_session(
        &fx.cookie,
        "PATCH",
        &uri,
        Some(json!({ "conditions": [{ "claim": "sub", "value": "x" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Deleting the issuer ends its policies and their tokens.
    let (_, third) = exchange_ok(&Workload::new(), sa).await;
    let (status, _) = in_session(
        &fx.cookie,
        "DELETE",
        &format!("{}/{issuer_id}", issuers_uri(fx.org_id)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!is_live(&third).await, "revoked with its issuer");
    assert!(
        policy_row(&fx.db, policy).await.is_none(),
        "the policy went with it"
    );
    refused(
        &Workload::new(),
        sa,
        StatusCode::UNAUTHORIZED,
        "unknown_issuer",
    )
    .await;
    let deleted = audit_rows(&fx.db, "oidc_issuer.deleted").await;
    assert_eq!(deleted[0].metadata["revoked_tokens"], 1);
    assert_eq!(deleted[0].metadata["deleted_policies"], 1);
}

#[tokio::test]
async fn a_grant_is_capped_by_the_accounts_standing_at_the_exchange() {
    trust_issuers();
    let fx = admin_fixture().await;
    let issuer_id = the_issuer(&fx).await;
    let sa = create_account(&fx, "ferret", "admin").await;
    register_policy(&fx, sa, policy_body(issuer_id, json!([whole_org("admin")]))).await;
    let (_, secret) = exchange_ok(&Workload::new(), sa).await;
    assert_eq!(
        workspace_status(&secret, fx.workspace_id, "POST", "manage").await,
        StatusCode::NO_CONTENT
    );

    let account = format!("/orgs/{}/service-accounts/{sa}", fx.org_id);
    in_session(
        &fx.cookie,
        "PATCH",
        &account,
        Some(json!({ "org_role": "member" })),
    )
    .await;
    let (status, minted) = exchange(&Workload::new(), sa).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(minted["grants"][0]["role_ceiling"], "member");
}

#[tokio::test]
async fn registration_is_session_only_and_checks_the_issuer() {
    trust_issuers();
    let fx = admin_fixture().await;

    // The discovery document must describe the issuer, and keep its keys at home.
    for issuer in [
        LYING_ISSUER,
        ELSEWHERE_ISSUER,
        "https://oidc.unreachable.example.com",
    ] {
        let (status, body) = register_issuer(&fx, issuer).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{issuer}: {body}");
        assert_eq!(body["code"], "issuer_unusable");
    }
    // Not a public HTTPS address, or GitHub's own issuer.
    for issuer in [
        "http://oidc.eks.example.com",
        "https://kubernetes.default.svc",
        "https://169.254.169.254/latest",
        "https://token.actions.githubusercontent.com",
    ] {
        let (status, _) = register_issuer(&fx, issuer).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{issuer}");
    }
    let issuer_id = the_issuer(&fx).await;
    let (status, body) = register_issuer(&fx, ISSUER).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("issuer_exists"))
    );

    // A token, even one that reaches the org as an admin, registers nothing.
    let sa = create_account(&fx, "ferret", "admin").await;
    register_policy(&fx, sa, policy_body(issuer_id, json!([whole_org("admin")]))).await;
    let (_, secret) = exchange_ok(&Workload::new(), sa).await;
    let body = json!({ "name": "x", "issuer": "https://oidc.other.example.com" });
    let (status, refusal) = with_token(&secret, "POST", &issuers_uri(fx.org_id), Some(body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refusal}");
    let (status, _) = with_token(
        &secret,
        "POST",
        &policies_uri(fx.org_id, sa),
        Some(policy_body(issuer_id, json!([whole_org("member")]))),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Conditions are checked: a bare '*', one in the middle, 'iss', or the
    // audience alone is refused.
    for conditions in [
        json!([{ "claim": "sub", "value": "*" }]),
        json!([{ "claim": "sub", "value": "arn:aws:*:ferret" }]),
        json!([{ "claim": "iss", "value": ISSUER }, { "claim": "sub", "value": "x" }]),
        json!([{ "claim": "aud", "value": audience() }]),
        json!([]),
    ] {
        let bad = conditions_body(issuer_id, conditions.clone(), json!([whole_org("member")]));
        let (status, _) =
            in_session(&fx.cookie, "POST", &policies_uri(fx.org_id, sa), Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{conditions}");
    }

    // An org the caller is not in: refused, whichever way.
    let elsewhere = seed_org(&fx.db).await;
    let (status, _) = in_session(&fx.cookie, "GET", &issuers_uri(elsewhere), None).await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{status}"
    );
}

#[tokio::test]
async fn more_than_one_matching_policy_is_refused_never_merged() {
    let (fx, issuer_id, sa, first) = deployer(json!([whole_org("member")])).await;
    // A second policy on the same account whose prefix also takes the role.
    let prefix = json!([{ "claim": "sub", "value": "arn:aws:iam::123456789012:role/ferret-*" }]);
    let second = register_policy(
        &fx,
        sa,
        conditions_body(issuer_id, prefix, json!([whole_org("viewer")])),
    )
    .await;
    refused(
        &Workload::new(),
        sa,
        StatusCode::FORBIDDEN,
        "ambiguous_policy",
    )
    .await;
    let rejected = audit_rows(&fx.db, "oidc.exchange_rejected").await;
    assert!(
        rejected
            .iter()
            .any(|r| r.metadata["reason"] == "ambiguous_policy")
    );
    // Disable one, and the other alone admits the workload.
    let uri = format!("{}/{first}", policies_uri(fx.org_id, sa));
    in_session(&fx.cookie, "PATCH", &uri, Some(json!({ "disabled": true }))).await;
    let (status, minted) = exchange(&Workload::new(), sa).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(minted["grants"][0]["role_ceiling"], "viewer");
    let row = token_row(
        &fx.db,
        Uuid::parse_str(minted["token_id"].as_str().unwrap()).unwrap(),
    )
    .await;
    assert_eq!(row.trust_policy_id, Some(second));
}

/// An issuer Oxygen cannot reach: the stub answers nothing for it.
const PRIVATE_ISSUER: &str = "https://oidc.build-farm.internal";

/// A token from the private issuer, for the build-farm job `job`.
fn farm_token(job: &str) -> Workload {
    let now = Utc::now().timestamp();
    Workload(json!({
        // Its tokens carry a trailing '/'; it is registered without one.
        "iss": format!("{PRIVATE_ISSUER}/"),
        "sub": format!("ci:build-farm:{job}"),
        "aud": audience(),
        "iat": now - 30,
        "exp": now + 270,
        "jti": Uuid::new_v4().to_string(),
        "project": "acme",
        "pipeline": 42,
    }))
}

#[tokio::test]
async fn a_custom_issuer_with_inline_keys_is_never_fetched() {
    trust_issuers();
    let fx = admin_fixture().await;
    let keys: Value = serde_json::from_str(TEST_JWKS).expect("the test JWKS");
    let body = json!({
        "name": "build-farm",
        "issuer": PRIVATE_ISSUER,
        "key_source": "inline",
        "jwks": keys,
        "preset": "custom",
    });
    let (status, issuer) =
        in_session(&fx.cookie, "POST", &issuers_uri(fx.org_id), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{issuer}");
    assert_eq!(issuer["key_source"], "inline");
    assert_eq!(issuer["jwks_uri"], Value::Null);
    assert_eq!(issuer["inline_key_ids"], json!([KID]));
    assert_eq!(
        issuer["jti_single_use"], true,
        "single use unless asked off"
    );
    assert!(issuer.get("jwks").is_none(), "the keys are not echoed");
    let farm = Uuid::parse_str(issuer["id"].as_str().unwrap()).unwrap();

    // A private issuer with keys to fetch is refused: there is nothing to read.
    let fetched = json!({ "name": "x", "issuer": "https://oidc.other.internal" });
    let (status, _) = in_session(&fx.cookie, "POST", &issuers_uri(fx.org_id), Some(fetched)).await;
    assert!(status.is_client_error(), "{status}");

    let sa = create_account(&fx, "builder", "member").await;
    let conditions = json!([
        { "claim": "sub", "value": "ci:build-farm:*" },
        { "claim": "project", "value": "acme" },
        { "claim": "pipeline", "value": "42" },
    ]);
    register_policy(
        &fx,
        sa,
        conditions_body(farm, conditions, json!([whole_org("member")])),
    )
    .await;

    let job = farm_token("job-1");
    let (id, secret) = exchange_ok(&job, sa).await;
    assert!(is_live(&secret).await);
    let row = token_row(&fx.db, id).await;
    assert_eq!(row.name, "oidc:build-farm ci:build-farm:job-1");
    // Single use: the same token is spent.
    refused(&job, sa, StatusCode::UNAUTHORIZED, "replayed").await;
    // A single-use issuer's token must carry a `jti`.
    refused(
        &farm_token("job-2").with("jti", Value::Null),
        sa,
        StatusCode::UNAUTHORIZED,
        "jti_required",
    )
    .await;
    // Every condition must hold.
    refused(
        &farm_token("job-3").with("project", json!("other")),
        sa,
        StatusCode::FORBIDDEN,
        "no_matching_policy",
    )
    .await;
    refused(
        &farm_token("job-4").with("sub", json!("ci:other-farm:job-4")),
        sa,
        StatusCode::FORBIDDEN,
        "no_matching_policy",
    )
    .await;
    // A key the inline set does not hold verifies nothing.
    let (status, body) = exchange_token(farm_token("job-5").signed_by("not-pasted"), sa).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_token"))
    );
}
