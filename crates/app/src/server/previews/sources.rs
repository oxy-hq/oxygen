//! Sandbox sources for Airway samples (P4 step 5): `GET/PUT /previews/sources`.
//!
//! A rotate-on-use source (QuickBooks) is sampled only against a sandbox
//! company staff register here, per pipeline — never production's grant.
//! Intuit voids a refresh token when it issues the next one, so exactly one
//! component may rotate a grant; the save therefore refuses:
//!
//! * a var any production QuickBooks pipeline names (promoted revision) —
//!   `409 production_var`;
//! * a `realm_id` equal to a production pipeline's — `409 production_realm`;
//! * a second pipeline rotating the same var (the
//!   `idx_workspace_preview_sources_one_rotator` unique index) — `409
//!   rotating_var_taken`;
//! * any var containing `/` — every app-scoped secret is `apps/<app_id>/<KEY>`,
//!   and Pokehouse's production rotator is a custom app's Function
//!   (`refresh-qb-token`) whose grant lives there — and any key a custom-app
//!   manifest in the workspace declares (`env`, `webhook.secretVar`) — `409
//!   reserved_var`.
//!
//! The same checks run again when a sample's platform is built
//! ([`check_sandbox`]), so a row that went stale (a manifest declared its var
//! later) never runs.
//!
//! So the sampler is the only rotator of the sandbox grant. Rows hold names and
//! identifiers only (control plane: `workspace_preview_sources`), never a
//! secret value; `access_token_var` (read-only custody, for a sandbox token
//! someone else refreshes) is accepted too.

mod guard;

use agentic_airway::preview::SandboxSource;
use axum::http::StatusCode;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum SourceRequestError {
    #[error("{0}")]
    BadRequest(String),
    #[error("`{0}` is a production QuickBooks pipeline's credential; register the sandbox's own")]
    ProductionVar(String),
    #[error("realm `{0}` is a production QuickBooks company; register the sandbox company")]
    ProductionRealm(String),
    #[error("another pipeline's sandbox already rotates `{0}`; one grant has one rotator")]
    RotatingVarTaken(String),
    #[error(
        "`{0}` is reserved: an app-scoped secret (`apps/…`, any name with `/`) or a key a \
         custom app declares — a custom app may already rotate it; register the sandbox's own"
    )]
    ReservedVar(String),
    #[error("{0}")]
    Internal(String),
}

impl SourceRequestError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::ProductionVar(_) => "production_var",
            Self::ProductionRealm(_) => "production_realm",
            Self::RotatingVarTaken(_) => "rotating_var_taken",
            Self::ReservedVar(_) => "reserved_var",
            Self::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::ProductionVar(_)
            | Self::ProductionRealm(_)
            | Self::RotatingVarTaken(_)
            | Self::ReservedVar(_) => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<DbErr> for SourceRequestError {
    fn from(e: DbErr) -> Self {
        Self::Internal(format!("database error: {e}"))
    }
}

/// One registered sandbox (`GET` item, `PUT` answer).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SourceItem {
    pub pipeline: String,
    pub environment: String,
    pub overrides: SandboxSource,
    pub updated_by: Option<String>,
    pub updated_at: String,
}

/// `PUT /previews/sources`.
#[derive(Debug, Deserialize)]
pub struct PutSource {
    pub pipeline: String,
    pub environment: String,
    pub overrides: SandboxSource,
}

const SELECT: &str = "SELECT pipeline_name, environment, overrides, updated_by, updated_at \
                      FROM workspace_preview_sources WHERE workspace_id = $1";

/// The workspace's registered sandboxes, by pipeline.
pub async fn list(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Vec<SourceItem>, SourceRequestError> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("{SELECT} ORDER BY pipeline_name"),
            [workspace_id.into()],
        ))
        .await?;
    rows.iter().map(item).collect()
}

/// Register (or replace) `req.pipeline`'s sandbox, refusing what the module
/// doc lists.
pub async fn save(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    updated_by: Uuid,
    req: PutSource,
) -> Result<SourceItem, SourceRequestError> {
    let pipeline = req.pipeline.trim();
    check_request(pipeline, &req)?;
    check_sandbox(db, workspace_id, &req.overrides).await?;
    let overrides = serde_json::to_value(&req.overrides)
        .map_err(|e| SourceRequestError::Internal(e.to_string()))?;
    let saved = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO workspace_preview_sources \
                 (workspace_id, pipeline_name, environment, overrides, updated_by) \
             VALUES ($1, $2, 'sandbox', $3, $4) \
             ON CONFLICT (workspace_id, pipeline_name) DO UPDATE SET \
                 environment = EXCLUDED.environment, overrides = EXCLUDED.overrides, \
                 updated_by = EXCLUDED.updated_by, updated_at = now() \
             RETURNING pipeline_name, environment, overrides, updated_by, updated_at",
            [
                workspace_id.into(),
                pipeline.into(),
                overrides.into(),
                updated_by.into(),
            ],
        ))
        .await;
    match saved {
        Ok(Some(row)) => item(&row),
        Ok(None) => Err(SourceRequestError::Internal(
            "the save returned no row".into(),
        )),
        Err(e) if is_unique_violation(&e) => Err(SourceRequestError::RotatingVarTaken(
            req.overrides.rotating_var().unwrap_or_default().to_string(),
        )),
        Err(e) => Err(e.into()),
    }
}

fn check_request(pipeline: &str, req: &PutSource) -> Result<(), SourceRequestError> {
    if pipeline.is_empty() || pipeline.starts_with(agentic_airway::config::RESERVED_NAME_PREFIX) {
        return Err(SourceRequestError::BadRequest(format!(
            "`{pipeline}` is not a pipeline name"
        )));
    }
    if req.environment != "sandbox" {
        return Err(SourceRequestError::BadRequest(
            "`environment` must be `sandbox`: a preview samples no production grant".into(),
        ));
    }
    Ok(())
}

fn item(row: &sea_orm::QueryResult) -> Result<SourceItem, SourceRequestError> {
    let overrides: Value = row.try_get("", "overrides")?;
    let updated_by: Option<Uuid> = row.try_get("", "updated_by")?;
    let updated_at: chrono::DateTime<chrono::FixedOffset> = row.try_get("", "updated_at")?;
    Ok(SourceItem {
        pipeline: row.try_get("", "pipeline_name")?,
        environment: row.try_get("", "environment")?,
        overrides: serde_json::from_value(overrides)
            .map_err(|e| SourceRequestError::Internal(format!("a stored sandbox source: {e}")))?,
        updated_by: updated_by.map(|u| u.to_string()),
        updated_at: updated_at
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    })
}

pub use guard::{ProductionQuickBooks, check_sandbox, production_quickbooks};

fn is_unique_violation(e: &DbErr) -> bool {
    matches!(
        e.sql_err(),
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
    )
}
