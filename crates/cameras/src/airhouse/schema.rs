//! DuckLake / Airhouse DDL for the per-tenant camera-fleet tables.
//!
//! Tables are prefixed with `oxy_cam_` so they don't collide with
//! user-defined tables that may exist in the same Airhouse tenant.
//!
//! NOTE: DuckLake rejects `PRIMARY KEY`, `UNIQUE`, and `CREATE INDEX`.
//! Keep DDL to plain table definitions only. Query optimization relies
//! on DuckLake's predicate pushdown over partitioned parquet, not on
//! catalog-level indexes.

use tokio_postgres::Client;

use super::AirhouseError;

pub const CREATE_EVENTS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS oxy_cam_events (
    event_id      VARCHAR     NOT NULL,
    ts            TIMESTAMPTZ NOT NULL,
    camera_id     VARCHAR     NOT NULL,
    event_type    VARCHAR     NOT NULL,
    zone_id       VARCHAR     DEFAULT '',
    line_id       VARCHAR     DEFAULT '',
    track_id      VARCHAR     NOT NULL,
    dwell_seconds FLOAT,
    confidence    FLOAT,
    /* NOTE: on event_type='congestion' rows these two carry zone-level meaning
       (edge congestion.py): confidence = peak head-count in the zone,
       dwell_seconds = seconds the backup had sustained at flag time. The
       congestion_windows rollup reads them that way. */
    frame_uri     VARCHAR     DEFAULT '',
    /* S3 key (bucket-relative) for an archived clip of this event's
       window. Populated on `congestion` events when the edge archived
       the backed-up window (empty/NULL otherwise, incl. every
       enter/exit/dwell/line_cross row). Bucket is deployment-wide
       config; we persist only the key, like the compliance table. */
    evidence_s3_key VARCHAR   DEFAULT '',
    /* Free-text label for the event. On event_type='upsell_attempt' rows it
       carries the offered item the classifier detected ('avocado', 'salmon',
       'large size'); the upsell rollup matches it against the order's
       modifiers. Empty/NULL on every other event kind. */
    label         VARCHAR     DEFAULT '',
    received_at   TIMESTAMPTZ DEFAULT current_timestamp
)
"#;

pub const CREATE_CAMERA_HEALTH_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS oxy_cam_camera_health (
    camera_id       VARCHAR     NOT NULL,
    ts              TIMESTAMPTZ NOT NULL,
    fps             FLOAT,
    bitrate_kbps    FLOAT,
    last_frame_at   TIMESTAMPTZ,
    decoder_errors  INTEGER     DEFAULT 0,
    reconnect_count INTEGER     DEFAULT 0
)
"#;

pub const CREATE_BOX_HEALTH_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS oxy_cam_box_health (
    box_id              VARCHAR     NOT NULL,
    ts                  TIMESTAMPTZ NOT NULL,
    cpu_pct             FLOAT,
    gpu_pct             FLOAT,
    mem_pct             FLOAT,
    temp_c              FLOAT,
    uptime_s            BIGINT,
    image_tag           VARCHAR     DEFAULT '',
    containers_running  INTEGER
)
"#;

pub const CREATE_COMPLIANCE_REPORTS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS oxy_cam_compliance_reports (
    report_id        VARCHAR     NOT NULL,
    camera_id        VARCHAR     NOT NULL,
    segment_start    TIMESTAMPTZ NOT NULL,
    segment_end      TIMESTAMPTZ NOT NULL,
    trigger_type     VARCHAR     NOT NULL,
    trigger_track_id VARCHAR     DEFAULT '',
    vlm_model        VARCHAR     NOT NULL,
    report_text      VARCHAR     NOT NULL,
    structured_json  VARCHAR     DEFAULT '{}',
    frame_uri        VARCHAR     DEFAULT '',
    tokens_used      INTEGER,
    /* Tier B #6 — S3 key (bucket-relative) for the archived
       dwell-window clip. NULL when the worker didn't upload
       (no violation, no S3 configured, or upload failed). Bucket
       is deployment-wide config; we only persist the key so the
       deployment can rotate buckets without rewriting rows. */
    evidence_s3_key  VARCHAR     DEFAULT '',
    /* P3 (Option C) — PPE-YOLO bounding boxes captured by the edge
       worker on the same frame it sent to the VLM. JSON shape
       documented on `service::compliance::CompliancePayload`.
       Stored as VARCHAR for parity with `structured_json` (DuckLake
       JSON support is in flux; parsed on read). Empty array means
       "edge box ran YOLO and saw nothing"; missing/NULL means
       "edge box wasn't running YOLO yet" (older worker). */
    detections_json  VARCHAR     DEFAULT '[]',
    /* VLM ↔ YOLO agreement signal computed at ingest time. One of
       `agree` (both pipelines concur on every flagged class),
       `disagree` (at least one class differs),
       `inconclusive` (one or both sides didn't produce data we can
       compare). Filterable column so the operator UI can surface a
       "review queue" of disagreements cheaply. NULL on rows
       ingested before this column existed. */
    agreement_status      VARCHAR     DEFAULT NULL,
    /* Per-class breakdown the UI shows in the arbitration view.
       JSON shape: `{ "hat": "agree", "apron": "vlm_only_missing",
       "glove": "yolo_only_present", "person": "agree" }`. Always
       computed when both `structured_json` and `detections_json`
       carry data. Empty `{}` when the comparison was inconclusive. */
    agreement_detail_json VARCHAR     DEFAULT '{}',
    received_at      TIMESTAMPTZ DEFAULT current_timestamp
)
"#;

