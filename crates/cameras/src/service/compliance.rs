//! Compliance report ingest + read — VLM verdicts for sustained-presence
//! events. Lower volume than [`super::ingest`] (~one row per camera
//! per minute at most) but same destination tenant.
//!
//! Writes come from the edge worker via [`write_reports`]; reads come
//! from the operator UI's Compliance tab via [`list_for_camera`].

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use tokio_postgres::SimpleQueryMessage;
use uuid::Uuid;

use crate::airhouse::escape::{sql_opt_i32, sql_opt_str, sql_str, sql_ts};
use crate::entities::{cameras, edge_boxes, sites};

use super::{ServiceError, ServiceResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompliancePayload {
    pub report_id: Uuid,
    pub camera_id: Uuid,
    pub segment_start: chrono::DateTime<chrono::Utc>,
    pub segment_end: chrono::DateTime<chrono::Utc>,
    /// `"sustained_presence" | "periodic" | "manual"`.
    pub trigger_type: String,
    pub trigger_track_id: Option<String>,
    pub vlm_model: String,
    /// Raw markdown / freeform output from the VLM.
    pub report_text: String,
    /// Parsed JSON: `{attire_compliant, hygiene_compliant,
    /// missing_items, confidence, notes}`. Stored as a VARCHAR
    /// because DuckLake's JSON support is in flux; downstream queries
    /// parse it on read.
    pub structured_json: serde_json::Value,
    pub frame_uri: Option<String>,
    pub tokens_used: Option<i32>,
    /// Tier B #6 — bucket-relative S3 key for the archived
    /// dwell-window clip. Set by the worker once the upload
    /// succeeds; empty otherwise. The bucket itself is
    /// deployment-wide config (`OXY_CAMERAS_CLIPS_S3_BUCKET`), not stored
    /// per-row, so the deployment can re-front the bucket
    /// without rewriting rows.
    #[serde(default)]
    pub evidence_s3_key: Option<String>,
    /// P3 (Option C) — PPE-YOLO detections from the same frame
    /// the worker sent to the VLM. The VLM verdict stays
    /// authoritative; these are visual hints the operator UI
    /// overlays on the clip player so a reviewer can see what
    /// YOLO saw.
    ///
    /// Schema (this is the contract with the edge worker):
    ///
    /// ```json
    /// {
    ///   "model": "yolov8n-ppe-v0",
    ///   "frame_offset_ms": 4200,
    ///   "frame_width": 1920,
    ///   "frame_height": 1080,
    ///   "detections": [
    ///     {
    ///       "class": "hat",
    ///       "confidence": 0.91,
    ///       "bbox": { "x": 0.42, "y": 0.18, "width": 0.13, "height": 0.18 }
    ///     }
    ///   ]
    /// }
    /// ```
    ///
    /// Conventions:
    ///   - `bbox` is normalized [0,1] (top-left origin, width/height
    ///     relative to frame dims). Frame-resolution-agnostic.
    ///   - `frame_offset_ms` = ms into `segment_start` when the
    ///     YOLO snapshot was taken (typically the same frame the
    ///     VLM scored). UI uses this to position the overlay
    ///     timestamp ("YOLO @ 4.2s").
    ///   - Empty `detections` array means "YOLO ran, saw nothing";
    ///     missing/None means "edge worker doesn't run YOLO yet"
    ///     (pre-Option-C build). The UI distinguishes these.
    #[serde(default)]
    pub detections_json: Option<serde_json::Value>,
}

#[derive(Debug, Default)]
pub struct ComplianceIngestResult {
    pub accepted: usize,
}

pub async fn write_reports(
    workspace_id: Uuid,
    reports: Vec<CompliancePayload>,
) -> ServiceResult<ComplianceIngestResult> {
    if reports.is_empty() {
        return Ok(ComplianceIngestResult { accepted: 0 });
    }
    let client = crate::airhouse::connect_and_ensure(workspace_id).await?;

    let mut sql = String::from(
        "INSERT INTO oxy_cam_compliance_reports \
         (report_id, camera_id, segment_start, segment_end, trigger_type, \
          trigger_track_id, vlm_model, report_text, structured_json, \
          frame_uri, tokens_used, evidence_s3_key, detections_json, \
          agreement_status, agreement_detail_json) VALUES ",
    );
    for (i, r) in reports.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let structured = serde_json::to_string(&r.structured_json).unwrap_or_else(|_| "{}".into());
        // `detections_json` is stored as VARCHAR; serialize the JSON
        // value here. Default "[]" matches the column default for
        // pre-Option-C workers (so a query never sees NULL).
        let detections = r
            .detections_json
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok())
            .unwrap_or_else(|| "[]".into());
        // Agreement signal computed at INSERT time so the column is
        // immediately filterable + indexable. We don't backfill old
        // rows — the analytics arc cares about rolling-window data,
        // not historic re-computation.
        let (status, detail) =
            crate::service::agreement::compute(&r.structured_json, r.detections_json.as_ref());
        let detail_json = serde_json::to_string(&detail).unwrap_or_else(|_| "{}".into());
        let _ = write!(
            sql,
            "({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
            sql_str(&r.report_id.to_string()),
            sql_str(&r.camera_id.to_string()),
            sql_ts(r.segment_start),
            sql_ts(r.segment_end),
            sql_str(&r.trigger_type),
            sql_opt_str(r.trigger_track_id.as_deref()),
            sql_str(&r.vlm_model),
            sql_str(&r.report_text),
            sql_str(&structured),
            sql_opt_str(r.frame_uri.as_deref()),
            sql_opt_i32(r.tokens_used),
            sql_opt_str(r.evidence_s3_key.as_deref()),
            sql_str(&detections),
            sql_str(status.as_str()),
            sql_str(&detail_json),
        );
    }
    client.simple_query(&sql).await.map_err(|e| {
        ServiceError::Airhouse(crate::airhouse::AirhouseError::Insert(
            crate::airhouse::pg_error_text(&e),
        ))
    })?;

    // Live fan-out (world-model SSE) once the rows are durably inserted.
    for r in &reports {
        let s = &r.structured_json;
        let non_compliant = |key: &str| s.get(key).and_then(|v| v.as_bool()) == Some(false);
        let missing_items = s
            .get("missing_items")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        crate::service::events::emit(
            workspace_id,
            crate::service::events::CameraDomainEvent::ComplianceReport {
                camera_id: r.camera_id,
                report_id: r.report_id,
                violation: non_compliant("attire_compliant") || non_compliant("hygiene_compliant"),
                missing_items,
                confidence: s.get("confidence").and_then(|v| v.as_f64()),
                segment_start: r.segment_start,
                segment_end: r.segment_end,
            },
        );
    }

    Ok(ComplianceIngestResult {
        accepted: reports.len(),
    })
}

