//! How the endpoints answer: a JSON body, or a refusal as `{code, message}`
//! (the shape `oxyc` reads into its error), and the parsing every route
//! shares.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::DbErr;
use serde::Serialize;

/// Why a route refused.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Unauthenticated(String),
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{1}")]
    NotFound(&'static str, String),
    #[error("{1}")]
    BadRequest(&'static str, String),
    #[error("{1}")]
    Conflict(&'static str, String),
    /// The org's OLTP store could not be reached, so nothing was changed.
    #[error("{0}")]
    StoreUnreachable(String),
    /// Logged where it happened; the message never carries database text.
    #[error("{0}")]
    Internal(String),
}

impl ApiError {
    /// A database failure: logged with its detail, answered without it.
    pub fn db(what: &str, e: DbErr) -> Self {
        tracing::error!(error = %e, "app events api: {what} failed");
        Self::Internal(format!("could not {what}; retry shortly"))
    }

    /// A write while the org's `app_events` flag is off.
    pub fn not_enabled() -> Self {
        Self::Conflict(
            "events_not_enabled",
            "app events are off for this org: nothing is queued or changed until the \
             `app_events` flag is on"
                .into(),
        )
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(..) => StatusCode::NOT_FOUND,
            Self::BadRequest(..) => StatusCode::BAD_REQUEST,
            Self::Conflict(..) => StatusCode::CONFLICT,
            Self::StoreUnreachable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Unauthenticated(_) => "unauthenticated",
            Self::Forbidden(code)
            | Self::NotFound(code, _)
            | Self::BadRequest(code, _)
            | Self::Conflict(code, _) => code,
            Self::StoreUnreachable(_) => "events_store_unreachable",
            Self::Internal(_) => "internal",
        }
    }

    /// The refusal a status from the shared auth flow stands for.
    pub(super) fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::UNAUTHORIZED => Self::Unauthenticated("sign in first".into()),
            StatusCode::FORBIDDEN => Self::Forbidden("forbidden"),
            StatusCode::NOT_FOUND => Self::NotFound("app_not_found", "no such app".into()),
            _ => Self::Internal("could not authorize the request; retry shortly".into()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "code": self.code(), "message": self.to_string() });
        (self.status(), Json(body)).into_response()
    }
}

/// `reply` as JSON, or its refusal.
pub(super) fn respond<T: Serialize>(reply: Result<T, ApiError>) -> Response {
    match reply {
        Ok(body) => Json(body).into_response(),
        Err(refused) => refused.into_response(),
    }
}

/// `{count}`-shaped replies: what a write did, or would do on a dry run.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Counted {
    pub count: u64,
    /// Matching rows past this call's batch: run it again for them.
    pub remaining: u64,
    pub dry_run: bool,
}

/// The environment `raw` names: production when absent or blank, otherwise
/// exactly an app environment's name.
pub(super) fn environment(raw: Option<&str>) -> Result<AppEnvironment, ApiError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(AppEnvironment::Production),
        Some(name) => AppEnvironment::parse(name).ok_or_else(|| {
            ApiError::BadRequest(
                "invalid_environment",
                format!("`{name}` is not an environment: production, staging or dev-<handle>"),
            )
        }),
    }
}

/// An RFC 3339 time in `field`.
pub(super) fn time(field: &str, raw: &str) -> Result<DateTime<Utc>, ApiError> {
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| {
            ApiError::BadRequest(
                "invalid_time",
                format!("`{field}` is not an RFC 3339 date-time: {raw}"),
            )
        })
}

/// `<app>/<function>`, the way `oxyc events` names a listener.
pub(super) fn listener(raw: &str) -> Result<(String, String), ApiError> {
    match raw.split_once('/') {
        Some((app, function)) if !app.is_empty() && !function.is_empty() => {
            Ok((app.to_string(), function.to_string()))
        }
        _ => Err(ApiError::BadRequest(
            "invalid_listener",
            format!("`{raw}` is not a listener: name it as <app>/<function>"),
        )),
    }
}

/// The first line of `error`, trimmed: with the outcome, a dead letter's
/// signature.
pub(super) fn first_line(error: &str) -> &str {
    error.lines().next().unwrap_or_default().trim()
}
