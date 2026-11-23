//! `mint_token`: the claims go on the wire only when asked for, each echo is
//! read back, and Airhouse's refusals map to typed errors.

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::*;

fn mint_response_json() -> serde_json::Value {
    json!({
        "username": "eph_a1b2c3d4ef98",
        "password": "tk_3e8b1f4d2c5a6b7e8d9f0a1b2c3d4e5f",
        "tenant": "acme",
        "role": "reader",
        "expires_at": "2026-05-05T12:15:00Z",
        "service_account_id": "sa_8f2a8c10ab12cd34"
    })
}

#[tokio::test]
async fn test_mint_token_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&mint_response_json()))
        .mount(&server)
        .await;

    let client = AirhouseAdminClient::new(server.uri(), "tok");
    let cred = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "user_alice",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap();
    assert!(cred.username.starts_with("eph_"));
    assert!(cred.password.starts_with("tk_"));
    assert_eq!(cred.tenant, "acme");
    assert_eq!(cred.service_account_id, "sa_8f2a8c10ab12cd34");
}

/// `shared_session` goes on the wire only when asked for, so an ordinary
/// mint is byte-identical to one from before the claim existed; the echo
/// comes back on the credential.
#[tokio::test]
async fn test_mint_token_sends_shared_session_only_when_asked() {
    let server = MockServer::start().await;
    let mut echoed = mint_response_json();
    echoed["shared_session"] = json!(true);
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .and(wiremock::matchers::body_partial_json(
            json!({"shared_session": true}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(&echoed))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&mint_response_json()))
        .mount(&server)
        .await;

    let client = AirhouseAdminClient::new(server.uri(), "tok");
    let shared = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "pool",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: true,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap();
    assert!(shared.shared_session);
    let plain = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "alice",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap();
    assert!(!plain.shared_session);

    let bodies: Vec<serde_json::Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[0]["shared_session"], json!(true));
    assert!(bodies[1].get("shared_session").is_none(), "{}", bodies[1]);
}

/// `uncached_reads` goes on the wire only when asked for, and its echo comes
/// back on the credential.
#[tokio::test]
async fn test_mint_token_sends_uncached_reads_only_when_asked() {
    let server = MockServer::start().await;
    let mut echoed = mint_response_json();
    echoed["uncached_reads"] = json!(true);
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .and(wiremock::matchers::body_partial_json(
            json!({"uncached_reads": true}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(&echoed))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&mint_response_json()))
        .mount(&server)
        .await;

    let client = AirhouseAdminClient::new(server.uri(), "tok");
    let watch = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "watch",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: true,
                read_only: false,
            },
        )
        .await
        .unwrap();
    assert!(watch.uncached_reads);
    let plain = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "alice",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap();
    assert!(!plain.uncached_reads);

    let bodies: Vec<serde_json::Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[0]["uncached_reads"], json!(true));
    assert!(bodies[1].get("uncached_reads").is_none(), "{}", bodies[1]);
}

/// `read_only` goes on the wire only when asked for, and its echo comes
/// back on the credential.
#[tokio::test]
async fn test_mint_token_sends_read_only_only_when_asked() {
    let server = MockServer::start().await;
    let mut echoed = mint_response_json();
    echoed["read_only"] = json!(true);
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .and(wiremock::matchers::body_partial_json(
            json!({"read_only": true}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(&echoed))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&mint_response_json()))
        .mount(&server)
        .await;

    let client = AirhouseAdminClient::new(server.uri(), "tok");
    let watch = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "watch",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: true,
            },
        )
        .await
        .unwrap();
    assert!(watch.read_only);
    let plain = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "alice",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap();
    assert!(!plain.read_only);

    let bodies: Vec<serde_json::Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[0]["read_only"], json!(true));
    assert!(bodies[1].get("read_only").is_none(), "{}", bodies[1]);
}

#[tokio::test]
async fn test_mint_token_429_maps_to_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .respond_with(ResponseTemplate::new(429).set_body_string("mint rate limit exceeded"))
        .mount(&server)
        .await;

    let client = AirhouseAdminClient::new(server.uri(), "tok");
    let err = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "user_alice",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AirhouseError::RateLimited(_)));
}

#[tokio::test]
async fn test_mint_token_403_maps_to_forbidden() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string(
                "requested ttl_secs 7200 exceeds service-account max_ttl_secs 3600",
            ),
        )
        .mount(&server)
        .await;

    let client = AirhouseAdminClient::new(server.uri(), "tok");
    let err = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "user_alice",
            UserRole::Admin,
            7200,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AirhouseError::Forbidden(_)));
}

#[tokio::test]
async fn test_mint_token_401_maps_to_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let client = AirhouseAdminClient::new(server.uri(), "tok");
    let err = client
        .mint_token(
            "acme",
            "ahsa_revoked",
            "user_alice",
            UserRole::Reader,
            900,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AirhouseError::Unauthorized(_)));
}

#[tokio::test]
async fn test_mint_token_400_maps_to_invalid_input() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/acme/tokens"))
        .respond_with(ResponseTemplate::new(400).set_body_string("ttl_secs must be positive"))
        .mount(&server)
        .await;

    let client = AirhouseAdminClient::new(server.uri(), "tok");
    let err = client
        .mint_token(
            "acme",
            "ahsa_secret",
            "user_alice",
            UserRole::Reader,
            0,
            MintClaims {
                write_schemas: None,
                shared_session: false,
                uncached_reads: false,
                read_only: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AirhouseError::InvalidInput(_)));
}