// ── Read side: operator UI Compliance tab ───────────────────────────────────

/// One row of the Compliance tab list. Shape matches the Airhouse
/// table 1:1 (no joins, all per-row), so the DTO is also flat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceReportRow {
    pub report_id: String,
    pub camera_id: String,
    pub segment_start: DateTime<Utc>,
    pub segment_end: DateTime<Utc>,
    pub trigger_type: String,
    pub trigger_track_id: Option<String>,
    pub vlm_model: String,
    pub report_text: String,
    /// Already-parsed JSON. We deserialize on the way out of pgwire
    /// so the UI doesn't have to re-parse a doubly-encoded string.
    pub structured_json: serde_json::Value,
    pub frame_uri: Option<String>,
    pub tokens_used: Option<i32>,
    /// Tier B #6 — S3 key for the archived dwell-window clip,
    /// when the worker uploaded one. UI surfaces a "view clip"
    /// affordance when set.
    pub evidence_s3_key: Option<String>,
    /// P3 (Option C) — parsed PPE-YOLO detections envelope as
    /// `{model, frame_offset_ms, frame_width, frame_height,
    /// detections: [...] }`. See `CompliancePayload::detections_json`
    /// for the full contract. `None` means "edge worker pre-dates
    /// Option C"; a present empty array means "YOLO ran, saw nothing."
    pub detections_json: Option<serde_json::Value>,
    /// VLM ↔ YOLO agreement signal computed at ingest. `Some("agree"
    /// | "disagree" | "inconclusive")` for rows ingested after Phase
    /// 1 of disagreement utilization; `None` for older rows.
    pub agreement_status: Option<String>,
    /// Per-class breakdown — only populated when both pipelines had
    /// comparable data. `{}` otherwise.
    pub agreement_detail_json: Option<serde_json::Value>,
    pub received_at: DateTime<Utc>,
}

