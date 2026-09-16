//! Device log ingest + reader (IoT Phase 4b).
//!
//! Edge workers ship batches of structured log lines via
//! `POST /control/boxes/{id}/logs`; operators read them via
//! `GET /{wid}/cameras/edge-boxes/{id}/logs?severity=warn&since=...`.
//!
//! ## Why ship logs at all
//!
//! Field-deployed boxes are unreachable from the operator's
//! desk. SSHing in to read `docker logs` doesn't scale past 2-3
//! customers, and on Tailscale-only boxes it's often impossible.
//! Streaming logs into Airhouse lets the operator answer "why is
//! camera X failing at site Y?" without a return trip.
//!
//! ## Volume control
//!
//! At-source filtering is the first line of defense — the worker
//! drops `debug` by default unless a per-box `log_level` flag is
//! flipped. We rely on that for cost: ~50 boxes × 100 lines/min =
//! 5k rows/min, ~7M/day, well inside DuckLake write ceiling.
//!
//! Retention: the doc calls for severity-based sweeps (info 7d,
//! warn+ 30d). V1 ships *no* retention — the rows accumulate
//! until DuckLake's per-tenant quota or an operator's pocketbook
//! tells us to add it. A sweep job is a follow-up (single SQL
//! statement on a cron, but needs a worker we haven't wired yet).

use crate::airhouse::escape::{sql_str, sql_ts};
use crate::service::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use uuid::Uuid;

/// One log line as the device sends it. `fields_json` is opaque
/// to us — the operator sees the same JSON the worker emitted
/// via `log("info", "camera.vlm_call", camera=..., dwell=...)`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogPayload {
    pub ts: DateTime<Utc>,
    pub severity: String,
    pub event: String,
    /// Pre-encoded JSON object; we don't re-parse, just store. The
    /// reader returns the string verbatim; the UI parses for
    /// display.
    #[serde(default)]
    pub fields_json: String,
}

/// Result of an ingest batch — count of rows actually written.
/// Mirrors `service::ingest::IngestResult` but kept separate so a
/// future change to one doesn't drag the other.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct IngestResult {
    pub accepted: usize,
}

/// Insert a batch from one device. Empty batches short-circuit
/// without a DB round-trip. Each row's `severity` is validated
/// against a fixed allowlist so a hostile or buggy worker can't
/// store arbitrary strings that would later confuse the reader's
/// filter.
pub async fn write_batch(
    workspace_id: Uuid,
    edge_box_id: Uuid,
    rows: Vec<LogPayload>,
) -> ServiceResult<IngestResult> {
    if rows.is_empty() {
        return Ok(IngestResult { accepted: 0 });
    }
    for r in &rows {
        if !is_valid_severity(&r.severity) {
            return Err(ServiceError::InvalidInput(format!(
                "invalid severity `{}` — expected debug/info/warn/error",
                r.severity
            )));
        }
    }

    let client = crate::airhouse::connect_and_ensure(workspace_id).await?;
    let mut sql = String::from(
        "INSERT INTO oxy_cam_device_logs \
         (box_id, ts, severity, event, fields_json) VALUES ",
    );
    let box_id_str = sql_str(&edge_box_id.to_string());
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        // Empty fields_json round-trips to '{}' via the column
        // default; we pass it explicitly so DuckLake doesn't
        // complain about column count mismatch.
        let fields = if r.fields_json.is_empty() {
            "{}"
        } else {
            r.fields_json.as_str()
        };
        let _ = write!(
            sql,
            "({}, {}, {}, {}, {})",
            box_id_str,
            sql_ts(r.ts),
            sql_str(&r.severity),
            sql_str(&r.event),
            sql_str(fields),
        );
    }

    client.simple_query(&sql).await.map_err(|e| {
        ServiceError::Airhouse(crate::airhouse::AirhouseError::Insert(
            crate::airhouse::pg_error_text(&e),
        ))
    })?;
    Ok(IngestResult {
        accepted: rows.len(),
    })
}

