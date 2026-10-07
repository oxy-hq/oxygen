//! What a token route answers when it refuses: `{ "error", "code"? }`, with
//! the codes the tokens HTTP contract names.

use axum::Json;
use axum::http::StatusCode;
use axum::http::header::RETRY_AFTER;
use axum::response::{IntoResponse, Response};
use oxy_auth::ExtendError;
use oxy_auth::token::access::Invalid;
use oxy_shared::errors::OxyError;
use serde_json::json;

#[derive(Debug)]
pub enum TokenError {
    /// 400 — a body that cannot be honoured, with the reason.
    Invalid(String),
    /// 403 `standing_required` — `platform` or `partner` asked for without
    /// holding it. Names the flag.
    StandingRequired(&'static str),
    /// 403 `unbounded_grant_required` — a staff route over credentials that
    /// reach the whole deployment, called with a platform grant bounded to
    /// some orgs. Decided from the caller alone, before any token is read.
    UnboundedGrantRequired,
    /// 404 — not the caller's, or a grant on something they cannot reach. The
    /// two read the same, so a caller cannot probe what exists.
    NotFound,
    /// 404 `no_token` — the request is a browser session; there is no calling
    /// token to describe or revoke.
    NoToken,
    /// 409 `legacy_immutable` — a legacy key may only be renamed, extended and
    /// revoked.
    LegacyImmutable,
    /// 409 `revoked` — the token is revoked; nothing more happens to it.
    Revoked,
    /// 409 `name_taken` — the org already has a service account of this name.
    NameTaken,
    /// 409 `use_service_account_routes` — the org tried to end the reach of a
    /// token it owns itself; that is done from its service account.
    UseServiceAccountRoutes,
    /// 400 `invalid_code` — an `oxyc login` exchange that cannot be honoured:
    /// the code is unknown, spent or expired, or the verifier does not match.
    /// One answer for all of them, so a caller cannot tell which.
    InvalidCode,
    /// 400 `invalid_ticket` — a browser ticket that opens nothing: unknown,
    /// spent or expired, or its token has since been revoked or has lapsed.
    /// One answer for all of them, as for a login code.
    InvalidTicket,
    /// 403 `personal_token_required` — only a personal access token opens a
    /// browser session; a legacy key, a service account's token, a CI token
    /// and a sandbox agent token do not.
    PersonalTokenRequired,
    /// 422 `repository_unresolved` — a trust policy's `owner/repo` could not
    /// be resolved to GitHub's ids, and the body supplied none.
    RepositoryUnresolved,
    /// 400 `environment_required` — a trust policy names no environment while
    /// the org requires one.
    EnvironmentRequired,
    /// 400 `exceeds_policy` — the new expiry outlives the tightest lifetime cap
    /// among the orgs the token holds grants in. The body names the cap as
    /// `max_lifetime_days`.
    ExceedsPolicy { max_lifetime_days: i32 },
    /// 400 `invalid_sandbox_token` — a sandbox agent mint that cannot be
    /// honoured: no apps, too many, one named twice, a lifetime out of range,
    /// or a personal token's field. The body repeats the reason as `message`.
    InvalidSandboxToken(String),
    /// 404 `app_not_found` — a sandbox agent mint named an app the caller may
    /// not mint for. One that does not exist reads the same. The body names
    /// the app as `app_id`, as it was sent.
    AppNotFound(String),
    /// 409 `sandbox_token_fixed` — a sandbox agent token is never renamed,
    /// widened, extended or regenerated.
    SandboxTokenFixed,
    /// 429 `rate_limited` — a public route asked too often from one address.
    /// `retry_after_secs` is sent as `Retry-After`.
    RateLimited { retry_after_secs: u64 },
    /// 500. The detail is logged, never returned.
    Internal(String),
}

impl From<OxyError> for TokenError {
    fn from(e: OxyError) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<sea_orm::DbErr> for TokenError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<Invalid> for TokenError {
    fn from(e: Invalid) -> Self {
        Self::Invalid(e.0)
    }
}

impl From<ExtendError> for TokenError {
    fn from(e: ExtendError) -> Self {
        match e {
            ExtendError::NotFound => Self::NotFound,
            ExtendError::Revoked => Self::Revoked,
            ExtendError::Invalid(message) => Self::Invalid(message),
            ExtendError::Db(e) => e.into(),
        }
    }
}

impl TokenError {
    /// `(status, message, code)`.
    fn parts(&self) -> (StatusCode, String, Option<&'static str>) {
        match self {
            Self::Invalid(message) => (StatusCode::BAD_REQUEST, message.clone(), None),
            Self::StandingRequired(flag) => (
                StatusCode::FORBIDDEN,
                format!("'{flag}' needs a standing you do not hold"),
                Some("standing_required"),
            ),
            Self::UnboundedGrantRequired => (
                StatusCode::FORBIDDEN,
                "your staff access is limited to some organizations; this needs access to all \
                 of them"
                    .into(),
                Some("unbounded_grant_required"),
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, "not found".into(), None),
            Self::NoToken => (
                StatusCode::NOT_FOUND,
                "this request did not authenticate with an API token".into(),
                Some("no_token"),
            ),
            Self::LegacyImmutable => (
                StatusCode::CONFLICT,
                "a legacy key can only be renamed, extended or revoked".into(),
                Some("legacy_immutable"),
            ),
            Self::Revoked => (
                StatusCode::CONFLICT,
                "the token is revoked".into(),
                Some("revoked"),
            ),
            Self::NameTaken => (
                StatusCode::CONFLICT,
                "this organization already has a service account with that name".into(),
                Some("name_taken"),
            ),
            Self::UseServiceAccountRoutes => (
                StatusCode::CONFLICT,
                "this token belongs to a service account: revoke it from the account".into(),
                Some("use_service_account_routes"),
            ),
            Self::InvalidCode => (
                StatusCode::BAD_REQUEST,
                "the login code is invalid or has expired".into(),
                Some("invalid_code"),
            ),
            Self::InvalidTicket => (
                StatusCode::BAD_REQUEST,
                "the sign-in link is invalid, has been used or has expired".into(),
                Some("invalid_ticket"),
            ),
            Self::PersonalTokenRequired => (
                StatusCode::FORBIDDEN,
                "only a personal access token opens a browser session".into(),
                Some("personal_token_required"),
            ),
            Self::RepositoryUnresolved => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "the repository's ids could not be read from GitHub: check 'owner/repo', or \
                 send 'repository_id' and 'repository_owner_id'"
                    .into(),
                Some("repository_unresolved"),
            ),
            Self::EnvironmentRequired => (
                StatusCode::BAD_REQUEST,
                "a trust policy must name the deployment 'environment' its workflow runs in".into(),
                Some("environment_required"),
            ),
            Self::ExceedsPolicy { max_lifetime_days } => (
                StatusCode::BAD_REQUEST,
                format!(
                    "the expiry is past the {max_lifetime_days}-day token lifetime an \
                     organization this token reaches allows"
                ),
                Some("exceeds_policy"),
            ),
            Self::InvalidSandboxToken(message) => (
                StatusCode::BAD_REQUEST,
                message.clone(),
                Some("invalid_sandbox_token"),
            ),
            Self::AppNotFound(_) => (
                StatusCode::NOT_FOUND,
                "not found".into(),
                Some("app_not_found"),
            ),
            Self::SandboxTokenFixed => (
                StatusCode::CONFLICT,
                "a sandbox agent token cannot be changed, extended or regenerated: mint a new \
                 one and revoke this one"
                    .into(),
                Some("sandbox_token_fixed"),
            ),
            Self::RateLimited { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many requests; try again later".into(),
                Some("rate_limited"),
            ),
            Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".into(),
                None,
            ),
        }
    }
}

impl IntoResponse for TokenError {
    fn into_response(self) -> Response {
        if let Self::Internal(detail) = &self {
            tracing::error!(error = %detail, "api token request failed");
        }
        let (status, message, code) = self.parts();
        let mut body = match code {
            Some(code) => json!({ "error": message, "code": code }),
            None => json!({ "error": message }),
        };
        match self {
            Self::ExceedsPolicy { max_lifetime_days } => {
                body["max_lifetime_days"] = json!(max_lifetime_days);
            }
            Self::InvalidSandboxToken(message) => {
                body["message"] = json!(message);
            }
            Self::AppNotFound(app_id) => {
                body["app_id"] = json!(app_id);
            }
            Self::RateLimited { retry_after_secs } => {
                let mut response = (status, Json(body)).into_response();
                if let Ok(value) = retry_after_secs.to_string().parse() {
                    response.headers_mut().insert(RETRY_AFTER, value);
                }
                return response;
            }
            _ => {}
        }
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
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
}