/// Caps for [`list_for_camera`] — keep `limit` query strings honest.
pub const DEFAULT_LIST_LIMIT: u32 = 50;
pub const MAX_LIST_LIMIT: u32 = 500;

/// List compliance reports for one camera, newest first.
///
/// Workspace ownership is enforced before any Airhouse traffic — a
/// caller in workspace A asking for a camera in workspace B gets 403
/// without us even minting a credential.
///
/// `since`, when set, filters `received_at >= since`. Use it for
/// "today's reports" or to page backward by handing in the oldest
/// `received_at` from the previous page.
///
/// Returns an empty Vec (NOT an error) when the table doesn't exist
/// yet — e.g. a brand-new workspace whose edge boxes haven't sent any
/// compliance reports. DuckLake answers `undefined_table` as a
/// distinct error code; we treat it as "no data" because the UI
/// shouldn't blow up just because the schema is lazy.
pub async fn list_for_camera(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    camera_id: Uuid,
    since: Option<DateTime<Utc>>,
    limit: u32,
) -> ServiceResult<Vec<ComplianceReportRow>> {
    // Workspace ownership check first — mirror the preview proxy
    // chain so probing a foreign camera doesn't even reach Airhouse.
    let cam = cameras::Entity::find_by_id(camera_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    let site = sites::Entity::find_by_id(cam.site_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    if site.workspace_id != workspace_id {
        return Err(ServiceError::Forbidden(
            "camera belongs to another workspace",
        ));
    }

    let effective_limit = limit.clamp(1, MAX_LIST_LIMIT);

    let client = crate::airhouse::connect_for_reads(workspace_id)
        .await
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_id,
                camera_id = %camera_id,
                error = %e,
                "compliance.list_for_camera: connect_for_reads failed (broker mint or pgwire connect)"
            );
            ServiceError::Airhouse(e)
        })?;

    let mut sql = format!(
        "SELECT report_id, camera_id, segment_start, segment_end, \
                trigger_type, trigger_track_id, vlm_model, report_text, \
                structured_json, frame_uri, tokens_used, evidence_s3_key, \
                detections_json, agreement_status, agreement_detail_json, \
                received_at \
         FROM oxy_cam_compliance_reports \
         WHERE camera_id = {}",
        sql_str(&camera_id.to_string()),
    );
    if let Some(ts) = since {
        let _ = write!(sql, " AND received_at >= {}", sql_ts(ts));
    }
    let _ = write!(sql, " ORDER BY received_at DESC LIMIT {effective_limit}");

    let msgs = match client.simple_query(&sql).await {
        Ok(m) => m,
        Err(e) => {
            // "Table doesn't exist yet" is normal for a fresh workspace.
            // DuckLake doesn't ship a structured SQLSTATE we can match
            // on cleanly, so we fall back to substring sniffing.
            let s = crate::airhouse::pg_error_text(&e);
            if crate::airhouse::is_missing_table(&s, "oxy_cam_compliance_reports") {
                tracing::debug!(
                    workspace_id = %workspace_id,
                    camera_id = %camera_id,
                    "compliance.list_for_camera: schema not provisioned, returning empty"
                );
                return Ok(Vec::new());
            }
            tracing::warn!(
                workspace_id = %workspace_id,
                camera_id = %camera_id,
                error = %s,
                "compliance.list_for_camera: SELECT failed"
            );
            return Err(ServiceError::Airhouse(
                crate::airhouse::AirhouseError::Insert(s),
            ));
        }
    };

    let mut out = Vec::new();
    for msg in &msgs {
        if let SimpleQueryMessage::Row(r) = msg {
            out.push(row_to_report(r)?);
        }
    }
    Ok(out)
}

