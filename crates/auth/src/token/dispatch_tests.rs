//! Unit tests for the dispatch order — everything decidable without a database.
//!
//! The DB-backed half (a real key or token resolving) lives in oxy-app's
//! `crates/server/tests/integration/token_auth`.

use super::*;
use crate::session_key::test_key;
use crate::token::format::generate_personal;
use axum::http::HeaderValue;
use jsonwebtoken::{EncodingKey, Header, encode};

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (name, value) in pairs {
        h.insert(*name, HeaderValue::from_str(value).unwrap());
    }
    h
}

#[derive(serde::Serialize)]
struct Claims {
    sub: String,
    email: String,
    exp: usize,
    iat: usize,
}

fn session_jwt(user_id: uuid::Uuid) -> String {
    let now = chrono::Utc::now().timestamp() as usize;
    encode(
        &Header::default(),
        &Claims {
            sub: user_id.to_string(),
            email: "ada@acme.com".into(),
            exp: now + 3600,
            iat: now,
        },
        &EncodingKey::from_secret(&test_key(Purpose::Session)),
    )
    .unwrap()
}

/// A token that claims `oxy_pat_` but fails its checksum — refused before
/// any database work, which is what lets these tests run without one.
fn broken_pat() -> String {
    let mut t = generate_personal().plaintext;
    let last = t.pop().expect("non-empty");
    t.push(if last == '0' { '1' } else { '0' });
    t
}

#[test]
fn a_new_prefix_bearer_is_found() {
    let pat = generate_personal().plaintext;
    let h = headers(&[("authorization", &format!("Bearer {pat}"))]);
    assert_eq!(new_prefix_token(&h), Some((pat, TokenFormat::Personal)));
}

#[test]
fn a_new_prefix_x_api_key_is_found() {
    let pat = generate_personal().plaintext;
    let h = headers(&[("x-api-key", &pat)]);
    assert_eq!(new_prefix_token(&h), Some((pat, TokenFormat::Personal)));
}

#[test]
fn bearer_wins_when_both_carry_a_new_prefix() {
    let a = generate_personal().plaintext;
    let b = generate_personal().plaintext;
    let h = headers(&[("authorization", &format!("bearer {a}")), ("x-api-key", &b)]);
    assert_eq!(new_prefix_token(&h).map(|(t, _)| t), Some(a));
}

#[test]
fn a_jwt_or_legacy_value_is_not_a_new_prefix() {
    let h = headers(&[
        ("authorization", &session_jwt(uuid::Uuid::new_v4())),
        ("x-api-key", "oxy_0123456789abcdef0123456789abcdef"),
    ]);
    assert_eq!(new_prefix_token(&h), None);
}

/// A removed legacy key is recognised in either header, so the 401 can say
/// what happened. Nothing else is: an arbitrary `X-API-Key` value used to be
/// looked up raw and now is not, which is the whole point.
#[test]
fn a_removed_legacy_key_is_recognised_in_either_header() {
    let key = "oxy_0123456789abcdef0123456789abcdef";
    for h in [
        headers(&[("x-api-key", key)]),
        headers(&[("authorization", &format!("Bearer {key}"))]),
    ] {
        assert!(presents_removed_legacy_key(&h));
    }
}

#[test]
fn nothing_else_reads_as_a_removed_legacy_key() {
    for value in [
        "Bearer not-a-key".to_string(),
        session_jwt(uuid::Uuid::new_v4()),
        format!("Bearer oxypublish_{}", "a".repeat(64)),
        format!("Bearer {}", generate_personal().plaintext),
    ] {
        let h = headers(&[("authorization", &value)]);
        assert!(!presents_removed_legacy_key(&h), "{value}");
    }
    for value in ["   ", "some-fixture-key", "oxy_short"] {
        let h = headers(&[("x-api-key", value)]);
        assert!(!presents_removed_legacy_key(&h), "{value}");
    }
}

