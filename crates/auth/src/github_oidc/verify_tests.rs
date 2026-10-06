use jsonwebtoken::Algorithm;
use serde_json::json;

use super::*;
use crate::github_oidc::claims::{AUDIENCE_OXY, AUDIENCE_PUBLISH};
use crate::github_oidc::test_support::{TEST_KID, jwks, payload, sign, sign_with};

fn key() -> DecodingKey {
    let jwks = jwks();
    DecodingKey::from_jwk(jwks.find(TEST_KID).expect("the test key")).expect("a decoding key")
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[test]
fn a_token_for_the_required_audience_decodes() {
    let token = sign(&payload(AUDIENCE_OXY));
    let claims = decode_claims(&token, &key(), AUDIENCE_OXY).expect("a good token");
    assert_eq!(claims.repository, "acme/app");
    assert_eq!(claims.repo_id(), Some(987));
    assert_eq!(claims.git_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(claims.run_id.as_deref(), Some("7001"));
}

#[test]
fn a_publish_audience_token_is_refused_at_the_new_exchange() {
    // A token a workflow requested for a publish cannot be traded for the
    // broader credential.
    let token = sign(&payload(AUDIENCE_PUBLISH));
    let refused = decode_claims(&token, &key(), AUDIENCE_OXY).unwrap_err();
    assert!(matches!(refused, OidcError::WrongAudience), "{refused:?}");
    assert_eq!(refused.code(), Some("wrong_audience"));
}

#[test]
fn an_oxy_audience_token_is_refused_at_the_legacy_exchange() {
    // And the reverse: the legacy exchange keeps requiring its own.
    let token = sign(&payload(AUDIENCE_OXY));
    let refused = decode_claims(&token, &key(), AUDIENCE_PUBLISH).unwrap_err();
    assert!(matches!(refused, OidcError::WrongAudience), "{refused:?}");
    // The legacy audience still verifies on the legacy route.
    let token = sign(&payload(AUDIENCE_PUBLISH));
    assert!(decode_claims(&token, &key(), AUDIENCE_PUBLISH).is_ok());
}

#[test]
fn githubs_default_audience_is_refused_by_both() {
    let token = sign(&payload("https://github.com/acme"));
    for audience in [AUDIENCE_OXY, AUDIENCE_PUBLISH] {
        assert!(matches!(
            decode_claims(&token, &key(), audience),
            Err(OidcError::WrongAudience)
        ));
    }
}

#[test]
fn an_expired_token_is_refused_past_the_leeway() {
    let mut expired = payload(AUDIENCE_OXY);
    expired["exp"] = json!(now() - 120);
    let refused = decode_claims(&sign(&expired), &key(), AUDIENCE_OXY).unwrap_err();
    assert!(matches!(refused, OidcError::Expired), "{refused:?}");
    assert_eq!(refused.code(), Some("expired"));

    // Ten seconds past `exp` is inside the 30 s leeway.
    let mut just_past = payload(AUDIENCE_OXY);
    just_past["exp"] = json!(now() - 10);
    assert!(decode_claims(&sign(&just_past), &key(), AUDIENCE_OXY).is_ok());
}

#[test]
fn a_token_not_yet_valid_is_refused_past_the_leeway() {
    let mut early = payload(AUDIENCE_OXY);
    early["nbf"] = json!(now() + 120);
    let refused = decode_claims(&sign(&early), &key(), AUDIENCE_OXY).unwrap_err();
    assert!(matches!(refused, OidcError::InvalidToken(_)), "{refused:?}");
    assert_eq!(refused.code(), Some("invalid_token"));

    // Ten seconds early is inside the leeway.
    let mut nearly = payload(AUDIENCE_OXY);
    nearly["nbf"] = json!(now() + 10);
    assert!(decode_claims(&sign(&nearly), &key(), AUDIENCE_OXY).is_ok());
}

#[test]
fn another_issuer_is_refused() {
    let mut other = payload(AUDIENCE_OXY);
    other["iss"] = json!("https://token.actions.githubusercontent.com.evil.example");
    assert!(matches!(
        decode_claims(&sign(&other), &key(), AUDIENCE_OXY),
        Err(OidcError::InvalidToken(_))
    ));
}

#[test]
fn only_rs256_is_accepted_whatever_the_header_claims() {
    // Signed with a shared secret under an HS256 header: the header's `alg` is
    // never what picks the algorithm.
    let token = sign_with(&payload(AUDIENCE_OXY), Algorithm::HS256, Some(TEST_KID));
    assert!(matches!(
        decode_claims(&token, &key(), AUDIENCE_OXY),
        Err(OidcError::InvalidToken(_))
    ));
}

#[test]
fn a_tampered_payload_fails_the_signature() {
    let token = sign(&payload(AUDIENCE_OXY));
    let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
    // Swap in the payload of a token for another repository.
    let mut other = payload(AUDIENCE_OXY);
    other["repository_id"] = json!("1");
    let forged = sign(&other);
    parts[1] = forged.split('.').nth(1).expect("a payload").to_string();
    let tampered = parts.join(".");
    assert!(matches!(
        decode_claims(&tampered, &key(), AUDIENCE_OXY),
        Err(OidcError::InvalidToken(_))
    ));
}

#[test]
fn a_token_missing_a_required_claim_is_refused() {
    for claim in [
        "exp",
        "iat",
        "iss",
        "aud",
        "jti",
        "repository",
        "event_name",
    ] {
        let mut partial = payload(AUDIENCE_OXY);
        partial.as_object_mut().expect("an object").remove(claim);
        assert!(
            decode_claims(&sign(&partial), &key(), AUDIENCE_OXY).is_err(),
            "a token without {claim} must not verify"
        );
    }
}

#[tokio::test]
async fn a_token_with_no_kid_or_an_unknown_one_never_reaches_the_database() {
    // A disconnected handle: reaching the `jti` burn would error as a database
    // failure, not as the refusals asserted here.
    let db = sea_orm::DatabaseConnection::default();
    let keys = JwksCache::fixed(jwks());

    let no_kid = sign_with(&payload(AUDIENCE_OXY), Algorithm::RS256, None);
    assert!(matches!(
        verify_token(&db, &keys, &no_kid, AUDIENCE_OXY).await,
        Err(OidcError::UnknownKey)
    ));
    let unknown = sign_with(&payload(AUDIENCE_OXY), Algorithm::RS256, Some("rotated"));
    assert!(matches!(
        verify_token(&db, &keys, &unknown, AUDIENCE_OXY).await,
        Err(OidcError::UnknownKey)
    ));
    assert!(matches!(
        verify_token(&db, &keys, "not-a-jwt", AUDIENCE_OXY).await,
        Err(OidcError::InvalidToken(_))
    ));
    // The wrong audience is refused before the `jti` is spent.
    let publish = sign(&payload(AUDIENCE_PUBLISH));
    assert!(matches!(
        verify_token(&db, &keys, &publish, AUDIENCE_OXY).await,
        Err(OidcError::WrongAudience)
    ));
}

#[test]
fn a_failure_of_ours_has_no_refusal_code() {
    assert_eq!(OidcError::Db("down".into()).code(), None);
    assert_eq!(OidcError::Replayed.code(), Some("replayed"));
    assert_eq!(OidcError::UnknownKey.code(), Some("invalid_token"));
    assert_eq!(OidcError::JwksUnavailable.code(), Some("invalid_token"));
}
