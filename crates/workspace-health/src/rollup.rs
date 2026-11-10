//! The cross-tenant health rollup as the console reads it: the persisted sweep
//! state, joined with display labels, worst-first. Pure Postgres read — no live
//! evaluation. The admin route that serves it is `oxy-api-workspace-health`'s;
//! the partner console reads the same rows through [`health_rollup`].

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;

use crate::evaluator::WorkspaceSignals;
use crate::queries;

/// Raw per-workspace signal counts, surfaced so the per-workspace Health tab
/// can show the underlying numbers behind each dimension's reason string.
#[derive(Serialize)]
pub struct SignalsRow {
    failed_runs: i64,
    timed_out_runs: i64,
    total_runs: i64,
    airway_last_run_failed: bool,
    airway_completed_with_errors: bool,
    open_high_anomalies: i64,
    open_medium_anomalies: i64,
    dead_letter_count: i64,
}

impl From<&WorkspaceSignals> for SignalsRow {
    fn from(s: &WorkspaceSignals) -> Self {
        Self {
            failed_runs: s.failed_runs,
            timed_out_runs: s.timed_out_runs,
            total_runs: s.total_runs,
            airway_last_run_failed: s.airway_last_run_failed,
            airway_completed_with_errors: s.airway_completed_with_errors,
            open_high_anomalies: s.open_high_anomalies,
            open_medium_anomalies: s.open_medium_anomalies,
            dead_letter_count: s.dead_letter_count,
        }
    }
}

#[derive(Serialize)]
pub struct WorkspaceHealthRow {
    workspace_id: uuid::Uuid,
    /// Workspace display name, resolved from the `workspaces` row. `None` only
    /// when the workspace was deleted between the signal scan and this read.
    workspace_name: Option<String>,
    /// Owning org name, `None` when the workspace has no `org_id` (or was
    /// deleted). Shown alongside the name so operators don't read bare UUIDs.
    org_name: Option<String>,
    status: String,
    reasons: Vec<String>,
    /// Per-dimension breakdown (job liveness / pipeline / queue /
    /// reconciliation), each with its own status. Served verbatim from the
    /// stored payload as opaque JSON — the read path does no live evaluation.
    dimensions: serde_json::Value,
    signals: serde_json::Value,
    /// Per-check reconciliation drift detail from the stored payload.
    reconciliation: serde_json::Value,
    /// Per-probe smoke-test detail from the stored payload. These verdicts can
    /// be older than `checked_at` — the smoke test runs on its own slower
    /// cadence, and `last_smoke_at` says when they were actually produced.
    smoke: serde_json::Value,
    /// Which smoke probe kinds are enabled (`[{ kind, enabled }]`), so the UI
    /// can render a disabled probe as "not enabled" instead of omitting it.
    /// Empty when the workspace's smoke test is disabled or the row predates it.
    smoke_probes: serde_json::Value,
    /// When the smoke probes last ran. `None` when no smoke test is configured.
    last_smoke_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// When the status last transitioned, from the persisted eval-pass state
    /// (`workspace_health_state.changed_at`). `None` when no eval pass has
    /// recorded this workspace yet — the rollup is computed live, but the
    /// transition time is only known from the periodic sweep.
    changed_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// When the periodic sweep last evaluated this workspace
    /// (`workspace_health_state.updated_at`), refreshed on every pass even when
    /// the status is unchanged. `None` until the first sweep records it. Drives
    /// the "last checked" display so operators can tell stale rollups from live
    /// ones (the sweep runs every 10 minutes).
    checked_at: Option<chrono::DateTime<chrono::FixedOffset>>,
}

#[derive(Serialize)]
pub struct WorkspaceHealthResponse {
    pub workspaces: Vec<WorkspaceHealthRow>,
}

/// The rollup rows, worst-first, joined with display labels. `workspace_ids =
/// None` returns every workspace (the staff cross-tenant view); `Some(ids)` scopes
/// to that set (the partner-scoped view over its managed clients' workspaces).
/// Reads the persisted sweep state only — no live evaluation.
pub async fn health_rollup(
    db: &sea_orm::DatabaseConnection,
    workspace_ids: Option<&[uuid::Uuid]>,
) -> Result<Vec<WorkspaceHealthRow>, sea_orm::DbErr> {
    if matches!(workspace_ids, Some(ids) if ids.is_empty()) {
        return Ok(Vec::new());
    }
    let labels = queries::gather_workspace_labels(db).await?;

    let mut find = entity::workspace_health_state::Entity::find();
    if let Some(ids) = workspace_ids {
        find = find.filter(entity::workspace_health_state::Column::WorkspaceId.is_in(ids.to_vec()));
    }
    let rows = find.all(db).await?;

    // A state row has no FK to `workspaces`, so it outlives a deleted workspace.
    // No label means the workspace is gone: its verdict is frozen and can never be
    // re-evaluated, so leave it out rather than list it as "Unknown workspace".
    let mut workspaces: Vec<WorkspaceHealthRow> = rows
        .into_iter()
        .filter_map(|r| {
            let label = labels.get(&r.workspace_id)?;
            Some(row_from_state(r, label))
        })
        .collect();
    // Worst-first: Unhealthy > Degraded > Healthy.
    workspaces.sort_by_key(|w| std::cmp::Reverse(status_rank(&w.status)));
    Ok(workspaces)
}

