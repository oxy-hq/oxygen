//! Analytics domain extension table: per-run analytics-specific data.
//!
//! The `entity` and `crud` submodules are crate-private — external consumers
//! use the [`AnalyticsRunMeta`] DTO and the facade functions below.

pub(crate) mod crud;
pub(crate) mod entity;
pub mod execution;
pub mod migration;

pub use execution::{RunExecution, beat_execution, begin_execution};
pub use migration::AnalyticsMigrator;

use sea_orm::{DatabaseConnection, DbErr};
use serde_json::Value;

// ── Public DTO ─────────────────────────────────────────────────────────────

/// Domain-specific metadata for an analytics run.
///
/// This is the public projection of the `analytics_run_extensions` table.
/// Consumers should never need the SeaORM entity directly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnalyticsRunMeta {
    pub run_id: String,
    pub agent_id: String,
    pub spec_hint: Option<Value>,
    pub thinking_mode: Option<String>,
}

impl From<entity::Model> for AnalyticsRunMeta {
    fn from(m: entity::Model) -> Self {
        Self {
            run_id: m.run_id,
            agent_id: m.agent_id,
            spec_hint: m.spec_hint,
            thinking_mode: m.thinking_mode,
        }
    }
}

// ── Facade functions ───────────────────────────────────────────────────────

/// Load the extension metadata for a single run.
pub async fn get_run_meta(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<AnalyticsRunMeta>, DbErr> {
    crud::get_extension(db, run_id)
        .await
        .map(|opt| opt.map(AnalyticsRunMeta::from))
}

/// Load extension metadata for multiple run IDs (bulk fetch).
pub async fn get_run_metas(
    db: &DatabaseConnection,
    run_ids: &[String],
) -> Result<Vec<AnalyticsRunMeta>, DbErr> {
    crud::get_extensions_by_run_ids(db, run_ids)
        .await
        .map(|v| v.into_iter().map(AnalyticsRunMeta::from).collect())
}

/// What a run's extension row records about its execution: whether an attempt
/// has begun it, and when that attempt was last alive. `None` when the run has
/// no extension row.
pub async fn get_run_execution(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<RunExecution>, DbErr> {
    crud::get_extension(db, run_id)
        .await
        .map(|opt| opt.map(RunExecution::from))
}

/// Insert an analytics extension row for a run.
pub async fn insert_run_meta(
    db: &DatabaseConnection,
    run_id: &str,
    agent_id: &str,
    thinking_mode: Option<String>,
) -> Result<(), DbErr> {
    crud::insert_extension(db, run_id, agent_id, thinking_mode).await
}

/// Update the spec_hint on the extension row after pipeline completion.
pub async fn update_run_spec_hint(
    db: &DatabaseConnection,
    run_id: &str,
    hint: Value,
) -> Result<(), DbErr> {
    crud::update_spec_hint(db, run_id, hint).await
}

pub async fn update_run_thinking_mode(
    db: &DatabaseConnection,
    run_id: &str,
    mode: Option<String>,
) -> Result<(), DbErr> {
    crud::update_thinking_mode(db, run_id, mode).await
}
