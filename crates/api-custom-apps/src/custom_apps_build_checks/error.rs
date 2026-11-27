//! Why a checks route did not answer with checks.
//!
//! A refusal with a reason is `{"code": "...", "message": "..."}` at the top
//! level, under the status the reason has; a caller who may not see the build
//! is the bare not-found an unknown build gets.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::queue::RerunRefusal;

#[derive(Debug)]
pub enum ChecksError {
    /// An unknown app, build or attempt, and a caller who may not see one.
    NotFound,
    /// A rerun that was not queued (`409`).
    Refused(RerunRefusal),
    /// A request the route understood and will not carry out, with its
    /// machine-readable code.
    Coded {
        status: StatusCode,
        code: &'static str,
        message: String,
    },
    Internal(String),
}

impl ChecksError {
    /// `409 <code>`.
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::coded(StatusCode::CONFLICT, code, message)
    }

    /// `400 <code>`.
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::coded(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn coded(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            status,
            code,
            message: message.into(),
        }
    }

    /// The refusal's code, when it has one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Refused(RerunRefusal::Db(_)) | Self::NotFound | Self::Internal(_) => None,
            Self::Refused(refusal) => Some(refusal.code()),
            Self::Coded { code, .. } => Some(code),
        }
    }
}

impl IntoResponse for ChecksError {
    fn into_response(self) -> Response {
        let coded = |status: StatusCode, code: &str, message: String| {
            (status, Json(json!({ "code": code, "message": message }))).into_response()
        };
        match self {
            Self::NotFound => StatusCode::NOT_FOUND.into_response(),
            Self::Refused(RerunRefusal::Db(e)) | Self::Internal(e) => {
                tracing::error!(error = %e, "build checks route failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
            Self::Refused(refusal) => {
                coded(StatusCode::CONFLICT, refusal.code(), refusal.to_string())
            }
            Self::Coded {
                status,
                code,
                message,
            } => coded(status, code, message),
        }
    }
}

pub(crate) fn internal(e: impl std::fmt::Display) -> ChecksError {
    ChecksError::Internal(e.to_string())
}

impl From<sea_orm::DbErr> for ChecksError {
    fn from(e: sea_orm::DbErr) -> Self {
        internal(e)
    }
}