fn row_to_report(r: &tokio_postgres::SimpleQueryRow) -> ServiceResult<ComplianceReportRow> {
    let report_id = get_str(r, "report_id")?;
    let camera_id = get_str(r, "camera_id")?;
    let segment_start = get_ts(r, "segment_start")?;
    let segment_end = get_ts(r, "segment_end")?;
    let trigger_type = get_str(r, "trigger_type")?;
    let trigger_track_id = get_opt_str(r, "trigger_track_id");
    let vlm_model = get_str(r, "vlm_model")?;
    let report_text = get_str(r, "report_text")?;
    let structured_raw = get_opt_str(r, "structured_json").unwrap_or_else(|| "{}".into());
    let structured_json = serde_json::from_str(&structured_raw).unwrap_or(serde_json::Value::Null);
    let frame_uri = get_opt_str(r, "frame_uri").filter(|s| !s.is_empty());
    let tokens_used = r
        .try_get("tokens_used")
        .ok()
        .flatten()
        .and_then(|s: &str| s.parse::<i32>().ok());
    let evidence_s3_key = get_opt_str(r, "evidence_s3_key").filter(|s| !s.is_empty());
    // P3 (Option C). Empty array / missing column / unparseable → None
    // so the UI can distinguish "no YOLO" from "YOLO saw nothing"
    // ([] in the inner detections array). The outer envelope being
    // absent is the signal for "edge worker doesn't run YOLO yet."
    let detections_json = get_opt_str(r, "detections_json")
        .filter(|s| !s.is_empty() && s != "[]")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    // Phase 1 — agreement signal. NULL on rows ingested before this
    // column existed; treated as `None` by the UI ("not yet
    // available") to distinguish from `Some("inconclusive")` (which
    // means we tried to compute but couldn't).
    let agreement_status = get_opt_str(r, "agreement_status").filter(|s| !s.is_empty());
    let agreement_detail_json = get_opt_str(r, "agreement_detail_json")
        .filter(|s| !s.is_empty() && s != "{}")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let received_at = get_ts(r, "received_at")?;

    Ok(ComplianceReportRow {
        report_id,
        camera_id,
        segment_start,
        segment_end,
        trigger_type,
        trigger_track_id,
        vlm_model,
        report_text,
        structured_json,
        frame_uri,
        tokens_used,
        evidence_s3_key,
        detections_json,
        agreement_status,
        agreement_detail_json,
        received_at,
    })
}

fn get_str(r: &tokio_postgres::SimpleQueryRow, col: &str) -> ServiceResult<String> {
    r.try_get(col)
        .ok()
        .flatten()
        .map(str::to_string)
        .ok_or_else(|| {
            ServiceError::Airhouse(crate::airhouse::AirhouseError::Insert(format!(
                "missing required column `{col}` in compliance_reports row"
            )))
        })
}

fn get_opt_str(r: &tokio_postgres::SimpleQueryRow, col: &str) -> Option<String> {
    r.try_get(col).ok().flatten().map(str::to_string)
}

fn get_ts(r: &tokio_postgres::SimpleQueryRow, col: &str) -> ServiceResult<DateTime<Utc>> {
    let raw = get_str(r, col)?;
    parse_ducklake_ts(&raw).ok_or_else(|| {
        ServiceError::Airhouse(crate::airhouse::AirhouseError::Insert(format!(
            "could not parse column `{col}` as timestamp: {raw}"
        )))
    })
}

/// DuckLake's pgwire surface renders `TIMESTAMPTZ` inconsistently across
/// versions and connector paths:
///
///   - SQL-style with no separator and no zone: `2026-05-28 00:55:40`
///   - With fractional seconds:                 `2026-05-28 00:55:40.123`
///   - With offset:                             `2026-05-28 00:55:40+00`
///   - RFC 3339 / ISO-8601:                     `2026-05-28T00:55:40Z`
///
/// We try each shape in turn. Naive timestamps (no zone) are treated as
/// UTC, which matches what DuckLake stores internally — TIMESTAMPTZ
/// values are normalized to UTC at write time and the displayed
/// timezone is just presentation.
fn parse_ducklake_ts(raw: &str) -> Option<DateTime<Utc>> {
    use chrono::NaiveDateTime;

    // 1. Native RFC 3339 / ISO-8601 (e.g. our INSERT path emits this).
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }

    // 2. Naive SQL-style with optional fractional seconds.
    //    `%.f` is a no-op when fractional seconds are absent.
    if let Ok(naive) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(naive.and_utc());
    }

    // 3. SQL-style with a numeric offset (`+00`, `+0000`, `+00:00`).
    for fmt in [
        "%Y-%m-%d %H:%M:%S%.f%#z",
        "%Y-%m-%d %H:%M:%S%.f%z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
    ] {
        if let Ok(dt) = DateTime::parse_from_str(raw, fmt) {
            return Some(dt.with_timezone(&Utc));
        }
    }

    None
}

