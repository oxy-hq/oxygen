//! Fixtures for GitHub Actions OIDC in tests: a run's token, signed with a
//! throwaway key the process is told to trust, and trust policies registered
//! through the API.
//!
//! - [`trust_test_keys`] makes this process verify against the test key
//!   instead of fetching GitHub's, and answers repository lookups from a stub
//!   instead of GitHub's API. Call it first in every test that exchanges.
//! - [`Run`] is one workflow run's claims; [`Run::jwt`] signs them for an
//!   audience. Every token gets a fresh `jti` unless one is set.
//! - [`register_policy`] / [`exchange`] drive the served routes.

use axum::http::StatusCode;
use entity::service_accounts;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use oxy_app::server::api::github_oidc_keys::install_keys_for_tests;
use oxy_app::server::api::org_api_access::repo_resolve::{Resolved, stub_resolver_for_tests};
use oxy_auth::github_oidc::GithubOidcClaims;
use oxy_auth::token::trust_policy_access::RepoIds;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::service_accounts::in_session;
use super::stack::flat_api;
use super::{Fixture, call};

const TEST_RSA_PEM: &str = include_str!("../../../../auth/src/github_oidc/testdata/test_rsa.pem");
const TEST_JWKS: &str = include_str!("../../../../auth/src/github_oidc/testdata/test_jwks.json");
const KID: &str = "oxy-test-key";
const ISSUER: &str = "https://token.actions.githubusercontent.com";

/// The repository every fixture run comes from, unless it says otherwise.
pub(crate) const REPO_ID: i64 = 987;
pub(crate) const OWNER_ID: i64 = 42;
/// What the stubbed GitHub lookup answers for `resolvable/repo`.
pub(crate) const RESOLVED_REPO_ID: i64 = 555_001;
pub(crate) const RESOLVED_OWNER_ID: i64 = 555_002;

/// Trust the test key, and answer repository lookups locally. Idempotent: the
/// first call in a process wins, and every call installs the same thing.
pub(crate) fn trust_test_keys() {
    let jwks = serde_json::from_str(TEST_JWKS).expect("the test JWKS parses");
    install_keys_for_tests(jwks);
    stub_resolver_for_tests(|owner, repo| {
        (owner == "resolvable").then(|| Resolved {
            ids: RepoIds {
                repository_id: RESOLVED_REPO_ID,
                repository_owner_id: RESOLVED_OWNER_ID,
            },
            full_name: format!("Resolvable/{repo}"),
        })
    });
}

/// One workflow run's claims.
#[derive(Clone)]
pub(crate) struct Run(Value);

impl Run {
    /// A github-hosted push to `acme/app`'s `main` through `release.yml`, in
    /// the `production` environment.
    pub(crate) fn new() -> Self {
        let now = chrono::Utc::now().timestamp();
        Self(json!({
            "iss": ISSUER,
            "iat": now - 30,
            "nbf": now - 30,
            "exp": now + 300,
            "sub": "repo:acme/app:environment:production",
            "repository": "acme/app",
            "repository_owner": "acme",
            "repository_owner_id": OWNER_ID.to_string(),
            "repository_id": REPO_ID.to_string(),
            "job_workflow_ref": "acme/app/.github/workflows/release.yml@refs/heads/main",
            "environment": "production",
            "event_name": "push",
            "runner_environment": "github-hosted",
            "ref": "refs/heads/main",
            "sha": "0123abcd0123abcd",
            "run_id": "7001",
            "run_attempt": "1",
            "actor_id": "5005",
        }))
    }

    /// Set (or, with `Value::Null`, remove) one claim.
    pub(crate) fn with(mut self, claim: &str, value: Value) -> Self {
        let claims = self.0.as_object_mut().expect("claims are an object");
        if value.is_null() {
            claims.remove(claim);
        } else {
            claims.insert(claim.to_string(), value);
        }
        self
    }

    /// The run's claims as the verifier hands them to the exchange, once the
    /// token has held: what the mint is given. A fresh `jti` unless one is set.
    pub(crate) fn claims(&self) -> GithubOidcClaims {
        let mut claims = self.0.clone();
        if claims.get("jti").is_none() {
            claims["jti"] = json!(Uuid::new_v4().to_string());
        }
        serde_json::from_value(claims).expect("the run's claims")
    }

