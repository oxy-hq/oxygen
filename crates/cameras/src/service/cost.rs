//! Per-box and workspace-wide VLM cost rollups.
//!
//! Builds on top of [`crate::service::pricing`]. Reads
//! `oxy_cam_compliance_reports.tokens_used` aggregated by
//! `vlm_model` and converts via the published price list. V1
//! rolls up by edge_box (joining cameras→edge_boxes in Postgres
//! first, then filtering the Airhouse query by camera_id IN
//! (...)). When a `llm_usage` table lands later, the workspace
//! rollup can switch to a flat scan of that.

use crate::airhouse::escape::{sql_str, sql_ts};
use crate::entities::{cameras, edge_boxes, sites};
use crate::service::pricing;
use crate::service::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use uuid::Uuid;

/// Operator-supplied time window for a cost rollup. Anything
/// outside this enum is rejected at the route layer — keeps the
/// SQL surface tiny and predictable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostRange {
    #[serde(rename = "24h")]
    H24,
    #[serde(rename = "7d")]
    D7,
    #[serde(rename = "30d")]
    D30,
}

impl CostRange {
    /// Convert to a `received_at >= ?` cutoff. Anchored on `now`
    /// rather than calendar boundaries — operators reading the
    /// dashboard expect "the last 24h" to mean "from now back
    /// 24h," not "since midnight."
    pub fn since(&self) -> DateTime<Utc> {
        use chrono::Duration;
        let d = match self {
            Self::H24 => Duration::hours(24),
            Self::D7 => Duration::days(7),
            Self::D30 => Duration::days(30),
        };
        Utc::now() - d
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::H24 => "24h",
            Self::D7 => "7d",
            Self::D30 => "30d",
        }
    }
}

/// One row per VLM model. Operators see "claude-haiku-4-5 — $X.YZ
/// from N reports" in the per-box drill-down.
#[derive(Debug, Clone, Serialize)]
pub struct CostByModel {
    pub model: String,
    pub reports: i64,
    pub tokens_used: i64,
    /// `None` for unknown models (we don't fudge — surface "n/a"
    /// in the UI). When the worker is sending a model we haven't
    /// priced yet, the operator sees the model name and can ask
    /// us to bump [`pricing::published_prices`].
    pub cost_usd_micros: Option<i64>,
}

/// Roll-up surfaced by `GET /{wid}/cameras/edge-boxes/{id}/cost`.
/// `cost_usd_micros` is the sum across all priced models; reports
/// for unpriced models still count toward `reports` but don't add
/// to the cost.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeBoxCost {
    pub edge_box_id: Uuid,
    pub range: String,
    pub reports: i64,
    pub tokens_used: i64,
    pub cost_usd_micros: i64,
    pub by_model: Vec<CostByModel>,
}

/// Workspace-wide aggregate plus the per-box breakdown the Fleet
/// dashboard renders as a sortable list.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceCost {
    pub range: String,
    pub reports: i64,
    pub tokens_used: i64,
    pub cost_usd_micros: i64,
    pub by_box: Vec<EdgeBoxCost>,
}