// ── Site rollup: incident counts per camera ─────────────────────────────────

/// One row of the site summary list. One row per camera in the site,
/// regardless of whether that camera has any compliance reports —
/// the UI surfaces "0 incidents" as useful information (cameras that
/// are configured but haven't produced any violations look healthy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraComplianceSummary {
    pub camera_id: String,
    pub camera_name: String,
    pub edge_box_name: Option<String>,
    pub total_reports: u64,
    /// Reports where the VLM marked attire or hygiene as non-compliant.
    /// Detected by `LIKE` against the encoded `structured_json` VARCHAR
    /// — DuckLake's JSON support is in flux and a substring match is
    /// stable across the field shapes the worker emits today.
    pub violations: u64,
    pub last_received_at: Option<DateTime<Utc>>,
}

/// Roll up compliance reports for one site, grouping by camera.
///
/// Returns one row per camera in the site (even ones with zero
/// reports). Workspace ownership is enforced on the site before any
/// Airhouse traffic.
///
/// One pgwire round-trip: a single GROUP BY over the camera_ids in
/// the site. The Postgres-side camera list is short (~10 per site
/// in restaurant deployments), so an `IN (…)` clause is fine — no
/// need for a temp table or values-list join.
pub async fn summary_for_site(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    site_id: Uuid,
    since: Option<DateTime<Utc>>,
) -> ServiceResult<Vec<CameraComplianceSummary>> {
    // Workspace ownership on the site.
    let site = sites::Entity::find_by_id(site_id)
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;
    if site.workspace_id != workspace_id {
        return Err(ServiceError::Forbidden("site belongs to another workspace"));
    }

    // All cameras in this site. The order is preserved into the
    // summary output so the UI gets a stable list — name-sorted via
    // the listing pattern elsewhere would also work, but the row
    // count is small enough that JS-side sort is the same speed.
    let cams = cameras::Entity::find()
        .filter(cameras::Column::SiteId.eq(site_id))
        .all(db)
        .await?;

    if cams.is_empty() {
        return Ok(Vec::new());
    }

    // Resolve human-readable edge_box names for the cameras that have
    // one. Same batch-lookup shape as `listing::list_cameras`.
    let edge_box_ids: Vec<Uuid> = cams.iter().filter_map(|c| c.edge_box_id).collect();
    let edge_box_names: std::collections::HashMap<Uuid, String> = if edge_box_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        edge_boxes::Entity::find()
            .filter(edge_boxes::Column::Id.is_in(edge_box_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|eb| (eb.id, format!("{} ({})", eb.hardware_model, eb.cohort)))
            .collect()
    };

    // Query Airhouse for per-camera aggregates. Use a single GROUP BY
    // so we get one row per camera in one round trip.
    let camera_ids_sql = cams
        .iter()
        .map(|c| sql_str(&c.id.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let since_clause = match since {
        Some(ts) => format!(" AND received_at >= {}", sql_ts(ts)),
        None => String::new(),
    };
    // Defense-in-depth violation filter:
    //   * `attire_compliant=false` OR `hygiene_compliant=false`
    //     (the actual "is this a violation" signal), AND
    //   * `confidence >= 0.5` to exclude the "I don't see a person"
    //     and "frame too blurry to tell" cases that the VLM is
    //     instructed to mark low-confidence (see
    //     `service::packs::starter::OUTPUT_SCHEMA_REMINDER`).
    //
    // We use substring LIKE for the boolean check (cheap, no JSON
    // parser dependency in DuckLake) and DuckDB's json_extract for
    // the numeric confidence — LIKE can't express `< 0.5` over a
    // float. CAST to DOUBLE handles missing/null confidence as 0,
    // which we treat as "not confident enough to count" (the
    // conservative direction for a violation surface).
    let sql = format!(
        "SELECT camera_id, \
                COUNT(*) AS total, \
                COUNT(*) FILTER (\
                    WHERE (\
                              structured_json LIKE '%\"attire_compliant\":false%' \
                           OR structured_json LIKE '%\"hygiene_compliant\":false%' \
                          ) \
                      AND COALESCE(CAST(json_extract(structured_json, '$.confidence') AS DOUBLE), 0.0) >= 0.5\
                ) AS violations, \
                MAX(received_at) AS last_at \
         FROM oxy_cam_compliance_reports \
         WHERE camera_id IN ({camera_ids_sql}){since_clause} \
         GROUP BY camera_id"
    );

    let client = crate::airhouse::connect_for_reads(workspace_id)
        .await
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_id,
                site_id = %site_id,
                error = %e,
                "compliance.summary_for_site: connect_for_reads failed (broker mint or pgwire connect)"
            );
            ServiceError::Airhouse(e)
        })?;
    let msgs = match client.simple_query(&sql).await {
        Ok(m) => m,
        Err(e) => {
            // Same "schema not provisioned yet → empty" treatment as
            // `list_for_camera`. The cameras still show up below with
            // zeroes.
            let s = crate::airhouse::pg_error_text(&e);
            if crate::airhouse::is_missing_table(&s, "oxy_cam_compliance_reports") {
                tracing::debug!(
                    workspace_id = %workspace_id,
                    site_id = %site_id,
                    "compliance.summary_for_site: schema not provisioned, all-zero rollup"
                );
                // Fall through with an empty aggregates map so cameras
                // still get a 0/0 row below.
                Vec::new()
            } else {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    site_id = %site_id,
                    error = %s,
                    "compliance.summary_for_site: SELECT failed"
                );
                return Err(ServiceError::Airhouse(
                    crate::airhouse::AirhouseError::Insert(s),
                ));
            }
        }
    };

    let mut agg_by_camera: std::collections::HashMap<String, (u64, u64, Option<DateTime<Utc>>)> =
        std::collections::HashMap::new();
    for msg in &msgs {
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
            let last_at = get_opt_str(r, "last_at").and_then(|raw| parse_ducklake_ts(&raw));
            agg_by_camera.insert(cid, (total, violations, last_at));
        }
    }

    Ok(cams
        .into_iter()
        .map(|c| {
            let (total, violations, last_at) = agg_by_camera
                .remove(&c.id.to_string())
                .unwrap_or((0, 0, None));
            let edge_box_name = c
                .edge_box_id
                .and_then(|id| edge_box_names.get(&id).cloned());
            CameraComplianceSummary {
                camera_id: c.id.to_string(),
                camera_name: c.name,
                edge_box_name,
                total_reports: total,
                violations,
                last_received_at: last_at,
            }
        })
        .collect())
}