    /// The run's token for `audience`, signed with the test key.
    pub(crate) fn jwt(&self, audience: &str) -> String {
        let mut claims = self.0.clone();
        claims["aud"] = json!(audience);
        if claims.get("jti").is_none() {
            claims["jti"] = json!(Uuid::new_v4().to_string());
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KID.to_string());
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).expect("the test key");
        encode(&header, &claims, &key).expect("sign the run's token")
    }
}

pub(crate) fn policies_uri(org_id: Uuid, sa_id: Uuid) -> String {
    format!("/orgs/{org_id}/service-accounts/{sa_id}/trust-policies")
}

/// "Whole organization", at `ceiling`.
pub(crate) fn whole_org(ceiling: &str) -> Value {
    json!({ "kind": "workspace", "workspace_id": null, "role_ceiling": ceiling })
}

/// The body that registers a policy for the fixture [`Run`], with `grants`.
pub(crate) fn policy_body(grants: Value) -> Value {
    json!({
        "repository": "acme/app",
        "workflow_path": ".github/workflows/release.yml",
        "environment": "production",
        "grants": grants,
        "repository_id": REPO_ID,
        "repository_owner_id": OWNER_ID,
    })
}

/// Register a policy on the account as the fixture's admin; its id.
pub(crate) async fn register_policy(fx: &Fixture, sa_id: Uuid, body: Value) -> Uuid {
    let uri = policies_uri(fx.org_id, sa_id);
    let (status, policy) = in_session(&fx.cookie, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "register a policy: {policy}");
    Uuid::parse_str(policy["id"].as_str().expect("an id")).expect("a uuid")
}

/// `POST /auth/oidc/exchange` with `body`, on the served flat tree.
pub(crate) async fn exchange_with(body: Value) -> (StatusCode, Value) {
    call(flat_api(), "POST", "/auth/oidc/exchange", &[], Some(body)).await
}

/// The account a fixture exchange names, unless it says otherwise.
pub(crate) const ACCOUNT: &str = "deployer";

/// `<org_slug>/<name>`: the readable name an exchange answers with, for an
/// account of `org_id`. The slug is the one `seed_org` gives every fixture
/// org. Never what a run sends — that is [`account_id`].
pub(crate) fn account_name(org_id: Uuid, name: &str) -> String {
    format!("acme-{}/{name}", org_id.simple())
}

/// The id of `org_id`'s account called `name`: how a run names the account it
/// acts as. Looked up by the org's id, so it does not care what the org's slug
/// is at the moment.
pub(crate) async fn account_id(fx: &Fixture, org_id: Uuid, name: &str) -> Uuid {
    service_accounts::Entity::find()
        .filter(service_accounts::Column::OrgId.eq(org_id))
        .filter(service_accounts::Column::Name.eq(name))
        .one(&fx.db)
        .await
        .expect("look the account up")
        .unwrap_or_else(|| panic!("no service account {name} in {org_id}"))
        .user_id
}

/// The audience this process's exchange requires: `oxy` while no public URL
/// is configured, which is every test that does not set one.
pub(crate) fn audience() -> String {
    oxy_app::server::api::oidc_exchange::audience::audience()
}

/// Exchange `run`'s token for this deployment's audience, as the fixture
/// org's `name`.
pub(crate) async fn exchange_as(fx: &Fixture, name: &str, run: &Run) -> (StatusCode, Value) {
    exchange_with(json!({
        "token": run.jwt(&audience()),
        "service_account": account_id(fx, fx.org_id, name).await,
    }))
    .await
}

/// Exchange `run`'s token for this deployment's audience, as the fixture org's
/// [`ACCOUNT`]. Every exchange names its account, by id: one that names none
/// is refused before anything is matched.
pub(crate) async fn exchange(fx: &Fixture, run: &Run) -> (StatusCode, Value) {
    exchange_as(fx, ACCOUNT, run).await
}

/// Exchange and expect a token: `(token id, secret)`.
pub(crate) async fn exchange_ok(fx: &Fixture, run: &Run) -> (Uuid, String) {
    let (status, minted) = exchange(fx, run).await;
    assert_eq!(status, StatusCode::OK, "exchange: {minted}");
    let id = Uuid::parse_str(minted["token_id"].as_str().expect("a token id")).expect("a uuid");
    (id, minted["token"].as_str().expect("a token").to_string())
}