/// Per-edge-box cost roll-up.
///
/// Ownership chain: edge_box → site → workspace_id. We re-do the
/// check here even though the route layer has already extracted
/// `workspace_id` from the URL, because a bug in route assembly
/// would otherwise turn into a cross-workspace cost leak.
pub async fn box_cost_for(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    edge_box_id: Uuid,
    range: CostRange,
) -> ServiceResult<EdgeBoxCost> {
    let box_row = edge_boxes::Entity::find_by_id(edge_box_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    let site = sites::Entity::find_by_id(box_row.site_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    if site.workspace_id != workspace_id {
        return Err(ServiceError::Forbidden(
            "edge box belongs to another workspace",
        ));
    }

    // Cameras bound to this edge box (NULL `edge_box_id` cameras
    // haven't been auto-claimed yet — their cost shows up on the
    // first box that claims them, which is correct).
    let camera_ids: Vec<Uuid> = cameras::Entity::find()
        .filter(cameras::Column::EdgeBoxId.eq(edge_box_id))
        .select_only()
        .column(cameras::Column::Id)
        .into_tuple::<Uuid>()
        .all(db)
        .await?;

    let by_model = if camera_ids.is_empty() {
        Vec::new()
    } else {
        aggregate_by_model(workspace_id, &camera_ids, range.since()).await?
    };
    Ok(roll_up_box(edge_box_id, range, by_model))
}

/// Sum workspace VLM spend since an arbitrary cutoff. Used by the
/// budget service to compute month-to-date — calendar-month
/// boundaries don't fit the [`CostRange`] enum, so callers pass
/// a precomputed `DateTime` cutoff instead.
///
/// Mirrors [`workspace_cost_for`]'s ownership-chain semantics
/// (workspace → sites → edge_boxes → cameras) but skips the
/// per-box breakdown — the budget UI only needs the headline
/// total. Returns `0` for workspaces with no cameras or no
/// compliance reports yet.
pub async fn workspace_total_since(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    since: DateTime<Utc>,
) -> ServiceResult<i64> {
    // Walk workspace → sites → edge_boxes → cameras to assemble
    // the camera_id set. Two-step (boxes, then cameras) instead
    // of a single multi-join because cameras has no direct
    // workspace_id column and the join through sites would need
    // a sea-orm `linked` definition we don't have.
    let box_ids: Vec<Uuid> = edge_boxes::Entity::find()
        .inner_join(sites::Entity)
        .filter(sites::Column::WorkspaceId.eq(workspace_id))
        .select_only()
        .column(edge_boxes::Column::Id)
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    if box_ids.is_empty() {
        return Ok(0);
    }
    let camera_ids: Vec<Uuid> = cameras::Entity::find()
        .filter(cameras::Column::EdgeBoxId.is_in(box_ids))
        .select_only()
        .column(cameras::Column::Id)
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    if camera_ids.is_empty() {
        return Ok(0);
    }
    let by_model = aggregate_by_model(workspace_id, &camera_ids, since).await?;
    Ok(by_model.iter().filter_map(|m| m.cost_usd_micros).sum())
}

/// Workspace-wide cost, with a per-box breakdown sorted by spend
/// descending so the heaviest boxes show first.
pub async fn workspace_cost_for(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    range: CostRange,
) -> ServiceResult<WorkspaceCost> {
    // Every edge box in this workspace (via sites).
    let box_rows = edge_boxes::Entity::find()
        .inner_join(sites::Entity)
        .filter(sites::Column::WorkspaceId.eq(workspace_id))
        .all(db)
        .await?;

    let mut by_box = Vec::with_capacity(box_rows.len());
    for b in &box_rows {
        // Reuse the per-box path so the ownership + model
        // aggregation logic stays in one place.
        let cost = box_cost_for(db, workspace_id, b.id, range).await?;
        by_box.push(cost);
    }

    // Sort heaviest-first; operators care about outliers.
    by_box.sort_by(|a, b| b.cost_usd_micros.cmp(&a.cost_usd_micros));

    let reports: i64 = by_box.iter().map(|b| b.reports).sum();
    let tokens_used: i64 = by_box.iter().map(|b| b.tokens_used).sum();
    let cost_usd_micros: i64 = by_box.iter().map(|b| b.cost_usd_micros).sum();
    Ok(WorkspaceCost {
        range: range.as_str().to_string(),
        reports,
        tokens_used,
        cost_usd_micros,
        by_box,
    })
}

// ── Internals ───────────────────────────────────────────────────────────────

fn roll_up_box(edge_box_id: Uuid, range: CostRange, by_model: Vec<CostByModel>) -> EdgeBoxCost {
    let reports: i64 = by_model.iter().map(|m| m.reports).sum();
    let tokens_used: i64 = by_model.iter().map(|m| m.tokens_used).sum();
    let cost_usd_micros: i64 = by_model.iter().filter_map(|m| m.cost_usd_micros).sum();
    EdgeBoxCost {
        edge_box_id,
        range: range.as_str().to_string(),
        reports,
        tokens_used,
        cost_usd_micros,
        by_model,
    }
}

/// Run the Airhouse aggregation query for one workspace's set of
/// camera_ids and convert each model bucket to a [`CostByModel`].
/// `Table doesn't exist` (fresh workspace, no compliance schema
/// yet) collapses to an empty result rather than an error.
async fn aggregate_by_model(
    workspace_id: Uuid,
    camera_ids: &[Uuid],
    since: DateTime<Utc>,
) -> ServiceResult<Vec<CostByModel>> {
    let client = match crate::airhouse::connect_for_reads(workspace_id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                error = %e,
                "cost.aggregate_by_model: connect_for_reads failed"
            );
            return Err(ServiceError::Airhouse(e));
        }
    };

    let camera_ids_sql = camera_ids
        .iter()
        .map(|c| sql_str(&c.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(
        "SELECT vlm_model, \
                COUNT(*) AS reports, \
                COALESCE(SUM(tokens_used), 0) AS tokens_used \
         FROM oxy_cam_compliance_reports \
         WHERE camera_id IN ({camera_ids_sql}) AND received_at >= {}",
        sql_ts(since),
    );
    let _ = write!(sql, " GROUP BY vlm_model");

    let msgs = match client.simple_query(&sql).await {
        Ok(m) => m,
        Err(e) => {
            let s = crate::airhouse::pg_error_text(&e);
            if crate::airhouse::is_missing_table(&s, "oxy_cam_compliance_reports") {
                tracing::debug!(
                    workspace_id = %workspace_id,
                    "cost.aggregate_by_model: schema not provisioned, returning empty"
                );
                return Ok(Vec::new());
            }
            tracing::warn!(
                workspace_id = %workspace_id,
                error = %s,
                "cost.aggregate_by_model: SELECT failed"
            );
            return Err(ServiceError::Airhouse(
                crate::airhouse::AirhouseError::Connect(format!("query failed: {s}")),
            ));
        }
    };

    let now = Utc::now();
    let mut agg: HashMap<String, (i64, i64)> = HashMap::new();
    for msg in msgs {
        if let tokio_postgres::SimpleQueryMessage::Row(row) = msg {
            let model = row.get("vlm_model").unwrap_or("").to_string();
            let reports: i64 = row.get("reports").and_then(|s| s.parse().ok()).unwrap_or(0);
            let tokens_used: i64 = row
                .get("tokens_used")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            // Defensive: a query that returns multiple rows for the
            // same model (shouldn't happen with GROUP BY but cheap
            // to handle) just sums.
            let entry = agg.entry(model).or_insert((0, 0));
            entry.0 += reports;
            entry.1 += tokens_used;
        }
    }

    Ok(agg
        .into_iter()
        .map(|(model, (reports, tokens_used))| {
            let cost = pricing::cost_micros_for(&model, tokens_used, now);
            CostByModel {
                model,
                reports,
                tokens_used,
                cost_usd_micros: cost,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_range_serde() {
        assert_eq!(serde_json::to_string(&CostRange::H24).unwrap(), "\"24h\"");
        assert_eq!(serde_json::to_string(&CostRange::D7).unwrap(), "\"7d\"");
        assert_eq!(serde_json::to_string(&CostRange::D30).unwrap(), "\"30d\"");

        assert!(matches!(
            serde_json::from_str::<CostRange>("\"24h\"").unwrap(),
            CostRange::H24
        ));
    }

    #[test]
    fn cost_range_since_is_now_anchored() {
        // Compare to "expected" with a 1s slop window — we anchor
        // on `Utc::now()` which moves between calls.
        let now = Utc::now();
        let computed = CostRange::H24.since();
        let expected = now - chrono::Duration::hours(24);
        let diff = (computed - expected).num_seconds().abs();
        assert!(diff <= 1, "diff was {diff}s");
    }

    #[test]
    fn roll_up_sums_across_models() {
        let by_model = vec![
            CostByModel {
                model: "a".into(),
                reports: 10,
                tokens_used: 1_000,
                cost_usd_micros: Some(500),
            },
            CostByModel {
                model: "b".into(),
                reports: 5,
                tokens_used: 2_000,
                cost_usd_micros: Some(2_500),
            },
            CostByModel {
                model: "unpriced".into(),
                reports: 3,
                tokens_used: 4_000,
                cost_usd_micros: None,
            },
        ];
        let rolled = roll_up_box(Uuid::nil(), CostRange::H24, by_model);
        assert_eq!(rolled.reports, 18);
        assert_eq!(rolled.tokens_used, 7_000);
        // Unpriced model contributes to tokens but not cost.
        assert_eq!(rolled.cost_usd_micros, 3_000);
    }
}
