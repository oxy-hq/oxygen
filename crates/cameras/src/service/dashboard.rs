//! Dashboard rollup — consolidates the volume + health KPIs the
//! Edge / Dashboard tab needs into one Airhouse round-trip, replacing
//! the per-site fan-out the frontend used in v0.
//!
//! Single endpoint covers totals, per-hour activity buckets (for the
//! time-series chart), and the top-N cameras by detections and by
//! violations. Health summary (per-camera status counts) is derived
//! Postgres-side from `service::camera_health` and folded in by the
//! route layer — keeping this module focused on Airhouse-backed
//! compliance aggregates.
//!
//! The site filter is optional. When `Some(site_id)`, the camera
//! universe narrows to that site; when `None`, all workspace cameras
//! participate. Ownership of the site is enforced before any Airhouse
//! query so a cross-workspace `site_id` errors cleanly.

use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use tokio_postgres::SimpleQueryMessage;
use uuid::Uuid;

use crate::airhouse::escape::{sql_str, sql_ts};
use crate::entities::{cameras, sites};
use crate::service::{ServiceError, ServiceResult};

/// `WHERE` fragment shared by every "is this a violation" filter in
/// this module. Kept as a string constant so the per-camera rollup
/// and the hourly rollup can't drift out of sync — operators would
/// be confused if "top violations" said 12 and the per-hour chart
/// summed to 8. See `service::compliance::summary_for_site` for the
/// rationale behind the LIKE-on-bool + `>= 0.5` confidence floor.
const VIOLATION_FILTER: &str = "(\
        structured_json LIKE '%\"attire_compliant\":false%' \
     OR structured_json LIKE '%\"hygiene_compliant\":false%' \
    ) \
    AND COALESCE(CAST(json_extract(structured_json, '$.confidence') AS DOUBLE), 0.0) >= 0.5";