/// The one thing a caller still holding a legacy key must get: a 401 that
/// names the replacement, not "no API key found in headers".
#[tokio::test]
async fn a_legacy_key_is_refused_by_name() {
    crate::built_in::set_auth_configured(true);
    let key = "oxy_0123456789abcdef0123456789abcdef";
    for h in [
        headers(&[("x-api-key", key)]),
        headers(&[("authorization", &format!("Bearer {key}"))]),
    ] {
        for surface in [AuthSurface::Session, AuthSurface::ApiKeyOnly] {
            let err = authenticate_request(
                &crate::ports::test_platform(),
                &h,
                surface,
                SandboxAgent::Refuse,
            )
            .await
            .expect_err("a legacy key authenticates nothing");
            let OxyError::AuthenticationError(message) = err else {
                panic!("expected an authentication error");
            };
            assert_eq!(message, LEGACY_KEY_REMOVED);
        }
    }
}

#[tokio::test]
async fn a_failing_new_prefix_token_does_not_fall_through_to_a_valid_cookie() {
    crate::built_in::set_auth_configured(true);
    let cookie = format!("oxy_session={}", session_jwt(uuid::Uuid::new_v4()));
    for (name, value) in [
        ("authorization", format!("Bearer {}", broken_pat())),
        ("x-api-key", broken_pat()),
    ] {
        let h = headers(&[("cookie", &cookie), (name, &value)]);
        let err = authenticate_request(
            &crate::ports::test_platform(),
            &h,
            AuthSurface::Session,
            SandboxAgent::Refuse,
        )
        .await
        .expect_err("a bad oxy_pat_ must 401 even beside a valid cookie");
        assert!(matches!(err, OxyError::AuthenticationError(_)), "{err}");
    }
}

#[tokio::test]
async fn a_failing_new_prefix_x_api_key_beats_a_valid_jwt_header() {
    crate::built_in::set_auth_configured(true);
    let h = headers(&[
        ("authorization", &session_jwt(uuid::Uuid::new_v4())),
        ("x-api-key", &broken_pat()),
    ]);
    assert!(
        authenticate_request(
            &crate::ports::test_platform(),
            &h,
            AuthSurface::Session,
            SandboxAgent::Refuse
        )
        .await
        .is_err()
    );
}

/// A session is tried before the request is refused for carrying a removed
/// key, so a browser that still has a stale `X-API-Key` lying around is not
/// locked out of its own cookie.
#[tokio::test]
async fn a_valid_session_still_beats_a_removed_legacy_key() {
    crate::built_in::set_auth_configured(true);
    let user = uuid::Uuid::new_v4();
    let cookie = format!("oxy_session={}", session_jwt(user));
    let h = headers(&[
        ("cookie", &cookie),
        ("x-api-key", "oxy_0123456789abcdef0123456789abcdef"),
    ]);
    let (identity, credential) = authenticate_request(
        &crate::ports::test_platform(),
        &h,
        AuthSurface::Session,
        SandboxAgent::Refuse,
    )
    .await
    .expect("cookie wins");
    assert_eq!(identity.user_id, Some(user));
    assert!(credential.is_none(), "a session is not a credential");
}

#[tokio::test]
async fn the_api_key_only_surface_never_accepts_a_session() {
    crate::built_in::set_auth_configured(true);
    let jwt = session_jwt(uuid::Uuid::new_v4());
    for h in [
        headers(&[("authorization", &format!("Bearer {jwt}"))]),
        headers(&[("cookie", &format!("oxy_session={jwt}"))]),
    ] {
        let err = authenticate_request(
            &crate::ports::test_platform(),
            &h,
            AuthSurface::ApiKeyOnly,
            SandboxAgent::Refuse,
        )
        .await
        .expect_err("no session on /external/api");
        assert!(matches!(err, OxyError::AuthenticationError(_)));
    }
}

