//! What a sample leaves behind, recorded for the preview: its tables in the
//! shadow map (`workspace_preview_tables`, `state = 'sample'`) — which is what
//! the TTL drop may drop and what later preview runs read instead of live — and
//! its stored schema compared with production's (`schema_compat::
//! compare_schemas`, the empirical half of the change check).
//!
//! Tables are recorded twice: as the load is about to start (the
//! `destination_load_started` event names them, so a sample that dies mid-load
//! still leaves its tables to the drop), and from the stored schema once the
//! sample has one — which also names each `replacing` table's `_raw` buffer.
//! A table recorded but never made is simply absent when the drop lists the
//! schema; one made but never recorded keeps its schema from being dropped
//! (left, and warned about), never dropped by mistake.

use agentic_airway::WriteDisposition;
use agentic_airway::preview::scoped_pipeline_name;
use agentic_airway::schema_compat::{PipelineCheck, Schema, Verdict, compare_schemas};
use airhouse::preview_sql::ShadowState;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde::Serialize;
use uuid::Uuid;

use crate::server::previews::analyze::FindingView;
use crate::server::previews::registry;

/// A finished sample, as its run outcome reports it. Names and findings only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SampleReport {
    /// The pipeline's own name.
    pub pipeline: String,
    /// `preview:<key>:<name>`, which the sample's rows are keyed by.
    pub preview_pipeline: String,
    /// The tables the sample's stored schema holds.
    pub tables: Vec<String>,
    /// Production had a stored schema to compare with.
    pub compared_with_live: bool,
    /// The worst finding's verdict; `None` when nothing was compared.
    pub verdict: Option<Verdict>,
    pub findings: Vec<FindingView>,
}

/// Where the sample is, for recording.
pub struct Recorded<'a> {
    pub workspace_id: Uuid,
    pub preview_key: &'a str,
    pub run_id: &'a str,
    pub pipeline: &'a str,
    /// The live dataset its tables stand in for; `None` records nothing.
    pub dataset: Option<&'a str>,
}

/// Record `tables` under the sample's dataset as it is about to load them.
pub async fn record_tables(
    db: &DatabaseConnection,
    at: &Recorded<'_>,
    tables: &[String],
) -> Result<(), String> {
    let Some(dataset) = at.dataset else {
        return Ok(());
    };
    let entries: Vec<_> = tables
        .iter()
        .map(|t| ((dataset.to_string(), t.clone()), ShadowState::Sample))
        .collect();
    registry::upsert_shadow(db, at.workspace_id, at.preview_key, at.run_id, &entries)
        .await
        .map_err(|e| format!("recording the sample's tables: {e}"))
}

/// Compare the sample's stored schema with production's and record its
/// tables (module doc).
pub async fn record_sample(
    db: &DatabaseConnection,
    at: &Recorded<'_>,
) -> Result<SampleReport, String> {
    let preview_pipeline = scoped_pipeline_name(at.preview_key, at.pipeline);
    let sample = stored_schema(db, at.workspace_id, &preview_pipeline).await?;
    let live = stored_schema(db, at.workspace_id, at.pipeline).await?;
    let findings = match (&live, &sample) {
        (Some(live), Some(sample)) => compare_schemas(live, sample),
        _ => Vec::new(),
    };
    let compared = live.is_some() && sample.is_some();
    let mut tables: Vec<String> = sample
        .as_ref()
        .map(|s| s.tables.keys().cloned().collect())
        .unwrap_or_default();
    tables.sort();
    if let (Some(dataset), Some(sample)) = (at.dataset, &sample) {
        let raw = format!("{dataset}_raw");
        let mut entries = Vec::new();
        for (name, table) in &sample.tables {
            entries.push(((dataset.to_string(), name.clone()), ShadowState::Sample));
            if table.write_disposition == WriteDisposition::Replacing {
                entries.push(((raw.clone(), name.clone()), ShadowState::Sample));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        registry::upsert_shadow(db, at.workspace_id, at.preview_key, at.run_id, &entries)
            .await
            .map_err(|e| format!("recording the sample's tables: {e}"))?;
    }
    Ok(SampleReport {
        pipeline: at.pipeline.to_string(),
        preview_pipeline,
        tables,
        compared_with_live: compared,
        verdict: compared.then(|| PipelineCheck::from_findings(findings.clone()).verdict),
        findings: findings.iter().map(FindingView::from).collect(),
    })
}

/// `airway_workspace_pipeline_state.schema_json` for `(workspace, name)`.
pub(super) async fn stored_schema(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<Option<Schema>, String> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT schema_json FROM airway_workspace_pipeline_state \
             WHERE workspace_id = $1 AND pipeline_name = $2",
            [workspace_id.into(), name.into()],
        ))
        .await
        .map_err(|e| format!("reading the stored schema of {name}: {e}"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let json: Option<serde_json::Value> = row
        .try_get("", "schema_json")
        .map_err(|e| format!("reading the stored schema of {name}: {e}"))?;
    json.map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("the stored schema of {name} does not parse: {e}"))
}
