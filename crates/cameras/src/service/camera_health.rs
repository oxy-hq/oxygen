//! Per-camera health rollup for the operator dashboard.
//!
//! The worker posts to `/control/cameras/{cam_id}/health` every
//! ~30s with fps, decoder error count, reconnect count, and the
//! latest frame timestamp. We persist each post into
//! `oxy_cam_camera_health` (Airhouse). This module rolls those
//! rows up into a *single most-recent snapshot per camera* so the
//! UI can show "is this camera healthy right now?" at a glance.
//!
//! ## Status taxonomy
//!
//! Three buckets, derived from the most-recent row:
//!
//!   - `ok` — fps within tolerance and `last_frame_at` is recent.
//!   - `degraded` — fps below target OR decoder errors / reconnect
//!     storms in the window.
//!   - `stale` — no health post in the staleness window, OR the
//!     worker says it hasn't decoded a frame recently. Either way
//!     the operator should look.
//!
//! Thresholds are env-tunable so deployments with non-standard
//! cameras (e.g. 5 fps PTZ instead of 15 fps fixed) can adjust
//! without code changes.
//!
//! ## Why "newest row per camera" instead of windowed aggregates
//!
//! At V1 fleet sizes (tens of cameras per workspace, one health
//! post per camera per 30s) the newest row is good enough — the
//! worker already tracks running counters across its lifetime,
//! and the operator-facing question is binary ("is this camera
//! upset right now?"). Windowed deltas would be more accurate
//! about "errors per minute" but the UI value of "current state"
//! beats the precision of "rate over time."

use crate::airhouse::escape::sql_str;
use crate::entities::{cameras, edge_boxes, sites};
use crate::service::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use std::fmt::Write;
use uuid::Uuid;

/// Default fps target. Cameras under this are flagged `degraded`.
const DEFAULT_FPS_TARGET: f32 = 10.0;

/// How long since `last_frame_at` before a camera is `stale`,
/// in seconds. 120s = the worker's two consecutive missed posts —
/// transient glitches don't flip the UI to red.
const DEFAULT_STALE_FRAME_SECS: i64 = 120;

/// How long since the most recent health row before the camera
/// itself is considered `stale` (the worker hasn't even reported
/// in). Tied to the worker's 30s health cadence; we tolerate 3
/// missed cycles.
const DEFAULT_STALE_REPORT_SECS: i64 = 120;

/// Decoder error count above which a camera is `degraded`.
/// Decoder errors are cumulative since worker boot; a small
/// non-zero count is normal during initial RTSP handshake.
const DEFAULT_DECODER_ERROR_LIMIT: i64 = 20;

/// Reconnect count above which a camera is `degraded`. A handful
/// of reconnects on boot is normal; double digits means the link
/// is flapping.
const DEFAULT_RECONNECT_LIMIT: i64 = 10;

/// Read tunables from env once, cache for the process. Env names
/// match the rest of the cameras crate (`OXY_CAMERA_HEALTH_*`).
#[derive(Debug, Clone, Copy)]
struct Thresholds {
    fps_target: f32,
    stale_frame_secs: i64,
    stale_report_secs: i64,
    decoder_error_limit: i64,
    reconnect_limit: i64,
}

