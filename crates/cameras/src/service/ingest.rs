//! Event + health ingest into the workspace's Airhouse tenant.
//!
//! Each `write_*` builds a multi-row INSERT and runs it via
//! tokio-postgres simple-query (DuckLake doesn't support prepared
//! statements / `$N` placeholders). Schema is ensured lazily by
//! [`crate::airhouse::connect_and_ensure`] — first call per process per
//! workspace runs `CREATE TABLE IF NOT EXISTS` once, subsequent calls
//! skip straight to the INSERT.
//!
//! The connection returned by [`crate::airhouse::connect_and_ensure`] is a
//! **persistent, per-tenant** handle reused across every ingest call, so a
//! fleet of edge boxes no longer churns one Airhouse DuckDB session per POST.
//! Large batches are split into [`crate::airhouse::insert_chunk_rows`]-sized
//! INSERTs to bound the SQL string and the server-side row group.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::airhouse::escape::{
    sql_opt_f32, sql_opt_i32, sql_opt_i64, sql_opt_str, sql_opt_ts, sql_str, sql_ts,
};

use super::{ServiceError, ServiceResult};

// ── Payload shapes (unchanged from the stub version) ────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPayload {
    pub event_id: Uuid,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub camera_id: Uuid,
    pub event_type: String,
    pub zone_id: Option<String>,
    pub line_id: Option<String>,
    pub track_id: String,
    pub dwell_seconds: Option<f32>,
    pub confidence: Option<f32>,
    pub frame_uri: Option<String>,
    /// S3 key for an archived clip of this event's window. Only the edge's
    /// `congestion` events carry it; `#[serde(default)]` so every other event
    /// kind (which omits the field) still deserializes to `None`.
    #[serde(default)]
    pub evidence_s3_key: Option<String>,
    /// Free-text label. `upsell_attempt` events carry the offered item
    /// ('avocado', 'salmon'); `#[serde(default)]` so every other event kind
    /// (which omits the field) still deserializes to `None`.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraHealthPayload {
    pub camera_id: Uuid,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub fps: Option<f32>,
    pub bitrate_kbps: Option<f32>,
    pub last_frame_at: Option<chrono::DateTime<chrono::Utc>>,
    pub decoder_errors: i32,
    pub reconnect_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxHealthPayload {
    pub box_id: Uuid,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub cpu_pct: Option<f32>,
    pub gpu_pct: Option<f32>,
    pub mem_pct: Option<f32>,
    pub temp_c: Option<f32>,
    pub uptime_s: Option<i64>,
    pub image_tag: Option<String>,
    pub containers_running: Option<i32>,
}

#[derive(Debug, Default)]
pub struct IngestResult {
    pub accepted: usize,
}

// ── Writers ─────────────────────────────────────────────────────────────────

pub async fn write_events(
    workspace_id: Uuid,
    events: &[EventPayload],
) -> ServiceResult<IngestResult> {
    if events.is_empty() {
        return Ok(IngestResult { accepted: 0 });
    }
    let client = crate::airhouse::connect_and_ensure(workspace_id).await?;

    let mut accepted = 0;
    for chunk in events.chunks(crate::airhouse::insert_chunk_rows()) {
        let sql = build_events_insert(chunk);
        client.simple_query(&sql).await.map_err(|e| {
            ServiceError::Airhouse(crate::airhouse::AirhouseError::Insert(
                crate::airhouse::pg_error_text(&e),
            ))
        })?;
        accepted += chunk.len();
    }
    Ok(IngestResult { accepted })
}

fn build_events_insert(events: &[EventPayload]) -> String {
    let mut sql = String::from(
        "INSERT INTO oxy_cam_events \
         (event_id, ts, camera_id, event_type, zone_id, line_id, track_id, \
          dwell_seconds, confidence, frame_uri, evidence_s3_key, label) VALUES ",
    );
    for (i, e) in events.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let _ = write!(
            sql,
            "({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
            sql_str(&e.event_id.to_string()),
            sql_ts(e.ts),
            sql_str(&e.camera_id.to_string()),
            sql_str(&e.event_type),
            sql_opt_str(e.zone_id.as_deref()),
            sql_opt_str(e.line_id.as_deref()),
            sql_str(&e.track_id),
            sql_opt_f32(e.dwell_seconds),
            sql_opt_f32(e.confidence),
            sql_opt_str(e.frame_uri.as_deref()),
            sql_opt_str(e.evidence_s3_key.as_deref()),
            sql_opt_str(e.label.as_deref()),
        );
    }
    sql
}

