//! `GET /api/customer-apps/{id}/environments/{name}/outbox?limit=N` — the
//! email and notifications an environment captured instead of sending
//! (`custom_apps_outbox`), newest first.
//!
//! The same door as `…/tail`, which reads the same function output
//! (`custom_apps_tail::admission`): the app's admins with non-production
//! reach, and a sandbox agent token for a sandbox it created. Production has
//! no outbox: `409 production_has_no_planes`.
//!
//! `?limit=` is a whole number of at least 1; more than
//! [`MAX_LIMIT`](crate::custom_apps_outbox::MAX_LIMIT) answers that many.
//! The answer is `{"messages": [...], "has_more": bool}`.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use serde::Deserialize;
use uuid::Uuid;

use super::SandboxError;
use crate::custom_apps_outbox::{self as outbox, DEFAULT_LIMIT, MAX_LIMIT};
use crate::custom_apps_tail::admission::{self, TailError};

#[derive(Debug, Deserialize)]
pub struct OutboxQuery {
    /// Taken as text so a malformed value is this route's `invalid_limit`,
    /// not the framework's plain-text rejection.
    limit: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum OutboxError {
    #[error(transparent)]
    Refused(#[from] TailError),
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error("limit must be a whole number of at least 1 (at most {MAX_LIMIT} are answered)")]
    InvalidLimit,
    #[error("the outbox could not be read; retry shortly")]
    Unavailable,
}

impl IntoResponse for OutboxError {
    fn into_response(self) -> Response {
        let message = self.to_string();
        let (status, code) = match self {
            Self::Refused(refused) => return refused.into_response(),
            Self::Sandbox(refused) => return refused.into_response(),
            Self::InvalidLimit => (StatusCode::BAD_REQUEST, "invalid_limit"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "outbox_unavailable"),
        };
        (
            status,
            Json(serde_json::json!({ "error": code, "message": message })),
        )
            .into_response()
    }
}

/// `limit`, when given, as a count in range; more than the maximum is the
/// maximum.
fn parse_limit(raw: Option<&str>) -> Result<usize, OutboxError> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(DEFAULT_LIMIT);
    };
    match raw.parse::<usize>() {
        Ok(0) | Err(_) => Err(OutboxError::InvalidLimit),
        Ok(limit) => Ok(limit.min(MAX_LIMIT)),
    }
}

/// `GET /api/customer-apps/{id}/environments/{name}/outbox`
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path((id, name)): Path<(Uuid, String)>,
    Query(query): Query<OutboxQuery>,
) -> Result<Json<outbox::Page>, OutboxError> {
    // Syntax only, so it says nothing about the app to a caller the gates
    // below would turn away.
    let limit = parse_limit(query.limit.as_deref())?;
    let admitted = admission::admit(&user, marker.is_some(), id, &name).await?;
    let environment = AppEnvironment::parse(&admitted.target.environment)
        .ok_or_else(|| TailError::InvalidEnvironment(name.clone()))?;
    if environment == AppEnvironment::Production {
        return Err(SandboxError::ProductionHasNoPlanes.into());
    }
    outbox::list_newest_first(
        &outbox::silo(id, &environment),
        limit,
        admitted.target.since,
    )
    .await
    .map(Json)
    .map_err(|error| {
        tracing::error!(%error, app_id = %id, %name, "outbox: reading failed");
        OutboxError::Unavailable
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absent or blank is the default; a count in range is itself; too many
    /// is the maximum; zero and anything that is not a whole number are
    /// refused rather than read as the default.
    #[test]
    fn a_limit_is_a_count_in_range() {
        assert_eq!(parse_limit(None).unwrap(), DEFAULT_LIMIT);
        assert_eq!(parse_limit(Some(" ")).unwrap(), DEFAULT_LIMIT);
        assert_eq!(parse_limit(Some("7")).unwrap(), 7);
        assert_eq!(parse_limit(Some("200")).unwrap(), MAX_LIMIT);
        assert_eq!(parse_limit(Some("5000")).unwrap(), MAX_LIMIT);
        for bad in ["0", "-1", "1.5", "many"] {
            assert!(
                matches!(parse_limit(Some(bad)), Err(OutboxError::InvalidLimit)),
                "{bad}"
            );
        }
    }
}