pub const CREATE_DEVICE_LOGS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS oxy_cam_device_logs (
    box_id        VARCHAR     NOT NULL,
    ts            TIMESTAMPTZ NOT NULL,
    severity      VARCHAR     NOT NULL,
    event         VARCHAR     NOT NULL,
    fields_json   VARCHAR     DEFAULT '{}',
    received_at   TIMESTAMPTZ DEFAULT current_timestamp
)
"#;

/// All DDL statements, in the order they should run. Each is
/// executed via simple-query (DuckLake doesn't support prepared
/// statements). All are idempotent.
/// Backfill `evidence_s3_key` on tenants provisioned before
/// Tier B #6. `IF NOT EXISTS` keeps the statement idempotent for
/// new tenants where [`CREATE_COMPLIANCE_REPORTS_TABLE`] already
/// included the column. DuckLake follows DuckDB's syntax here.
pub const ALTER_COMPLIANCE_ADD_EVIDENCE_KEY: &str = r#"
ALTER TABLE oxy_cam_compliance_reports
    ADD COLUMN IF NOT EXISTS evidence_s3_key VARCHAR DEFAULT ''
"#;

/// Backfill `evidence_s3_key` on `oxy_cam_events` for tenants
/// provisioned before congestion evidence clips. New tenants get it
/// from [`CREATE_EVENTS_TABLE`]; this keeps old tenants on the same
/// shape so the congestion event's key has a column to land in.
pub const ALTER_EVENTS_ADD_EVIDENCE_KEY: &str = r#"
ALTER TABLE oxy_cam_events
    ADD COLUMN IF NOT EXISTS evidence_s3_key VARCHAR DEFAULT ''
"#;

/// Backfill `label` on `oxy_cam_events` for tenants provisioned before
/// upsell detection. New tenants get it from [`CREATE_EVENTS_TABLE`];
/// this keeps old tenants on the same shape so `upsell_attempt`'s item
/// has a column to land in.
pub const ALTER_EVENTS_ADD_LABEL: &str = r#"
ALTER TABLE oxy_cam_events
    ADD COLUMN IF NOT EXISTS label VARCHAR DEFAULT ''
"#;

/// Backfill `detections_json` on tenants provisioned before P3
/// (Option C). New tenants get the column from the CREATE; this
/// keeps old tenants on the same shape.
pub const ALTER_COMPLIANCE_ADD_DETECTIONS: &str = r#"
ALTER TABLE oxy_cam_compliance_reports
    ADD COLUMN IF NOT EXISTS detections_json VARCHAR DEFAULT '[]'
"#;

/// Backfill the VLM-YOLO agreement signal columns on tenants
/// provisioned before Phase 1 of "utilizing disagreements." Rows
/// ingested before this point keep NULL `agreement_status` until
/// the next write rewrites them — no batch backfill, since the
/// historic computation would have to re-parse JSON anyway and the
/// dashboards we care about are rolling-window forward.
pub const ALTER_COMPLIANCE_ADD_AGREEMENT: &str = r#"
ALTER TABLE oxy_cam_compliance_reports
    ADD COLUMN IF NOT EXISTS agreement_status VARCHAR DEFAULT NULL
"#;

pub const ALTER_COMPLIANCE_ADD_AGREEMENT_DETAIL: &str = r#"
ALTER TABLE oxy_cam_compliance_reports
    ADD COLUMN IF NOT EXISTS agreement_detail_json VARCHAR DEFAULT '{}'
"#;

pub const ALL_DDL: &[&str] = &[
    CREATE_EVENTS_TABLE,
    CREATE_CAMERA_HEALTH_TABLE,
    CREATE_BOX_HEALTH_TABLE,
    CREATE_COMPLIANCE_REPORTS_TABLE,
    CREATE_DEVICE_LOGS_TABLE,
    // ALTERs run after the CREATEs and gracefully no-op when the
    // column already exists.
    ALTER_COMPLIANCE_ADD_EVIDENCE_KEY,
    ALTER_COMPLIANCE_ADD_DETECTIONS,
    ALTER_COMPLIANCE_ADD_AGREEMENT,
    ALTER_COMPLIANCE_ADD_AGREEMENT_DETAIL,
    ALTER_EVENTS_ADD_EVIDENCE_KEY,
    ALTER_EVENTS_ADD_LABEL,
];

/// Run the camera-fleet DDL inside a tenant. Idempotent; safe to call
/// on every ingest, but the [`super::connect_and_ensure`] cache means
/// in practice we only call it once per (workspace_id, process).
pub async fn ensure(client: &Client) -> Result<(), AirhouseError> {
    for stmt in ALL_DDL {
        client.simple_query(stmt).await.map_err(|e| {
            // Capture the server-side SQLSTATE + message when present —
            // a bare `{e}` from tokio-postgres flattens to "db error",
            // which leaves operators staring at the offending SQL with
            // no idea why DuckLake rejected it. The DbError source has
            // code / detail / hint that's much more actionable.
            let detail = super::pg_error_text(&e);
            AirhouseError::Ddl(format!("{detail}: {stmt}"))
        })?;
    }
    Ok(())
}