impl Thresholds {
    fn from_env() -> Self {
        Self {
            fps_target: env_f32("OXY_CAMERA_HEALTH_FPS_TARGET", DEFAULT_FPS_TARGET),
            stale_frame_secs: env_i64(
                "OXY_CAMERA_HEALTH_STALE_FRAME_SECS",
                DEFAULT_STALE_FRAME_SECS,
            ),
            stale_report_secs: env_i64(
                "OXY_CAMERA_HEALTH_STALE_REPORT_SECS",
                DEFAULT_STALE_REPORT_SECS,
            ),
            decoder_error_limit: env_i64(
                "OXY_CAMERA_HEALTH_DECODER_ERROR_LIMIT",
                DEFAULT_DECODER_ERROR_LIMIT,
            ),
            reconnect_limit: env_i64("OXY_CAMERA_HEALTH_RECONNECT_LIMIT", DEFAULT_RECONNECT_LIMIT),
        }
    }
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// One row per camera the operator should see — even cameras
/// that have never reported (those show `status='unknown'`).
#[derive(Debug, Clone, Serialize)]
pub struct CameraHealthSummary {
    pub camera_id: Uuid,
    /// `ok` | `degraded` | `stale` | `unknown`. The route layer
    /// returns this verbatim so the UI doesn't have to compute
    /// status itself.
    pub status: String,
    /// Most-recent fps, if reported.
    pub fps: Option<f32>,
    /// Most-recent decoder error counter, if reported. Cumulative
    /// since worker boot.
    pub decoder_errors: Option<i64>,
    pub reconnect_count: Option<i64>,
    /// When the worker last successfully decoded a frame. Older
    /// than `stale_frame_secs` triggers `stale`.
    pub last_frame_at: Option<DateTime<Utc>>,
    /// When this row was posted to us. Older than
    /// `stale_report_secs` triggers `stale` regardless of the
    /// reported metrics — the worker itself is silent.
    pub last_reported_at: Option<DateTime<Utc>>,
    /// Human-readable reason the status isn't `ok`. Empty when
    /// status is `ok` or `unknown`. UI uses this as a tooltip.
    pub reason: String,
}

/// Per-camera health, one row per camera in the workspace.
///
/// `unknown` cameras (no health row ever) still appear so the UI
/// can distinguish "we don't know" from "this camera is fine."
pub async fn summarize_for_workspace(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> ServiceResult<Vec<CameraHealthSummary>> {
    // Camera IDs the workspace owns. Walk
    // workspace → sites → edge_boxes → cameras in two steps so
    // the join chain stays inside sea-orm's `Related` types and
    // we don't have to spell out `JoinType::InnerJoin` (which
    // varies in shape between sea-orm versions). The two-step
    // shape also keeps the second query small even on large
    // workspaces — `is_in` against a UUID list is bounded by
    // the per-workspace edge_box count.
    let box_ids: Vec<Uuid> = edge_boxes::Entity::find()
        .inner_join(sites::Entity)
        .filter(sites::Column::WorkspaceId.eq(workspace_id))
        .select_only()
        .column(edge_boxes::Column::Id)
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    if box_ids.is_empty() {
        return Ok(Vec::new());
    }
    let camera_ids: Vec<Uuid> = cameras::Entity::find()
        .filter(cameras::Column::EdgeBoxId.is_in(box_ids))
        .select_only()
        .column(cameras::Column::Id)
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    if camera_ids.is_empty() {
        return Ok(Vec::new());
    }

    let thresholds = Thresholds::from_env();
    let recent = fetch_recent_rows(workspace_id, &camera_ids).await?;
    let now = Utc::now();
    let mut out = Vec::with_capacity(camera_ids.len());
    for cid in camera_ids {
        let row = recent.iter().find(|r| r.camera_id == cid);
        out.push(build_summary(cid, row, &thresholds, now));
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct HealthRow {
    camera_id: Uuid,
    ts: DateTime<Utc>,
    fps: Option<f32>,
    decoder_errors: i64,
    reconnect_count: i64,
    last_frame_at: Option<DateTime<Utc>>,
}

/// Single Airhouse SELECT pulling the most-recent health row
/// per camera. A correlated subquery would be cleaner, but
/// DuckLake doesn't support `MAX() OVER` window functions in
/// this client — we fall back to grouping by camera + selecting
/// the max ts, then re-joining for the metric columns. At
/// workspace cameras counts (tens, not hundreds), this is fine.
async fn fetch_recent_rows(
    workspace_id: Uuid,
    camera_ids: &[Uuid],
) -> ServiceResult<Vec<HealthRow>> {
    let client = match crate::airhouse::connect_for_reads(workspace_id).await {
        Ok(c) => c,
        Err(crate::airhouse::AirhouseError::Disabled) => return Ok(Vec::new()),
        Err(e) => return Err(ServiceError::Airhouse(e)),
    };

    let camera_ids_sql = camera_ids
        .iter()
        .map(|c| sql_str(&c.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    // `qualify row_number() over` is the idiomatic "latest per
    // group" in DuckLake/DuckDB. Falls through to empty when the
    // schema isn't yet provisioned.
    let mut sql = String::from(
        "SELECT camera_id, ts, fps, decoder_errors, reconnect_count, last_frame_at \
         FROM ( \
             SELECT camera_id, ts, fps, decoder_errors, reconnect_count, last_frame_at, \
                    ROW_NUMBER() OVER (PARTITION BY camera_id ORDER BY ts DESC) AS rn \
             FROM oxy_cam_camera_health \
             WHERE camera_id IN (",
    );
    let _ = write!(sql, "{camera_ids_sql})");
    let _ = write!(sql, " ) sub WHERE rn = 1");

    let msgs = match client.simple_query(&sql).await {
        Ok(m) => m,
        Err(e) => {
            let s = crate::airhouse::pg_error_text(&e);
            if crate::airhouse::is_missing_table(&s, "oxy_cam_camera_health") {
                return Ok(Vec::new());
            }
            tracing::warn!(
                workspace_id = %workspace_id,
                error = %s,
                "camera_health.summarize: SELECT failed"
            );
            return Err(ServiceError::Airhouse(
                crate::airhouse::AirhouseError::Connect(format!("query failed: {s}")),
            ));
        }
    };

    let mut out = Vec::new();
    for msg in msgs {
        if let tokio_postgres::SimpleQueryMessage::Row(row) = msg {
            let camera_id = row
                .get("camera_id")
                .and_then(|s| Uuid::parse_str(s).ok())
                .unwrap_or(Uuid::nil());
            if camera_id.is_nil() {
                continue;
            }
            let ts = row.get("ts").and_then(parse_ts).unwrap_or_else(Utc::now);
            let fps = row.get("fps").and_then(|s| s.parse::<f32>().ok());
            let decoder_errors = row
                .get("decoder_errors")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let reconnect_count = row
                .get("reconnect_count")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let last_frame_at = row.get("last_frame_at").and_then(parse_ts);
            out.push(HealthRow {
                camera_id,
                ts,
                fps,
                decoder_errors,
                reconnect_count,
                last_frame_at,
            });
        }
    }
    Ok(out)
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .or_else(|| {
            // DuckLake's `YYYY-MM-DD HH:MM:SS[.fff]+TZ` (space).
            let with_t = s.replacen(' ', "T", 1);
            DateTime::parse_from_rfc3339(&with_t).ok()
        })
        .map(|t| t.with_timezone(&Utc))
}

fn build_summary(
    camera_id: Uuid,
    row: Option<&HealthRow>,
    thresholds: &Thresholds,
    now: DateTime<Utc>,
) -> CameraHealthSummary {
    let Some(row) = row else {
        return CameraHealthSummary {
            camera_id,
            status: "unknown".into(),
            fps: None,
            decoder_errors: None,
            reconnect_count: None,
            last_frame_at: None,
            last_reported_at: None,
            reason: String::new(),
        };
    };

    let mut reasons: Vec<String> = Vec::new();

    let report_age = (now - row.ts).num_seconds();
    let report_stale = report_age > thresholds.stale_report_secs;
    if report_stale {
        reasons.push(format!("no health report for {report_age}s"));
    }

    let frame_stale = match row.last_frame_at {
        Some(lf) => (now - lf).num_seconds() > thresholds.stale_frame_secs,
        // No `last_frame_at` reported AND the row itself isn't
        // stale → treat as not-yet-decoding (transient on
        // initial RTSP connect). We only flag frame staleness
        // when we have a positive signal that frames have
        // stopped.
        None => false,
    };
    if frame_stale && let Some(lf) = row.last_frame_at {
        let age = (now - lf).num_seconds();
        reasons.push(format!("last frame {age}s ago"));
    }

    let fps_low = row.fps.map(|f| f < thresholds.fps_target).unwrap_or(false);
    if fps_low {
        let f = row.fps.unwrap_or(0.0);
        reasons.push(format!(
            "fps {f:.1} below target {:.1}",
            thresholds.fps_target
        ));
    }

    let decoder_busy = row.decoder_errors > thresholds.decoder_error_limit;
    if decoder_busy {
        reasons.push(format!(
            "{} decoder errors (limit {})",
            row.decoder_errors, thresholds.decoder_error_limit
        ));
    }

    let reconnect_flap = row.reconnect_count > thresholds.reconnect_limit;
    if reconnect_flap {
        reasons.push(format!(
            "{} reconnects (limit {})",
            row.reconnect_count, thresholds.reconnect_limit
        ));
    }

    let status = if report_stale || frame_stale {
        "stale"
    } else if fps_low || decoder_busy || reconnect_flap {
        "degraded"
    } else {
        "ok"
    };

    CameraHealthSummary {
        camera_id,
        status: status.into(),
        fps: row.fps,
        decoder_errors: Some(row.decoder_errors),
        reconnect_count: Some(row.reconnect_count),
        last_frame_at: row.last_frame_at,
        last_reported_at: Some(row.ts),
        reason: reasons.join("; "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline_thresholds() -> Thresholds {
        Thresholds {
            fps_target: 10.0,
            stale_frame_secs: 120,
            stale_report_secs: 120,
            decoder_error_limit: 20,
            reconnect_limit: 10,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap()
    }

    use chrono::TimeZone;

    #[test]
    fn unknown_when_no_row() {
        let s = build_summary(Uuid::nil(), None, &baseline_thresholds(), now());
        assert_eq!(s.status, "unknown");
        assert!(s.reason.is_empty());
    }

    #[test]
    fn ok_when_recent_and_healthy() {
        let row = HealthRow {
            camera_id: Uuid::nil(),
            ts: now() - chrono::Duration::seconds(10),
            fps: Some(15.0),
            decoder_errors: 1,
            reconnect_count: 0,
            last_frame_at: Some(now() - chrono::Duration::seconds(1)),
        };
        let s = build_summary(Uuid::nil(), Some(&row), &baseline_thresholds(), now());
        assert_eq!(s.status, "ok");
        assert!(s.reason.is_empty());
    }

    #[test]
    fn stale_when_report_is_old() {
        let row = HealthRow {
            camera_id: Uuid::nil(),
            ts: now() - chrono::Duration::seconds(300),
            fps: Some(15.0),
            decoder_errors: 0,
            reconnect_count: 0,
            last_frame_at: Some(now() - chrono::Duration::seconds(1)),
        };
        let s = build_summary(Uuid::nil(), Some(&row), &baseline_thresholds(), now());
        assert_eq!(s.status, "stale");
        assert!(s.reason.contains("no health report"));
    }

    #[test]
    fn stale_when_last_frame_is_old() {
        let row = HealthRow {
            camera_id: Uuid::nil(),
            ts: now() - chrono::Duration::seconds(10),
            fps: Some(15.0),
            decoder_errors: 0,
            reconnect_count: 0,
            last_frame_at: Some(now() - chrono::Duration::seconds(300)),
        };
        let s = build_summary(Uuid::nil(), Some(&row), &baseline_thresholds(), now());
        assert_eq!(s.status, "stale");
        assert!(s.reason.contains("last frame"));
    }

    #[test]
    fn degraded_on_low_fps() {
        let row = HealthRow {
            camera_id: Uuid::nil(),
            ts: now() - chrono::Duration::seconds(10),
            fps: Some(3.0),
            decoder_errors: 0,
            reconnect_count: 0,
            last_frame_at: Some(now() - chrono::Duration::seconds(1)),
        };
        let s = build_summary(Uuid::nil(), Some(&row), &baseline_thresholds(), now());
        assert_eq!(s.status, "degraded");
        assert!(s.reason.contains("fps"));
    }

    #[test]
    fn degraded_on_decoder_errors() {
        let row = HealthRow {
            camera_id: Uuid::nil(),
            ts: now() - chrono::Duration::seconds(10),
            fps: Some(15.0),
            decoder_errors: 50,
            reconnect_count: 0,
            last_frame_at: Some(now() - chrono::Duration::seconds(1)),
        };
        let s = build_summary(Uuid::nil(), Some(&row), &baseline_thresholds(), now());
        assert_eq!(s.status, "degraded");
        assert!(s.reason.contains("decoder"));
    }

    #[test]
    fn degraded_on_reconnect_storm() {
        let row = HealthRow {
            camera_id: Uuid::nil(),
            ts: now() - chrono::Duration::seconds(10),
            fps: Some(15.0),
            decoder_errors: 0,
            reconnect_count: 50,
            last_frame_at: Some(now() - chrono::Duration::seconds(1)),
        };
        let s = build_summary(Uuid::nil(), Some(&row), &baseline_thresholds(), now());
        assert_eq!(s.status, "degraded");
        assert!(s.reason.contains("reconnect"));
    }

    #[test]
    fn multiple_reasons_concatenate() {
        let row = HealthRow {
            camera_id: Uuid::nil(),
            ts: now() - chrono::Duration::seconds(10),
            fps: Some(3.0),
            decoder_errors: 50,
            reconnect_count: 50,
            last_frame_at: Some(now() - chrono::Duration::seconds(1)),
        };
        let s = build_summary(Uuid::nil(), Some(&row), &baseline_thresholds(), now());
        assert_eq!(s.status, "degraded");
        assert!(s.reason.contains("fps"));
        assert!(s.reason.contains("decoder"));
        assert!(s.reason.contains("reconnect"));
    }

    #[test]
    fn stale_wins_over_degraded() {
        // Both a low fps AND a stale report — the staleness is
        // the more actionable signal, so it should be the
        // declared status. Reasons accumulate either way.
        let row = HealthRow {
            camera_id: Uuid::nil(),
            ts: now() - chrono::Duration::seconds(600),
            fps: Some(3.0),
            decoder_errors: 0,
            reconnect_count: 0,
            last_frame_at: Some(now() - chrono::Duration::seconds(600)),
        };
        let s = build_summary(Uuid::nil(), Some(&row), &baseline_thresholds(), now());
        assert_eq!(s.status, "stale");
    }
}