/// What `GET /{wid}/fleet/dashboard` returns. Designed for one render
/// of the Dashboard page — totals up top, hourly buckets for the
/// chart, top-N tables. Health summary is computed Postgres-side and
/// merged in by the route handler.
#[derive(Debug, Clone, Serialize)]
pub struct DashboardRollup {
    pub totals: DashboardTotals,
    pub hourly: Vec<HourlyBucket>,
    pub top_by_detections: Vec<CameraStat>,
    pub top_by_violations: Vec<CameraStat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardTotals {
    pub detections: u64,
    pub violations: u64,
    pub cameras_total: u32,
    pub cameras_with_activity: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct HourlyBucket {
    /// Bucket start, truncated to the hour boundary.
    pub hour: DateTime<Utc>,
    pub detections: u64,
    pub violations: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CameraStat {
    pub camera_id: Uuid,
    pub camera_name: String,
    pub detections: u64,
    pub violations: u64,
}

const TOP_N: usize = 5;

pub async fn rollup(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    site_id: Option<Uuid>,
    since: DateTime<Utc>,
) -> ServiceResult<DashboardRollup> {
    // Resolve the camera universe. With a `site_id`, narrow to that
    // site's cameras after verifying workspace ownership; without,
    // walk every site the workspace owns and take all their cameras.
    let cams = if let Some(sid) = site_id {
        let site = sites::Entity::find_by_id(sid)
            .one(db)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if site.workspace_id != workspace_id {
            return Err(ServiceError::Forbidden("site belongs to another workspace"));
        }
        cameras::Entity::find()
            .filter(cameras::Column::SiteId.eq(sid))
            .all(db)
            .await?
    } else {
        let site_ids: Vec<Uuid> = sites::Entity::find()
            .filter(sites::Column::WorkspaceId.eq(workspace_id))
            .all(db)
            .await?
            .into_iter()
            .map(|s| s.id)
            .collect();
        if site_ids.is_empty() {
            return Ok(empty_rollup());
        }
        cameras::Entity::find()
            .filter(cameras::Column::SiteId.is_in(site_ids))
            .all(db)
            .await?
    };

    if cams.is_empty() {
        return Ok(empty_rollup());
    }
    let cameras_total = cams.len() as u32;
    let camera_name_by_id: std::collections::HashMap<String, String> = cams
        .iter()
        .map(|c| (c.id.to_string(), c.name.clone()))
        .collect();
    let camera_ids_sql = cams
        .iter()
        .map(|c| sql_str(&c.id.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let since_clause = format!("received_at >= {}", sql_ts(since));

    // Two SELECTs in two round-trips. They could be unioned but the
    // shapes differ enough that two simple queries read clearer than
    // one CTE'd union; pgwire-side latency is negligible compared to
    // the Airhouse engine work.
    let per_camera_sql = format!(
        "SELECT camera_id, \
                COUNT(*) AS total, \
                COUNT(*) FILTER (WHERE {VIOLATION_FILTER}) AS violations \
         FROM oxy_cam_compliance_reports \
         WHERE camera_id IN ({camera_ids_sql}) AND {since_clause} \
         GROUP BY camera_id"
    );
    let hourly_sql = format!(
        "SELECT date_trunc('hour', received_at) AS hour, \
                COUNT(*) AS total, \
                COUNT(*) FILTER (WHERE {VIOLATION_FILTER}) AS violations \
         FROM oxy_cam_compliance_reports \
         WHERE camera_id IN ({camera_ids_sql}) AND {since_clause} \
         GROUP BY date_trunc('hour', received_at) \
         ORDER BY hour"
    );

    let client = match crate::airhouse::connect_for_reads(workspace_id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                error = %e,
                "dashboard.rollup: connect_for_reads failed"
            );
            return Err(ServiceError::Airhouse(e));
        }
    };

    // Per-camera aggregates. Schema-missing → empty (same treatment
    // as the per-site rollup) so a fresh workspace gets a clean 0/0
    // dashboard instead of a red error banner.
    let per_camera = match client.simple_query(&per_camera_sql).await {
        Ok(rows) => parse_per_camera(&rows),
        Err(e) => {
            let s = crate::airhouse::pg_error_text(&e);
            if schema_missing(&s) {
                Vec::new()
            } else {
                return Err(ServiceError::Airhouse(
                    crate::airhouse::AirhouseError::Insert(s),
                ));
            }
        }
    };
    let hourly = match client.simple_query(&hourly_sql).await {
        Ok(rows) => parse_hourly(&rows),
        Err(e) => {
            let s = crate::airhouse::pg_error_text(&e);
            if schema_missing(&s) {
                Vec::new()
            } else {
                return Err(ServiceError::Airhouse(
                    crate::airhouse::AirhouseError::Insert(s),
                ));
            }
        }
    };

    // Derive totals from the per-camera aggregates — keeps a single
    // source of truth (vs separately summing the hourly buckets, which
    // could disagree if a report straddles the boundary).
    let detections: u64 = per_camera.iter().map(|(_, t, _)| *t).sum();
    let violations: u64 = per_camera.iter().map(|(_, _, v)| *v).sum();
    let cameras_with_activity = per_camera.iter().filter(|(_, t, _)| *t > 0).count() as u32;

    // Top-N rankings. Sort once descending; same iter for both
    // detections-rank and violations-rank with different sort keys.
    let mut by_detections: Vec<CameraStat> = per_camera
        .iter()
        .filter_map(|(cid, total, viol)| {
            let name = camera_name_by_id.get(cid).cloned()?;
            cid.parse::<Uuid>().ok().map(|uuid| CameraStat {
                camera_id: uuid,
                camera_name: name,
                detections: *total,
                violations: *viol,
            })
        })
        .collect();
    by_detections.sort_by(|a, b| b.detections.cmp(&a.detections));
    let top_by_detections = by_detections.iter().take(TOP_N).cloned().collect();
    let mut by_violations = by_detections.clone();
    by_violations.retain(|c| c.violations > 0);
    by_violations.sort_by(|a, b| b.violations.cmp(&a.violations));
    let top_by_violations = by_violations.into_iter().take(TOP_N).collect();

    Ok(DashboardRollup {
        totals: DashboardTotals {
            detections,
            violations,
            cameras_total,
            cameras_with_activity,
        },
        hourly,
        top_by_detections,
        top_by_violations,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn empty_rollup() -> DashboardRollup {
    DashboardRollup {
        totals: DashboardTotals {
            detections: 0,
            violations: 0,
            cameras_total: 0,
            cameras_with_activity: 0,
        },
        hourly: Vec::new(),
        top_by_detections: Vec::new(),
        top_by_violations: Vec::new(),
    }
}

/// Both statements in `rollup` read the same table, so one predicate covers
/// them. Narrowed to that table: see `airhouse::is_missing_table`.
fn schema_missing(err: &str) -> bool {
    crate::airhouse::is_missing_table(err, "oxy_cam_compliance_reports")
}

fn parse_per_camera(msgs: &[SimpleQueryMessage]) -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    for msg in msgs {
        if let SimpleQueryMessage::Row(r) = msg {
            let cid = match get_opt_str(r, "camera_id") {
                Some(s) => s,
                None => continue,
            };
            let total = get_opt_str(r, "total")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let violations = get_opt_str(r, "violations")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            out.push((cid, total, violations));
        }
    }
    out
}

fn parse_hourly(msgs: &[SimpleQueryMessage]) -> Vec<HourlyBucket> {
    let mut out = Vec::new();
    for msg in msgs {
        if let SimpleQueryMessage::Row(r) = msg {
            let hour = match get_opt_str(r, "hour").and_then(|s| parse_ducklake_ts(&s)) {
                Some(t) => t,
                None => continue,
            };
            let detections = get_opt_str(r, "total")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let violations = get_opt_str(r, "violations")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            out.push(HourlyBucket {
                hour,
                detections,
                violations,
            });
        }
    }
    out
}

fn get_opt_str(row: &tokio_postgres::SimpleQueryRow, name: &str) -> Option<String> {
    row.try_get(name)
        .ok()
        .flatten()
        .map(|s: &str| s.to_string())
}

/// DuckLake naive-UTC timestamp parser. Mirrors the one in
/// `service::compliance`. Local copy here keeps `dashboard.rs`
/// self-contained without a cross-module dependency on a private fn.
fn parse_ducklake_ts(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .ok()
}
