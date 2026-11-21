//! The intent-classification insert, kept as a test fixture.
//!
//! Its only production writer was the `oxy intent` CLI, deleted 2026-11-21
//! with the classifier it drove. The tables stay (no migration drops them),
//! and the cluster map (`get_cluster_map_data`, `get_cluster_infos`) still
//! reads them; the scope tests seed `observability_intent_classifications`
//! through this function to prove that read is confined to one workspace.

use clickhouse::Row;
use oxy_shared::errors::OxyError;
use serde::Serialize;

use super::ClickHouseObservabilityStorage;

#[derive(Debug, Serialize, Row)]
struct ClassificationInsertRow {
    trace_id: String,
    question: String,
    cluster_id: i32,
    intent_name: String,
    confidence: f32,
    embedding: Vec<f32>,
    source_type: String,
    source: String,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn store_classification(
    storage: &ClickHouseObservabilityStorage,
    trace_id: &str,
    question: &str,
    cluster_id: u32,
    intent_name: &str,
    confidence: f32,
    embedding: &[f32],
    source_type: &str,
    source: &str,
) -> Result<(), OxyError> {
    if embedding.iter().any(|v| !v.is_finite()) {
        return Err(OxyError::RuntimeError("Non-finite embedding value".into()));
    }

    let mut insert = storage
        .client()
        .insert::<ClassificationInsertRow>("observability_intent_classifications")
        .await
        .map_err(|e| OxyError::RuntimeError(format!("ClickHouse insert init failed: {e}")))?;

    let row = ClassificationInsertRow {
        trace_id: trace_id.to_string(),
        question: question.to_string(),
        cluster_id: cluster_id as i32,
        intent_name: intent_name.to_string(),
        confidence,
        embedding: embedding.to_vec(),
        source_type: source_type.to_string(),
        source: source.to_string(),
    };

    insert
        .write(&row)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Classification write failed: {e}")))?;
    insert
        .end()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Classification insert end failed: {e}")))?;

    Ok(())
}