pub async fn write_camera_health(
    workspace_id: Uuid,
    rows: &[CameraHealthPayload],
) -> ServiceResult<IngestResult> {
    if rows.is_empty() {
        return Ok(IngestResult { accepted: 0 });
    }
    let client = crate::airhouse::connect_and_ensure(workspace_id).await?;

    let mut accepted = 0;
    for chunk in rows.chunks(crate::airhouse::insert_chunk_rows()) {
        let sql = build_camera_health_insert(chunk);
        client.simple_query(&sql).await.map_err(|e| {
            ServiceError::Airhouse(crate::airhouse::AirhouseError::Insert(
                crate::airhouse::pg_error_text(&e),
            ))
        })?;
        accepted += chunk.len();
    }
    Ok(IngestResult { accepted })
}

fn build_camera_health_insert(rows: &[CameraHealthPayload]) -> String {
    let mut sql = String::from(
        "INSERT INTO oxy_cam_camera_health \
         (camera_id, ts, fps, bitrate_kbps, last_frame_at, decoder_errors, \
          reconnect_count) VALUES ",
    );
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let _ = write!(
            sql,
            "({}, {}, {}, {}, {}, {}, {})",
            sql_str(&r.camera_id.to_string()),
            sql_ts(r.ts),
            sql_opt_f32(r.fps),
            sql_opt_f32(r.bitrate_kbps),
            sql_opt_ts(r.last_frame_at),
            sql_opt_i32(Some(r.decoder_errors)),
            sql_opt_i32(Some(r.reconnect_count)),
        );
    }
    sql
}

pub async fn write_box_health(
    workspace_id: Uuid,
    rows: &[BoxHealthPayload],
) -> ServiceResult<IngestResult> {
    if rows.is_empty() {
        return Ok(IngestResult { accepted: 0 });
    }
    let client = crate::airhouse::connect_and_ensure(workspace_id).await?;

    let mut accepted = 0;
    for chunk in rows.chunks(crate::airhouse::insert_chunk_rows()) {
        let sql = build_box_health_insert(chunk);
        client.simple_query(&sql).await.map_err(|e| {
            ServiceError::Airhouse(crate::airhouse::AirhouseError::Insert(
                crate::airhouse::pg_error_text(&e),
            ))
        })?;
        accepted += chunk.len();
    }
    Ok(IngestResult { accepted })
}

fn build_box_health_insert(rows: &[BoxHealthPayload]) -> String {
    let mut sql = String::from(
        "INSERT INTO oxy_cam_box_health \
         (box_id, ts, cpu_pct, gpu_pct, mem_pct, temp_c, uptime_s, image_tag, \
          containers_running) VALUES ",
    );
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let _ = write!(
            sql,
            "({}, {}, {}, {}, {}, {}, {}, {}, {})",
            sql_str(&r.box_id.to_string()),
            sql_ts(r.ts),
            sql_opt_f32(r.cpu_pct),
            sql_opt_f32(r.gpu_pct),
            sql_opt_f32(r.mem_pct),
            sql_opt_f32(r.temp_c),
            sql_opt_i64(r.uptime_s),
            sql_opt_str(r.image_tag.as_deref()),
            sql_opt_i32(r.containers_running),
        );
    }
    sql
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(track: &str) -> EventPayload {
        EventPayload {
            event_id: Uuid::nil(),
            ts: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            camera_id: Uuid::nil(),
            event_type: "person".into(),
            zone_id: None,
            line_id: None,
            track_id: track.into(),
            dwell_seconds: None,
            confidence: None,
            frame_uri: None,
            evidence_s3_key: None,
            label: None,
        }
    }

    #[test]
    fn events_insert_is_single_multi_row_statement() {
        let sql = build_events_insert(&[event("a"), event("b"), event("c")]);
        assert!(sql.starts_with("INSERT INTO oxy_cam_events"));
        // One INSERT, three value tuples → exactly two separators.
        assert_eq!(sql.matches("), (").count(), 2);
        assert_eq!(sql.matches("INSERT INTO").count(), 1);
    }

    #[test]
    fn events_insert_escapes_single_quotes() {
        // A track_id carrying a quote must not break out of the literal.
        let sql = build_events_insert(&[event("a'b")]);
        assert!(sql.contains("'a''b'"));
    }

    #[test]
    fn camera_and_box_health_insert_target_right_tables() {
        let cam = CameraHealthPayload {
            camera_id: Uuid::nil(),
            ts: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            fps: Some(30.0),
            bitrate_kbps: None,
            last_frame_at: None,
            decoder_errors: 0,
            reconnect_count: 0,
        };
        assert!(
            build_camera_health_insert(&[cam]).starts_with("INSERT INTO oxy_cam_camera_health")
        );

        let bx = BoxHealthPayload {
            box_id: Uuid::nil(),
            ts: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            cpu_pct: Some(1.0),
            gpu_pct: None,
            mem_pct: None,
            temp_c: None,
            uptime_s: Some(10),
            image_tag: None,
            containers_running: Some(2),
        };
        assert!(build_box_health_insert(&[bx]).starts_with("INSERT INTO oxy_cam_box_health"));
    }
}
