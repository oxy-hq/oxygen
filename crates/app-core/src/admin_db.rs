//! The staff console's connect-on-demand database handle and its JSON error
//! body (`{ code, message }`), so every admin section — in `oxy-app` or out of
//! it — answers a failed connect or query the same way.
//!
//! Down from `oxy-app`'s `admin::internal_jobs` in the admin split's PR 3 (it
//! was the prelude's `surface::admin_db`): a staff section on any surface takes
//! it from here. Database access is on demand via
//! `oxy::database::client::establish_connection()`; the console deliberately
//! puts no pool on `AppState`. The log lines keep the `internal-jobs:` wording
//! they had there.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sea_orm::{DatabaseConnection, DbErr};
use serde::Serialize;

// Connect-on-demand helper

pub async fn connect() -> Result<DatabaseConnection, Response> {
    oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            tracing::error!(?e, "internal-jobs: DB connect failed");
            error_body(
                StatusCode::SERVICE_UNAVAILABLE,
                "db_unavailable",
                Some("Database connection failed".into()),
            )
        })
}

// Error helpers

/// Shared error body shape for every admin write route that reuses `connect`
/// / `db_err` from this module (`oxy-api-admin`'s `internal_jobs`, plus the admin
/// sections that left it: `oxy-api-airway`'s write handlers and the rest), so a
/// validation-style 400 elsewhere doesn't have to invent a one-off shape the
/// frontend would need to special-case. Built through [`error_body`].
#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

pub fn error_body(status: StatusCode, code: &'static str, message: Option<String>) -> Response {
    (status, Json(ErrorBody { code, message })).into_response()
}

pub fn db_err(e: DbErr) -> Response {
    tracing::error!(?e, "internal-jobs: DB error");
    error_body(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db_error",
        Some(e.to_string()),
    )
}