// ── Fleet-wide list / summary ─────────────────────────────────────────────
//
// Operator pages (Detections, Playback Grid) need to render reports
// or per-camera totals across an entire workspace (or one site). The
// per-camera `list_for_camera` / per-site `summary_for_site` forced
// the frontend to fan out N queries — each one mints a fresh
// Airhouse credential and opens a fresh pgwire connection, so the
// cost was linear in fleet size even though the work itself isn't.
// These two functions collapse that to a single Airhouse round-trip
// regardless of camera/site count.

/// Resolve the camera universe the fleet read paths operate on,
/// honoring the workspace boundary and the optional site narrow.
/// Returned tuple: (cameras, site-ownership-error iff site_id given
/// belongs to another workspace).
async fn cameras_in_fleet_scope(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    site_id: Option<Uuid>,
) -> ServiceResult<Vec<crate::entities::cameras::Model>> {
    if let Some(sid) = site_id {
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
            .await
            .map_err(ServiceError::from)
    } else {
        let site_ids: Vec<Uuid> = sites::Entity::find()
            .filter(sites::Column::WorkspaceId.eq(workspace_id))
            .all(db)
            .await?
            .into_iter()
            .map(|s| s.id)
            .collect();
        if site_ids.is_empty() {
            return Ok(Vec::new());
        }
        cameras::Entity::find()
            .filter(cameras::Column::SiteId.is_in(site_ids))
            .all(db)
            .await
            .map_err(ServiceError::from)
    }
}

