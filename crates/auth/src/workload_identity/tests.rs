//! The envelope, for any registered issuer. The tokens here are shaped like a
//! Kubernetes pod's; nothing in the verifier knows that.

use jsonwebtoken::{Algorithm, DecodingKey};
use serde_json::{Value, json};

use super::claims::{RECORDED_MAX_BYTES, VerifiedToken};
use super::verify::{IssuerTokenError, MAX_LIFETIME_SECONDS, decode_claims};
use super::{keys, unverified_issuer, verify_token};
use crate::github_oidc::JwksCache;
use crate::github_oidc::test_support::{TEST_KID, jwks, sign, sign_with};

const ISSUER: &str = "https://oidc.eks.us-west-2.amazonaws.com/id/EXAMPLE";
const AUDIENCE: &str = "oxy:app.oxygen-hq.com";

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A pod's projected token: `kestra/kestra-worker`, ten minutes, issued a
/// minute ago.
fn payload() -> Value {
    let now = now();
    json!({
        "iss": ISSUER,
        "sub": "system:serviceaccount:kestra:kestra-worker",
        "aud": [AUDIENCE],
        "iat": now - 60,
        "nbf": now - 60,
        "exp": now + 540,
        "jti": "8f1c…",
        "kubernetes.io": {
            "namespace": "kestra",
            "serviceaccount": { "name": "kestra-worker", "uid": "sa-uid" },
            "pod": { "name": "kestra-worker-7d9", "uid": "pod-uid" },
            "node": { "name": "ip-10-0-1-2", "uid": "node-uid" },
        },
    })
}

fn with(mut payload: Value, claim: &str, value: Value) -> Value {
    let claims = payload.as_object_mut().expect("an object");
    if value.is_null() {
        claims.remove(claim);
    } else {
        claims.insert(claim.into(), value);
    }
    payload
}

fn key() -> DecodingKey {
    DecodingKey::from_jwk(jwks().find(TEST_KID).expect("the test key")).expect("a key")
}

fn decoded(payload: &Value) -> Result<VerifiedToken, IssuerTokenError> {
    decode_claims(&sign(payload), &key(), ISSUER, AUDIENCE, now())
}

fn refusal(payload: &Value) -> &'static str {
    match decoded(payload) {
        Ok(_) => "accepted",
        Err(e) => e.code(),
    }
}

#[test]
fn a_pods_projected_token_verifies() {
    let token = decoded(&payload()).expect("verifies");
    assert_eq!(token.sub, "system:serviceaccount:kestra:kestra-worker");
    assert_eq!(token.jti.as_deref(), Some("8f1c…"));
    assert_eq!(token.claims["kubernetes.io"]["namespace"], "kestra");
    // One audience may also be a bare string.
    assert!(decoded(&with(payload(), "aud", json!(AUDIENCE))).is_ok());
}

#[test]
fn the_audience_is_this_deployments_and_nothing_else() {
    for aud in [
        json!("oxy:aip.staging.oxy.tech"),
        json!(["https://kubernetes.default.svc"]),
        // Ours, beside the API server's: a token good there too is not ours.
        json!([AUDIENCE, "https://kubernetes.default.svc"]),
        json!(["https://kubernetes.default.svc", AUDIENCE]),
        json!([]),
    ] {
        assert_eq!(
            refusal(&with(payload(), "aud", aud.clone())),
            "wrong_audience",
            "{aud}"
        );
    }
}

#[test]
fn only_the_registered_issuer_verifies() {
    let other = with(payload(), "iss", json!("https://evil.example"));
    assert_eq!(refusal(&other), "invalid_token");
    let longer = with(payload(), "iss", json!(format!("{ISSUER}x")));
    assert_eq!(refusal(&longer), "invalid_token", "exact, not a prefix");
    // A trailing '/' is ignored, on the token's side and on the registered one.
    let slashed = with(payload(), "iss", json!(format!("{ISSUER}/")));
    assert!(decoded(&slashed).is_ok());
    let registered_slashed = decode_claims(
        &sign(&payload()),
        &key(),
        &format!("{ISSUER}/"),
        AUDIENCE,
        now(),
    );
    assert!(registered_slashed.is_ok());
}

#[test]
fn a_token_lives_at_most_an_hour() {
    let now = now();
    let hour = with(
        with(payload(), "iat", json!(now - 60)),
        "exp",
        json!(now - 60 + MAX_LIFETIME_SECONDS),
    );
    assert!(
        decoded(&hour).is_ok(),
        "an hour exactly is the kubelet default"
    );
    let longer = with(hour, "exp", json!(now - 60 + MAX_LIFETIME_SECONDS + 1));
    assert_eq!(refusal(&longer), "token_lifetime_too_long");
    // The admission-injected token, extended to a year.
    let year = with(payload(), "exp", json!(now + 365 * 24 * 3600));
    assert_eq!(refusal(&year), "token_lifetime_too_long");
}