/// The status/reasons/dimensions/signals/reconciliation parts of a row, taken
/// from the stored payload when present, else from the state row's own
/// `status`/`reasons` columns (a transient state right after the payload
/// migration backfills NULL, before the next sweep writes it).
struct PayloadParts {
    status: String,
    reasons: Vec<String>,
    dimensions: serde_json::Value,
    signals: serde_json::Value,
    reconciliation: serde_json::Value,
    smoke: serde_json::Value,
    smoke_probes: serde_json::Value,
}

fn payload_parts(
    payload: Option<serde_json::Value>,
    status_col: String,
    reasons_col: serde_json::Value,
) -> PayloadParts {
    let empty_arr = || serde_json::Value::Array(vec![]);
    match payload {
        Some(p) => PayloadParts {
            status: p
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or(status_col),
            reasons: p
                .get("reasons")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            dimensions: p.get("dimensions").cloned().unwrap_or_else(empty_arr),
            signals: p.get("signals").cloned().unwrap_or(serde_json::Value::Null),
            reconciliation: p.get("reconciliation").cloned().unwrap_or_else(empty_arr),
            smoke: p.get("smoke").cloned().unwrap_or_else(empty_arr),
            smoke_probes: p.get("smoke_probes").cloned().unwrap_or_else(empty_arr),
        },
        None => PayloadParts {
            status: status_col,
            reasons: serde_json::from_value(reasons_col).unwrap_or_default(),
            dimensions: empty_arr(),
            signals: serde_json::Value::Null,
            reconciliation: empty_arr(),
            smoke: empty_arr(),
            smoke_probes: empty_arr(),
        },
    }
}

/// Build a response row from a persisted state row + its display label.
fn row_from_state(
    r: entity::workspace_health_state::Model,
    label: &queries::WorkspaceLabel,
) -> WorkspaceHealthRow {
    let changed_at = r.changed_at;
    let checked_at = r.updated_at;
    let last_smoke_at = r.last_smoke_at;
    let workspace_id = r.workspace_id;
    let parts = payload_parts(r.payload, r.status, r.reasons);
    WorkspaceHealthRow {
        workspace_id,
        workspace_name: Some(label.name.clone()),
        org_name: label.org_name.clone(),
        status: parts.status,
        reasons: parts.reasons,
        dimensions: parts.dimensions,
        signals: parts.signals,
        reconciliation: parts.reconciliation,
        smoke: parts.smoke,
        smoke_probes: parts.smoke_probes,
        last_smoke_at,
        changed_at: Some(changed_at),
        checked_at: Some(checked_at),
    }
}

fn status_rank(s: &str) -> u8 {
    match s {
        "unhealthy" => 2,
        "degraded" => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_payload_falls_back_to_columns() {
        // Right after the migration backfills NULL, the row's own status/reasons
        // columns still carry real data — use them, with empty detail.
        let parts = payload_parts(
            None,
            "degraded".to_string(),
            serde_json::json!(["x failed"]),
        );
        assert_eq!(parts.status, "degraded");
        assert_eq!(parts.reasons, vec!["x failed".to_string()]);
        assert!(parts.dimensions.as_array().unwrap().is_empty());
        assert!(parts.signals.is_null());
        assert!(parts.reconciliation.as_array().unwrap().is_empty());
        assert!(parts.smoke.as_array().unwrap().is_empty());
        assert!(parts.smoke_probes.as_array().unwrap().is_empty());
    }

    #[test]
    fn payload_without_smoke_keys_reads_as_no_checks() {
        // Rows written before the smoke dimension existed have neither `smoke`
        // nor `smoke_probes`; they must read as "no smoke data", not fail to
        // deserialize.
        let payload = serde_json::json!({
            "status": "healthy",
            "reasons": [],
            "dimensions": [],
            "signals": null,
            "reconciliation": []
        });
        let parts = payload_parts(Some(payload), "healthy".to_string(), serde_json::json!([]));
        assert!(parts.smoke.as_array().unwrap().is_empty());
        assert!(parts.smoke_probes.as_array().unwrap().is_empty());
    }

    #[test]
    fn stored_payload_is_returned_verbatim() {
        let payload = serde_json::json!({
            "workspace_id": uuid::Uuid::nil(),
            "status": "degraded",
            "reasons": ["x drifts 3.0% from source"],
            "dimensions": [],
            "signals": null,
            "reconciliation": [{ "check": "x", "status": "degraded" }],
            "smoke": [{ "check": "connection:bq", "status": "healthy" }],
            "smoke_probes": [{ "kind": "connection", "enabled": true }]
        });
        let parts = payload_parts(Some(payload), "healthy".to_string(), serde_json::json!([]));
        // Payload status wins over the column.
        assert_eq!(parts.status, "degraded");
        assert_eq!(parts.reasons[0], "x drifts 3.0% from source");
        assert_eq!(parts.reconciliation.as_array().unwrap().len(), 1);
        assert_eq!(parts.smoke.as_array().unwrap().len(), 1);
        assert_eq!(parts.smoke_probes.as_array().unwrap().len(), 1);
    }

    /// Anomalies no longer vote on the verdict (see `evaluator::WorkspaceSignals`),
    /// but the counts must still reach the Health tab — "visible, non-voting" is
    /// the whole point, and dropping them from the DTO would silently make the
    /// panel's anomaly rows read zero.
    #[test]
    fn anomaly_counts_still_reach_the_dto() {
        let mut s = WorkspaceSignals::empty(uuid::Uuid::nil());
        s.open_high_anomalies = 4;
        s.open_medium_anomalies = 2;
        let row = SignalsRow::from(&s);
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["open_high_anomalies"], 4);
        assert_eq!(json["open_medium_anomalies"], 2);
    }
}