/// One row as the operator UI sees it.
#[derive(Debug, Clone, Serialize)]
pub struct LogRow {
    pub box_id: Uuid,
    pub ts: DateTime<Utc>,
    pub severity: String,
    pub event: String,
    pub fields_json: String,
    pub received_at: DateTime<Utc>,
}

/// Filters for [`list_for_box`]. All optional; missing means "no
/// filter on this axis."
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LogListFilter {
    /// `info`, `warn`, `error` — rows at or above this severity
    /// are returned. Missing = include all severities (including
    /// debug, which the worker rarely sends).
    pub min_severity: Option<String>,
    /// Substring match on `event` (case-sensitive — events are
    /// canonical snake_case so case-folding adds noise without
    /// helping find anything).
    pub event_contains: Option<String>,
    /// Only rows with `received_at >= since`. Operator UI uses
    /// this for the "tail" toggle to skip what it's already seen.
    pub since: Option<DateTime<Utc>>,
    /// Cap on returned rows. Defaults to 200, hard cap 1000 —
    /// browser UIs choke past that.
    pub limit: Option<u32>,
}

const DEFAULT_LIMIT: u32 = 200;
const MAX_LIMIT: u32 = 1000;

/// Read logs for one edge box. Ownership chain via the route
/// layer (the operator route resolves edge_box → site →
/// workspace_id before calling us); this fn trusts `workspace_id`
/// and `edge_box_id` are both already validated together.
///
/// A missing log table (fresh workspace, no device has shipped
/// anything yet) collapses to an empty result rather than
/// erroring — the UI shouldn't blow up just because the table
/// hasn't been provisioned.
pub async fn list_for_box(
    workspace_id: Uuid,
    edge_box_id: Uuid,
    filter: LogListFilter,
) -> ServiceResult<Vec<LogRow>> {
    let limit = filter.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let client = match crate::airhouse::connect_for_reads(workspace_id).await {
        Ok(c) => c,
        Err(crate::airhouse::AirhouseError::Disabled) => {
            // Deployment without Airhouse (local dev, demo, or a
            // fresh workspace that never finished provisioning).
            // The reader is non-essential — the operator just sees
            // "no logs yet" instead of a 503 banner.
            tracing::debug!(
                workspace_id = %workspace_id,
                edge_box_id = %edge_box_id,
                "logs.list_for_box: airhouse disabled, returning empty"
            );
            return Ok(Vec::new());
        }
        Err(e) => return Err(ServiceError::Airhouse(e)),
    };
    let mut sql = format!(
        "SELECT box_id, ts, severity, event, fields_json, received_at \
         FROM oxy_cam_device_logs \
         WHERE box_id = {}",
        sql_str(&edge_box_id.to_string()),
    );
    if let Some(min_sev) = filter.min_severity.as_deref() {
        // Severity is text in the schema, not an enum, so we build
        // an IN list of acceptable values. Keeps the SQL flat (no
        // joins to a numeric severity table) at the cost of one
        // extra IN expansion per query.
        let allowed = severities_at_or_above(min_sev);
        let list = allowed
            .iter()
            .map(|s| sql_str(s))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(sql, " AND severity IN ({list})");
    }
    if let Some(needle) = filter.event_contains.as_deref().filter(|s| !s.is_empty()) {
        // `%` and `_` need escaping for LIKE — substring match on
        // structured event names is rare to need wildcards.
        let escaped = needle
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let _ = write!(
            sql,
            " AND event LIKE {} ESCAPE '\\'",
            sql_str(&format!("%{escaped}%"))
        );
    }
    if let Some(since) = filter.since {
        let _ = write!(sql, " AND received_at >= {}", sql_ts(since));
    }
    let _ = write!(sql, " ORDER BY ts DESC LIMIT {limit}");

    let msgs = match client.simple_query(&sql).await {
        Ok(m) => m,
        Err(e) => {
            let s = crate::airhouse::pg_error_text(&e);
            if crate::airhouse::is_missing_table(&s, "oxy_cam_device_logs") {
                tracing::debug!(
                    workspace_id = %workspace_id,
                    edge_box_id = %edge_box_id,
                    "logs.list_for_box: schema not provisioned, returning empty"
                );
                return Ok(Vec::new());
            }
            tracing::warn!(
                workspace_id = %workspace_id,
                edge_box_id = %edge_box_id,
                error = %s,
                "logs.list_for_box: SELECT failed"
            );
            return Err(ServiceError::Airhouse(
                crate::airhouse::AirhouseError::Connect(format!("query failed: {s}")),
            ));
        }
    };

    let mut out = Vec::new();
    for msg in msgs {
        if let tokio_postgres::SimpleQueryMessage::Row(row) = msg {
            let parsed = LogRow {
                box_id: row
                    .get("box_id")
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .unwrap_or(edge_box_id),
                ts: row
                    .get("ts")
                    .and_then(parse_airhouse_ts)
                    .unwrap_or_else(Utc::now),
                severity: row.get("severity").unwrap_or("").to_string(),
                event: row.get("event").unwrap_or("").to_string(),
                fields_json: row.get("fields_json").unwrap_or("{}").to_string(),
                received_at: row
                    .get("received_at")
                    .and_then(parse_airhouse_ts)
                    .unwrap_or_else(Utc::now),
            };
            out.push(parsed);
        }
    }
    Ok(out)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn is_valid_severity(s: &str) -> bool {
    matches!(s, "debug" | "info" | "warn" | "warning" | "error")
}

/// Return the severities at or above `min`. `"info"` includes
/// info/warn/error; `"warn"` includes warn/error; etc. Used to
/// build the WHERE-clause IN list for [`list_for_box`].
fn severities_at_or_above(min: &str) -> Vec<&'static str> {
    match min {
        "debug" => vec!["debug", "info", "warn", "warning", "error"],
        "info" => vec!["info", "warn", "warning", "error"],
        "warn" | "warning" => vec!["warn", "warning", "error"],
        "error" => vec!["error"],
        // Unknown — accept everything rather than reject; the route
        // layer should never let unknown values through but the
        // SQL builder shouldn't crash if it does.
        _ => vec!["debug", "info", "warn", "warning", "error"],
    }
}