/// List compliance reports across the workspace (or one site),
/// newest first, with a single Airhouse round-trip. Drives the
/// Detections page.
pub async fn list_for_fleet(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    site_id: Option<Uuid>,
    since: Option<DateTime<Utc>>,
    limit: u32,
) -> ServiceResult<Vec<ComplianceReportRow>> {
    let cams = cameras_in_fleet_scope(db, workspace_id, site_id).await?;
    if cams.is_empty() {
        return Ok(Vec::new());
    }
    let effective_limit = limit.clamp(1, MAX_LIST_LIMIT);
    let camera_ids_sql = cams
        .iter()
        .map(|c| sql_str(&c.id.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(
        "SELECT report_id, camera_id, segment_start, segment_end, \
                trigger_type, trigger_track_id, vlm_model, report_text, \
                structured_json, frame_uri, tokens_used, evidence_s3_key, \
                detections_json, agreement_status, agreement_detail_json, \
                received_at \
         FROM oxy_cam_compliance_reports \
         WHERE camera_id IN ({camera_ids_sql})"
    );
    if let Some(ts) = since {
        let _ = write!(sql, " AND received_at >= {}", sql_ts(ts));
    }
    let _ = write!(sql, " ORDER BY received_at DESC LIMIT {effective_limit}");

    let client = crate::airhouse::connect_for_reads(workspace_id)
        .await
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_id,
                site_id = ?site_id,
                error = %e,
                "compliance.list_for_fleet: connect_for_reads failed"
            );
            ServiceError::Airhouse(e)
        })?;
    let msgs = match client.simple_query(&sql).await {
        Ok(m) => m,
        Err(e) => {
            let s = crate::airhouse::pg_error_text(&e);
            if crate::airhouse::is_missing_table(&s, "oxy_cam_compliance_reports") {
                tracing::debug!(
                    workspace_id = %workspace_id,
                    "compliance.list_for_fleet: schema not provisioned, returning empty"
                );
                return Ok(Vec::new());
            }
            tracing::warn!(
                workspace_id = %workspace_id,
                site_id = ?site_id,
                error = %s,
                "compliance.list_for_fleet: SELECT failed"
            );
            return Err(ServiceError::Airhouse(
                crate::airhouse::AirhouseError::Insert(s),
            ));
        }
    };
    let mut out = Vec::new();
    for msg in &msgs {
        if let SimpleQueryMessage::Row(r) = msg {
            out.push(row_to_report(r)?);
        }
    }
    Ok(out)
}