#[test]
fn an_entry_point_that_has_not_admitted_it_refuses_a_sandbox_agent_token_and_nothing_else() {
    // The gate itself. Every other format passes it whatever the entry point
    // says, so the opt-in changes nothing for any existing credential.
    for format in [
        TokenFormat::Personal,
        TokenFormat::ServiceAccount,
        TokenFormat::Ci,
        TokenFormat::LegacyPublish,
    ] {
        for admitted in [SandboxAgent::Admit, SandboxAgent::Refuse] {
            assert!(
                refuse_sandbox_agent(format, admitted).is_ok(),
                "{format:?} {admitted:?}"
            );
        }
    }
    assert!(refuse_sandbox_agent(TokenFormat::SandboxAgent, SandboxAgent::Admit).is_ok());
    let refused = refuse_sandbox_agent(TokenFormat::SandboxAgent, SandboxAgent::Refuse);
    assert!(
        matches!(refused, Err(OxyError::AuthenticationError(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_well_formed_sandbox_agent_token_is_refused_on_both_surfaces_in_both_headers() {
    // A token this server could have minted, checksum and all — refused where
    // the entry point has not admitted the kind, even beside a valid cookie.
    crate::built_in::set_auth_configured(true);
    let token = crate::token::format::generate_sandbox_agent().plaintext;
    assert!(verify_checksum(&token));
    let cookie = format!("oxy_session={}", session_jwt(uuid::Uuid::new_v4()));
    for surface in [AuthSurface::Session, AuthSurface::ApiKeyOnly] {
        for (name, value) in [
            ("authorization", format!("Bearer {token}")),
            ("x-api-key", token.clone()),
        ] {
            let h = headers(&[("cookie", &cookie), (name, &value)]);
            let err = authenticate_request(
                &crate::ports::test_platform(),
                &h,
                surface,
                SandboxAgent::Refuse,
            )
            .await
            .expect_err("an oxy_sbx_ token is refused, and never falls through");
            assert!(matches!(err, OxyError::AuthenticationError(_)), "{err}");
        }
    }
}

#[test]
fn presenting_an_api_token_is_read_off_the_prefix_in_either_header() {
    use crate::token::format::{generate_ci, generate_sandbox_agent, generate_service_account};

    // Every new format, in the bearer and in the API-key header.
    for (kind, token) in [
        ("personal", generate_personal().plaintext),
        ("service_account", generate_service_account().plaintext),
        ("ci", generate_ci().plaintext),
        ("sandbox_agent", generate_sandbox_agent().plaintext),
    ] {
        let bearer = format!("Bearer {token}");
        for h in [
            headers(&[("authorization", &bearer)]),
            headers(&[("x-api-key", &token)]),
        ] {
            assert!(presents_api_token(&h), "{kind}");
            assert_eq!(presents_sandbox_agent(&h), kind == "sandbox_agent");
        }
    }

    // Everything else is not one: nothing, a session, a removed legacy key, a
    // publish token. Their requests are never counted or wrapped on the serve
    // tree.
    let legacy = "oxy_0123456789abcdef0123456789abcdef".to_string();
    let legacy_bearer = format!("Bearer {legacy}");
    let session = format!("oxy_session={}", session_jwt(uuid::Uuid::new_v4()));
    let jwt = format!("Bearer {}", session_jwt(uuid::Uuid::new_v4()));
    let publish = format!("Bearer oxypublish_{}", "ab".repeat(24));
    for (what, h) in [
        ("anonymous", headers(&[])),
        ("a session cookie", headers(&[("cookie", &session)])),
        ("a session bearer", headers(&[("authorization", &jwt)])),
        ("a legacy key", headers(&[("x-api-key", &legacy)])),
        (
            "a legacy key as a bearer",
            headers(&[("authorization", &legacy_bearer)]),
        ),
        ("a publish token", headers(&[("authorization", &publish)])),
    ] {
        assert!(!presents_api_token(&h), "{what}");
    }
}