/// DuckLake/Postgres TIMESTAMPTZ comes back as a string we have
/// to parse. Tolerant of `2026-05-29 02:00:45.123+00` (no `T`)
/// and the more standard RFC-3339 form.
fn parse_airhouse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .or_else(|| {
            // DuckLake uses `YYYY-MM-DD HH:MM:SS[.fff]+TZ` (space).
            let with_t = s.replacen(' ', "T", 1);
            DateTime::parse_from_rfc3339(&with_t).ok()
        })
        .map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_validation_allowlist() {
        assert!(is_valid_severity("info"));
        assert!(is_valid_severity("warn"));
        assert!(is_valid_severity("warning"));
        assert!(is_valid_severity("error"));
        assert!(is_valid_severity("debug"));
        assert!(!is_valid_severity("CRITICAL")); // case-sensitive
        assert!(!is_valid_severity(""));
        assert!(!is_valid_severity("trace"));
    }

    #[test]
    fn severities_at_or_above_orders_correctly() {
        assert_eq!(
            severities_at_or_above("warn"),
            vec!["warn", "warning", "error"]
        );
        assert_eq!(severities_at_or_above("error"), vec!["error"]);
        // Unknown falls back to all — defense, not policy.
        assert_eq!(severities_at_or_above("nope").len(), 5);
    }

    #[test]
    fn parse_airhouse_ts_accepts_both_forms() {
        // Standard RFC 3339.
        assert!(parse_airhouse_ts("2026-05-29T02:00:45Z").is_some());
        // DuckLake's space-separated variant.
        assert!(parse_airhouse_ts("2026-05-29 02:00:45+00:00").is_some());
        // Garbage → None.
        assert!(parse_airhouse_ts("not a date").is_none());
    }
}