#[test]
fn expiry_issue_time_and_not_before_hold() {
    let now = now();
    let expired = with(
        with(payload(), "iat", json!(now - 900)),
        "exp",
        json!(now - 300),
    );
    let expired = with(expired, "nbf", json!(now - 900));
    assert_eq!(refusal(&expired), "expired");
    // A legacy Secret token: no `exp` at all.
    assert_eq!(
        refusal(&with(payload(), "exp", Value::Null)),
        "invalid_token"
    );
    // No `iat`.
    assert_eq!(
        refusal(&with(payload(), "iat", Value::Null)),
        "invalid_token"
    );
    // Issued in the future.
    let future = with(payload(), "iat", json!(now + 600));
    assert_eq!(refusal(&future), "invalid_token");
    // Not yet valid.
    let early = with(payload(), "nbf", json!(now + 600));
    assert_eq!(refusal(&early), "invalid_token");
}

#[test]
fn a_subject_is_required_and_nothing_else_about_it_is_judged_here() {
    assert_eq!(
        refusal(&with(payload(), "sub", Value::Null)),
        "invalid_token"
    );
    assert_eq!(refusal(&with(payload(), "sub", json!(""))), "invalid_token");
    // Any issuer's subject verifies: what it must be is a policy's condition.
    for sub in [
        "repo:acme/app:environment:production",
        "project_path:acme/app",
    ] {
        assert!(
            decoded(&with(payload(), "sub", json!(sub))).is_ok(),
            "{sub}"
        );
    }
    // A Kubernetes token with no `kubernetes.io` claim verifies too.
    assert!(decoded(&with(payload(), "kubernetes.io", Value::Null)).is_ok());
}

#[test]
fn only_rs256_is_accepted() {
    let token = sign_with(&payload(), Algorithm::HS256, Some(TEST_KID));
    let refused = decode_claims(&token, &key(), ISSUER, AUDIENCE, now());
    assert!(matches!(refused, Err(IssuerTokenError::Invalid(_))));
}

#[tokio::test]
async fn verification_uses_the_issuers_keys_and_needs_a_kid() {
    let keys = JwksCache::fixed(jwks());
    assert!(
        verify_token(&keys, &sign(&payload()), ISSUER, AUDIENCE)
            .await
            .is_ok()
    );
    let other_kid = sign_with(&payload(), Algorithm::RS256, Some("rotated-away"));
    assert!(matches!(
        verify_token(&keys, &other_kid, ISSUER, AUDIENCE).await,
        Err(IssuerTokenError::UnknownKey)
    ));
    let no_kid = sign_with(&payload(), Algorithm::RS256, None);
    assert!(matches!(
        verify_token(&keys, &no_kid, ISSUER, AUDIENCE).await,
        Err(IssuerTokenError::UnknownKey)
    ));
}

#[tokio::test]
async fn inline_keys_verify_without_a_fetch() {
    let stored = serde_json::to_value(jwks()).expect("a key set");
    let keys = keys::inline(&stored).expect("a key set");
    assert!(
        verify_token(&keys, &sign(&payload()), ISSUER, AUDIENCE)
            .await
            .is_ok()
    );
    assert!(keys::inline(&json!({ "nope": 1 })).is_none());
}

#[test]
fn the_unverified_issuer_is_read_and_nothing_else_is_trusted() {
    assert_eq!(
        unverified_issuer(&sign(&payload())).as_deref(),
        Some(ISSUER)
    );
    for garbage in ["", "a", "a.b.c", "a.!!!.c", "a.e30.c"] {
        assert_eq!(unverified_issuer(garbage), None, "{garbage}");
    }
}

#[test]
fn what_is_recorded_is_the_claims_and_never_the_token() {
    let recorded = decoded(&payload()).expect("verifies").recorded();
    assert_eq!(
        recorded["sub"],
        "system:serviceaccount:kestra:kestra-worker"
    );
    assert_eq!(recorded["jti"], "8f1c…");
    assert_eq!(
        recorded["claims"]["kubernetes.io"]["pod"]["name"],
        "kestra-worker-7d9"
    );
    let text = recorded.to_string();
    assert!(!text.contains("eyJ"), "no JWT in {text}");
}

#[test]
fn a_large_claim_set_is_recorded_as_its_short_scalars() {
    let big = with(payload(), "blob", json!("x".repeat(RECORDED_MAX_BYTES)));
    let recorded = decoded(&big).expect("verifies").recorded();
    assert!(recorded["claims"].get("blob").is_none());
    assert!(recorded["claims"].get("kubernetes.io").is_none());
    assert_eq!(
        recorded["claims"]["sub"],
        "system:serviceaccount:kestra:kestra-worker"
    );
}
