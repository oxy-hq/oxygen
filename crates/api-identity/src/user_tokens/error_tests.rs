//! Each refusal of a token route, against the status and code the contract names.

use super::*;

#[test]
fn each_refusal_carries_the_contracts_status_and_code() {
    let cases = [
        (TokenError::Invalid("bad".into()), 400, None),
        (
            TokenError::StandingRequired("platform"),
            403,
            Some("standing_required"),
        ),
        (
            TokenError::UnboundedGrantRequired,
            403,
            Some("unbounded_grant_required"),
        ),
        (TokenError::NotFound, 404, None),
        (TokenError::NoToken, 404, Some("no_token")),
        (TokenError::LegacyImmutable, 409, Some("legacy_immutable")),
        (TokenError::Revoked, 409, Some("revoked")),
        (TokenError::NameTaken, 409, Some("name_taken")),
        (
            TokenError::UseServiceAccountRoutes,
            409,
            Some("use_service_account_routes"),
        ),
        (TokenError::InvalidCode, 400, Some("invalid_code")),
        (TokenError::InvalidTicket, 400, Some("invalid_ticket")),
        (
            TokenError::PersonalTokenRequired,
            403,
            Some("personal_token_required"),
        ),
        (
            TokenError::RepositoryUnresolved,
            422,
            Some("repository_unresolved"),
        ),
        (
            TokenError::EnvironmentRequired,
            400,
            Some("environment_required"),
        ),
        (
            TokenError::ExceedsPolicy {
                max_lifetime_days: 30,
            },
            400,
            Some("exceeds_policy"),
        ),
        (
            TokenError::RateLimited {
                retry_after_secs: 5,
            },
            429,
            Some("rate_limited"),
        ),
        (
            TokenError::InvalidSandboxToken("no apps".into()),
            400,
            Some("invalid_sandbox_token"),
        ),
        (
            TokenError::AppNotFound("an-app".into()),
            404,
            Some("app_not_found"),
        ),
        (
            TokenError::SandboxTokenFixed,
            409,
            Some("sandbox_token_fixed"),
        ),
        (
            TokenError::InvalidAgentToken("too long".into()),
            400,
            Some("invalid_agent_token"),
        ),
        (TokenError::AgentTokenFixed, 409, Some("agent_token_fixed")),
        (TokenError::Internal("db down".into()), 500, None),
    ];
    for (error, status, code) in cases {
        let (got_status, message, got_code) = error.parts();
        assert_eq!(got_status.as_u16(), status, "{error:?}");
        assert_eq!(got_code, code, "{error:?}");
        assert!(!message.contains("db down"), "the detail is never returned");
    }
}

#[tokio::test]
async fn exceeds_policy_names_the_cap_in_the_body() {
    let response = TokenError::ExceedsPolicy {
        max_lifetime_days: 30,
    }
    .into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "exceeds_policy");
    assert_eq!(body["max_lifetime_days"], 30);
}

async fn body_of(error: TokenError) -> (StatusCode, serde_json::Value) {
    let response = error.into_response();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn a_sandbox_mint_refusal_says_why_and_names_the_app() {
    let (status, body) = body_of(TokenError::InvalidSandboxToken("no apps".into())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_sandbox_token");
    // The contract's `message`, beside the `error` every refusal carries.
    assert_eq!(body["message"], "no apps");
    assert_eq!(body["error"], "no apps");

    let (status, body) = body_of(TokenError::AppNotFound("an-app".into())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "app_not_found");
    assert_eq!(body["app_id"], "an-app");

    let (status, body) = body_of(TokenError::SandboxTokenFixed).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "sandbox_token_fixed");
}

#[tokio::test]
async fn an_agent_mint_refusal_says_why_and_nothing_else() {
    let why = "'expires_in_hours' must be an integer from 1 to 168";
    let (status, body) = body_of(TokenError::InvalidAgentToken(why.into())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // The contract's body exactly: the reason and the code.
    assert_eq!(body, json!({ "error": why, "code": "invalid_agent_token" }));

    let (status, body) = body_of(TokenError::AgentTokenFixed).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "agent_token_fixed");
    assert!(body["error"].as_str().is_some_and(|e| e.contains("revoke")));
}

#[test]
fn a_rate_limit_says_when_to_retry() {
    let response = TokenError::RateLimited {
        retry_after_secs: 7,
    }
    .into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()[RETRY_AFTER], "7");
}

#[test]
fn an_extend_refusal_maps_onto_the_same_codes() {
    assert!(matches!(
        TokenError::from(ExtendError::Revoked),
        TokenError::Revoked
    ));
    assert!(matches!(
        TokenError::from(ExtendError::NotFound),
        TokenError::NotFound
    ));
    assert!(matches!(
        TokenError::from(ExtendError::Invalid("x".into())),
        TokenError::Invalid(_)
    ));
}