/// Per-camera totals across the workspace (or one site), one Airhouse
/// round-trip. Drives the Playback Grid view (replaces a per-site
/// fan-out of [`summary_for_site`]).
pub async fn summary_for_fleet(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    site_id: Option<Uuid>,
    since: Option<DateTime<Utc>>,
) -> ServiceResult<Vec<CameraComplianceSummary>> {
    let cams = cameras_in_fleet_scope(db, workspace_id, site_id).await?;
    if cams.is_empty() {
        return Ok(Vec::new());
    }
    // Resolve human-readable edge_box names — same shape as
    // `summary_for_site`. Workspace-scoped so we batch by every
    // distinct edge_box_id that appears in `cams`.
    let edge_box_ids: Vec<Uuid> = cams.iter().filter_map(|c| c.edge_box_id).collect();
    let edge_box_names: std::collections::HashMap<Uuid, String> = if edge_box_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        edge_boxes::Entity::find()
            .filter(edge_boxes::Column::Id.is_in(edge_box_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|eb| (eb.id, format!("{} ({})", eb.hardware_model, eb.cohort)))
            .collect()
    };

    let camera_ids_sql = cams
        .iter()
        .map(|c| sql_str(&c.id.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let since_clause = match since {
        Some(ts) => format!(" AND received_at >= {}", sql_ts(ts)),
        None => String::new(),
    };
    // Same defense-in-depth violation filter as `summary_for_site` —
    // see that function's comment for the rationale on the LIKE +
    // confidence floor combination.
    let sql = format!(
        "SELECT camera_id, \
                COUNT(*) AS total, \
                COUNT(*) FILTER (\
                    WHERE (\
                              structured_json LIKE '%\"attire_compliant\":false%' \
                           OR structured_json LIKE '%\"hygiene_compliant\":false%' \
                          ) \
                      AND COALESCE(CAST(json_extract(structured_json, '$.confidence') AS DOUBLE), 0.0) >= 0.5\
                ) AS violations, \
                MAX(received_at) AS last_at \
         FROM oxy_cam_compliance_reports \
         WHERE camera_id IN ({camera_ids_sql}){since_clause} \
         GROUP BY camera_id"
    );

    let client = crate::airhouse::connect_for_reads(workspace_id)
        .await
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_id,
                site_id = ?site_id,
                error = %e,
                "compliance.summary_for_fleet: connect_for_reads failed"
            );
            ServiceError::Airhouse(e)
        })?;
    let msgs = match client.simple_query(&sql).await {
        Ok(m) => m,
        Err(e) => {
            let s = crate::airhouse::pg_error_text(&e);
            if crate::airhouse::is_missing_table(&s, "oxy_cam_compliance_reports") {
                tracing::debug!(
                    workspace_id = %workspace_id,
                    "compliance.summary_for_fleet: schema not provisioned, all-zero rollup"
                );
                Vec::new()
            } else {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    site_id = ?site_id,
                    error = %s,
                    "compliance.summary_for_fleet: SELECT failed"
                );
                return Err(ServiceError::Airhouse(
                    crate::airhouse::AirhouseError::Insert(s),
                ));
            }
        }
    };

    let mut agg_by_camera: std::collections::HashMap<String, (u64, u64, Option<DateTime<Utc>>)> =
        std::collections::HashMap::new();
    for msg in &msgs {
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
            let last_at = get_opt_str(r, "last_at").and_then(|raw| parse_ducklake_ts(&raw));
            agg_by_camera.insert(cid, (total, violations, last_at));
        }
    }

    Ok(cams
        .into_iter()
        .map(|c| {
            let (total, violations, last_at) = agg_by_camera
                .remove(&c.id.to_string())
                .unwrap_or((0, 0, None));
            let edge_box_name = c
                .edge_box_id
                .and_then(|id| edge_box_names.get(&id).cloned());
            CameraComplianceSummary {
                camera_id: c.id.to_string(),
                camera_name: c.name,
                edge_box_name,
                total_reports: total,
                violations,
                last_received_at: last_at,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parse_ducklake_ts_handles_every_shape_we_see_in_the_wild() {
        // Expected value built once via chrono so we don't hand-compute
        // the epoch.
        let expect = Utc.with_ymd_and_hms(2026, 5, 28, 0, 55, 40).unwrap();

        // Naive SQL-style (what DuckLake actually returns over pgwire —
        // the shape that triggered the original `premature end of input`
        // bug on the Compliance tab read path).
        assert_eq!(parse_ducklake_ts("2026-05-28 00:55:40"), Some(expect));

        // With fractional seconds.
        let expect_ms = Utc
            .with_ymd_and_hms(2026, 5, 28, 0, 55, 40)
            .unwrap()
            .with_timezone(&Utc)
            + chrono::Duration::milliseconds(123);
        assert_eq!(
            parse_ducklake_ts("2026-05-28 00:55:40.123"),
            Some(expect_ms)
        );

        // RFC 3339 (what our INSERT path writes via sql_ts).
        assert_eq!(parse_ducklake_ts("2026-05-28T00:55:40Z"), Some(expect));

        // SQL-style with offset variants — they're all the same instant
        // in UTC, just rendered differently.
        for shape in [
            "2026-05-28 00:55:40+00",
            "2026-05-28 00:55:40+0000",
            "2026-05-28 00:55:40+00:00",
        ] {
            assert_eq!(parse_ducklake_ts(shape), Some(expect), "shape: {shape}");
        }

        // Garbage stays None instead of panicking.
        assert!(parse_ducklake_ts("not a timestamp").is_none());
        assert!(parse_ducklake_ts("").is_none());
    }
}
