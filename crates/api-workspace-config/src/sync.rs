//! `POST /api/{workspace_id}/databases/sync`: introspect one configured
//! database and return the `.databases/` files a sync writes, for `oxyc sync`
//! to write into the caller's checkout (it was `oxy sync`, which wrote them
//! itself; `internal-docs/oxy-cli-to-oxyc.md`).
//!
//! The file format is `oxy::semantic::render`'s; this handler only picks the
//! database and maps the outcome to a status. It writes no server state and
//! reads the served revision's compiled config through
//! `WorkspaceManagerReadOnly`, so any replica answers (FleetOk). A database
//! whose connection needs a workspace file (a DuckDB file, a BigQuery
//! `key_path`) says so through `try_resolve_file` and answers 400 on a node
//! without one; `key_var` and the managed types do not.
//!
//! One database per request, awaited inline. Introspection is one
//! `INFORMATION_SCHEMA` query per dataset plus the DDL query, measured at
//! 3.6s for a BigQuery dataset and 0.3s for a DuckDB file (CI, 2026-11-21),
//! well inside the global 60s layer; a request that outgrows it answers 408
//! and is narrowed with `datasets`. Nothing is queued, retried or persisted,
//! so there is no `TaskSpec` to make: a rerun is the retry.
//!
//! Status map, each pinned in the tests below and by `oxyc sync`'s exit codes:
//!
//! - 400 `database_required`: no `database` named (`oxyc` lists the databases
//!   and asks for each);
//! - 404 `database_not_found`: no such database in the compiled config;
//! - 422 `not_syncable`: a type with no schema loader (`airhouse_managed`,
//!   `airhouse`, `mysql`), refused by design and reported as skipped;
//! - 400 `configuration`: the database is configured in a way that cannot
//!   connect (a missing secret, a file this node does not have);
//! - 502 `introspection_failed`: the warehouse failed; a rerun is safe.

use axum::{
    extract::Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use oxy::semantic::render::{RenderedFile, is_syncable, render_databases};
use oxy::service::sync::SyncFilter;
use oxy_app::surface::WorkspaceManagerReadOnly;
use oxy_app::surface::role_guards::WorkspaceAdmin;
use oxy_shared::errors::OxyError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The body: which database, narrowed to datasets or `schema.table`s.
/// `tables` wins over `datasets` when both are given.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct SyncDatabaseRequest {
    pub database: Option<String>,
    #[serde(default)]
    pub datasets: Vec<String>,
    #[serde(default)]
    pub tables: Vec<String>,
}

/// A refusal's body: a stable `error` code a caller branches on and a
/// `message` a person reads.
#[derive(Debug, Serialize, ToSchema)]
pub struct SyncError {
    pub error: &'static str,
    pub message: String,
}

impl SyncError {
    fn new(status: StatusCode, error: &'static str, message: impl Into<String>) -> Response {
        let body = SyncError {
            error,
            message: message.into(),
        };
        (status, Json(body)).into_response()
    }
}

/// Render one database's `.databases/` files from the warehouse.
#[utoipa::path(
    post,
    path = "/{workspace_id}/databases/sync",
    request_body = SyncDatabaseRequest,
    responses(
        (status = 200, description = "The files a sync writes, paths under `.databases/`", body = Vec<RenderedFile>),
        (status = 400, description = "No database named, or it cannot connect as configured", body = SyncError),
        (status = 404, description = "No such database in the compiled config", body = SyncError),
        (status = 422, description = "The database's type cannot be synced", body = SyncError),
        (status = 502, description = "The warehouse failed the introspection", body = SyncError)
    ),
    tag = "Databases"
)]
pub async fn sync_database_handler(
    _: WorkspaceAdmin,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Json(request): Json<SyncDatabaseRequest>,
) -> Response {
    let config = &workspace_manager.config_manager;
    let Some(name) = request.database.clone() else {
        return SyncError::new(
            StatusCode::BAD_REQUEST,
            "database_required",
            "name the database to sync; list them with GET /{workspace_id}/databases",
        );
    };
    let database = match config.resolve_database(&name) {
        Ok(database) => database,
        Err(e) => {
            return SyncError::new(StatusCode::NOT_FOUND, "database_not_found", e.to_string());
        }
    };
    if !is_syncable(&database) {
        return SyncError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_syncable",
            format!(
                "database '{name}' is {}, which sync cannot introspect",
                database.database_type
            ),
        );
    }
    let filter = SyncFilter {
        database: Some(name),
        datasets: request.datasets,
        tables: request.tables,
    };
    match render_databases(config, &workspace_manager.secrets_manager, filter).await {
        Ok(files) => Json(files).into_response(),
        Err(e) => failure(e),
    }
}

/// A render error's status: the database's configuration, or the warehouse.
fn failure(e: OxyError) -> Response {
    match e {
        OxyError::ConfigurationError(_) | OxyError::ArgumentError(_) => {
            SyncError::new(StatusCode::BAD_REQUEST, "configuration", e.to_string())
        }
        other => {
            tracing::warn!(error = %other, "databases/sync: introspection failed");
            SyncError::new(
                StatusCode::BAD_GATEWAY,
                "introspection_failed",
                other.to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// `oxyc sync` maps these to exit codes (6 and 7), and reads `error`.
    #[tokio::test]
    async fn a_configuration_error_is_400_and_anything_else_502() {
        let r = failure(OxyError::ConfigurationError("no key".into()));
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body(r).await["error"], "configuration");

        let r = failure(OxyError::DBError("connection reset".into()));
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
        let b = body(r).await;
        assert_eq!(b["error"], "introspection_failed");
        assert!(b["message"].as_str().unwrap().contains("connection reset"));
    }

    #[test]
    fn the_body_takes_a_database_and_optional_filters() {
        let r: SyncDatabaseRequest = serde_json::from_str(r#"{"database":"bq"}"#).unwrap();
        assert_eq!(r.database.as_deref(), Some("bq"));
        assert!(r.datasets.is_empty() && r.tables.is_empty());
        let r: SyncDatabaseRequest =
            serde_json::from_str(r#"{"database":"bq","datasets":["a"],"tables":["a.t"]}"#).unwrap();
        assert_eq!(
            (r.datasets, r.tables),
            (vec!["a".into()], vec!["a.t".into()])
        );
    }
}
