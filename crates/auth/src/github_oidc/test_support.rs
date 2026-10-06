//! Fixtures for the verifier's unit tests: a run's claims, and a throwaway
//! RSA key to sign them with. The key signs nothing outside tests.

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};

use super::claims::{GITHUB_OIDC_ISSUER, GithubOidcClaims};

pub(crate) const TEST_KID: &str = "oxy-test-key";
const TEST_RSA_PEM: &str = include_str!("testdata/test_rsa.pem");
const TEST_JWKS: &str = include_str!("testdata/test_jwks.json");

/// A github-hosted push to `acme/app`'s `main`, through `release.yml`, in the
/// `production` environment.
pub(crate) fn claims() -> GithubOidcClaims {
    GithubOidcClaims {
        repository: "acme/app".into(),
        repository_owner: "acme".into(),
        repository_owner_id: "42".into(),
        repository_id: Some("987".into()),
        job_workflow_ref: "acme/app/.github/workflows/release.yml@refs/heads/main".into(),
        environment: Some("production".into()),
        event_name: "push".into(),
        runner_environment: "github-hosted".into(),
        jti: "jti-1".into(),
        iat: 1_700_000_000,
        git_ref: Some("refs/heads/main".into()),
        sha: Some("0123abcd".into()),
        run_id: Some("7001".into()),
        run_attempt: Some("1".into()),
        actor_id: Some("5005".into()),
    }
}

pub(crate) fn jwks() -> JwkSet {
    serde_json::from_str(TEST_JWKS).expect("the test JWKS parses")
}

/// The JWT payload GitHub would sign for [`claims`], for audience `aud`,
/// valid from a minute ago for five minutes.
pub(crate) fn payload(aud: &str) -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({
        "iss": GITHUB_OIDC_ISSUER,
        "aud": aud,
        "iat": now - 60,
        "nbf": now - 60,
        "exp": now + 300,
        "sub": "repo:acme/app:environment:production",
        "repository": "acme/app",
        "repository_owner": "acme",
        "repository_owner_id": "42",
        "repository_id": "987",
        "job_workflow_ref": "acme/app/.github/workflows/release.yml@refs/heads/main",
        "environment": "production",
        "event_name": "push",
        "runner_environment": "github-hosted",
        "jti": "jti-1",
        "ref": "refs/heads/main",
        "sha": "0123abcd",
        "run_id": "7001",
        "run_attempt": "1",
        "actor_id": "5005",
    })
}

/// Sign `payload` with the test key, RS256, under [`TEST_KID`].
pub(crate) fn sign(payload: &Value) -> String {
    sign_with(payload, Algorithm::RS256, Some(TEST_KID))
}

pub(crate) fn sign_with(payload: &Value, alg: Algorithm, kid: Option<&str>) -> String {
    let mut header = Header::new(alg);
    header.kid = kid.map(str::to_string);
    let key = match alg {
        Algorithm::HS256 => EncodingKey::from_secret(b"not-a-github-key"),
        _ => EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).expect("the test key parses"),
    };
    encode(&header, payload, &key).expect("sign the test token")
}
