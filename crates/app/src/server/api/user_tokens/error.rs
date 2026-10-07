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
    /// 400 `invalid_agent_token` — an agent token's mint that cannot be
    /// honoured: a lifetime out of range, a name past the limit, an unknown
    /// field, or a field of another kind of token. The reason is the `error`.
    InvalidAgentToken(String),
    /// 409 `agent_token_fixed` — an agent token is never renamed, widened,
    /// extended or regenerated.
    AgentTokenFixed,
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
            Self::InvalidAgentToken(message) => (
                StatusCode::BAD_REQUEST,
                message.clone(),
                Some("invalid_agent_token"),
            ),
            Self::AgentTokenFixed => (
                StatusCode::CONFLICT,
                "an agent token cannot be changed, extended or regenerated: mint a new one and \
                 revoke this one"
                    .into(),
                Some("agent_token_fixed"),
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
#[path = "error_tests.rs"]
mod tests;
