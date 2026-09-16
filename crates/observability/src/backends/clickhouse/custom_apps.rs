//! Custom-app wide events and function logs — inserts, plus the availability
//! read the SLI is computed from.
//!
//! See `internal-docs/2026-09-04-custom-app-observability-design.md` §3.
//!
//! Every query here is scoped `org_id` **and** `app_id` and leads with them, in
//! that order, because that is the table's sort key. A query that filtered on
//! `app_id` alone would be correct and would also scan every tenant's parts.

use std::collections::HashMap;

use clickhouse::Row;
use oxy_shared::errors::OxyError;
use serde::{Deserialize, Serialize};

use super::ClickHouseObservabilityStorage;
use crate::types::{
    AppAvailabilityWindow, ClientErrorGroup, CustomAppClientErrorRecord, CustomAppEventRecord,
    CustomAppLogRecord, FunctionLogRow,
};

/// ANSI single-quote doubling — the only escape ClickHouse accepts
/// unconditionally. Backslash escapes depend on
/// `allow_backslash_escaping_in_strings`, which is off by default in
/// ClickHouse >= 22.4 and would silently produce malformed literals.
fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

#[derive(Debug, Serialize, Row)]
struct CustomAppEventInsertRow {
    /// Unix milliseconds (`DateTime64(3)` is an Int64 on the wire).
    timestamp: i64,
    org_id: String,
    app_id: String,
    build_id: String,
    request_id: String,
    session_id: String,
    user_id: String,
    kind: String,
    route: String,
    status: u16,
    duration_ms: u32,
    host_calls: u32,
    init_ms: u32,
    bytes: u64,
    app_role: String,
    outcome: String,
    error_kind: String,
    error_detail: String,
    trace_id: String,
    span_id: String,
}

#[derive(Debug, Serialize, Row)]
struct CustomAppLogInsertRow {
    timestamp: i64,
    org_id: String,
    app_id: String,
    build_id: String,
    invocation_id: String,
    request_id: String,
    function_name: String,
    mode: String,
    log_level: String,
    seq: u32,
    message: String,
    trace_id: String,
    span_id: String,
}

#[derive(Debug, Serialize, Row)]
struct ClientErrorInsertRow {
    timestamp: i64,
    org_id: String,
    app_id: String,
    build_id: String,
    session_id: String,
    user_id: String,
    error_name: String,
    message: String,
    stack: String,
    stack_hash: String,
    path: String,
    kind: String,
    user_agent: String,
    trace_id: String,
    span_id: String,
}

#[derive(Debug, Deserialize, Row)]
struct ClientErrorGroupRow {
    stack_hash: String,
    error_name: String,
    message: String,
    stack: String,
    build_id: String,
    path: String,
    kind: String,
    occurrences: u64,
    sessions: u64,
    first_seen: String,
    last_seen: String,
}

#[derive(Debug, Deserialize, Row)]
struct FunctionLogQueryRow {
    timestamp: String,
    build_id: String,
    invocation_id: String,
    request_id: String,
    function_name: String,
    mode: String,
    log_level: String,
    seq: u32,
    message: String,
    trace_id: String,
}

#[derive(Debug, Deserialize, Row)]
struct AvailabilityRow {
    total: u64,
    failed: u64,
}

/// The same counts, plus the app they belong to, for the grouped fleet queries.
///
/// The baseline query has no failure notion and selects a literal `0` for
/// `failed` rather than getting its own row type — one shape for both keeps the
/// two fleet queries readable side by side.
#[derive(Debug, Deserialize, Row)]
struct FleetAvailabilityRow {
    app_id: String,
    total: u64,
    failed: u64,
}

/// Longest a single log line may be. A function that dumps a 4 MB response body
/// into `ctx.log` should cost one truncated row, not a multi-megabyte insert
/// repeated per line — and the truncation is visible in the stored text rather
/// than silent.
const MAX_LOG_MESSAGE_BYTES: usize = 8 * 1024;

/// Truncate on a char boundary, marking that it happened.
fn clamp_message(message: String) -> String {
    clamp_to(message, MAX_LOG_MESSAGE_BYTES)
}

fn clamp_to(mut message: String, limit: usize) -> String {
    if message.len() <= limit {
        return message;
    }
    let mut cut = limit;
    while cut > 0 && !message.is_char_boundary(cut) {
        cut -= 1;
    }
    message.truncate(cut);
    message.push_str("… [truncated]");
    message
}

pub(super) async fn insert_custom_app_events(
    storage: &ClickHouseObservabilityStorage,
    events: Vec<CustomAppEventRecord>,
) -> Result<(), OxyError> {
    if events.is_empty() {
        return Ok(());
    }

    let mut insert = storage
        .client()
        .insert::<CustomAppEventInsertRow>("custom_app_events")
        .await
        .map_err(|e| OxyError::RuntimeError(format!("ClickHouse insert init failed: {e}")))?;

    for e in events {
        let row = CustomAppEventInsertRow {
            timestamp: e.timestamp_ms,
            org_id: e.org_id,
            app_id: e.app_id,
            build_id: e.build_id,
            request_id: e.request_id,
            session_id: e.session_id,
            user_id: e.user_id,
            kind: e.kind,
            route: e.route,
            status: e.status,
            duration_ms: e.duration_ms,
            host_calls: e.host_calls,
            init_ms: e.init_ms,
            bytes: e.bytes,
            app_role: e.app_role,
            outcome: e.outcome,
            error_kind: e.error_kind,
            error_detail: e.error_detail,
            trace_id: e.trace_id,
            span_id: e.span_id,
        };
        insert.write(&row).await.map_err(|err| {
            OxyError::RuntimeError(format!("ClickHouse custom_app_events write failed: {err}"))
        })?;
    }

    insert.end().await.map_err(|e| {
        OxyError::RuntimeError(format!("ClickHouse custom_app_events insert failed: {e}"))
    })
}

pub(super) async fn insert_custom_app_logs(
    storage: &ClickHouseObservabilityStorage,
    logs: Vec<CustomAppLogRecord>,
) -> Result<(), OxyError> {
    if logs.is_empty() {
        return Ok(());
    }

    let mut insert = storage
        .client()
        .insert::<CustomAppLogInsertRow>("custom_app_logs")
        .await
        .map_err(|e| OxyError::RuntimeError(format!("ClickHouse insert init failed: {e}")))?;

    for l in logs {
        let row = CustomAppLogInsertRow {
            timestamp: l.timestamp_ms,
            org_id: l.org_id,
            app_id: l.app_id,
            build_id: l.build_id,
            invocation_id: l.invocation_id,
            request_id: l.request_id,
            function_name: l.function_name,
            mode: l.mode,
            log_level: l.log_level,
            seq: l.seq,
            message: clamp_message(l.message),
            trace_id: l.trace_id,
            span_id: l.span_id,
        };
        insert.write(&row).await.map_err(|err| {
            OxyError::RuntimeError(format!("ClickHouse custom_app_logs write failed: {err}"))
        })?;
    }

    insert.end().await.map_err(|e| {
        OxyError::RuntimeError(format!("ClickHouse custom_app_logs insert failed: {e}"))
    })
}

/// A stack can be long and this table is read by a human, not replayed. 16 KB is
/// several dozen frames — past that the tail is framework internals.
const MAX_STACK_BYTES: usize = 16 * 1024;

pub(super) async fn insert_custom_app_client_errors(
    storage: &ClickHouseObservabilityStorage,
    errors: Vec<CustomAppClientErrorRecord>,
) -> Result<(), OxyError> {
    if errors.is_empty() {
        return Ok(());
    }

    let mut insert = storage
        .client()
        .insert::<ClientErrorInsertRow>("custom_app_client_errors")
        .await
        .map_err(|e| OxyError::RuntimeError(format!("ClickHouse insert init failed: {e}")))?;

    for e in errors {
        let row = ClientErrorInsertRow {
            timestamp: e.timestamp_ms,
            org_id: e.org_id,
            app_id: e.app_id,
            build_id: e.build_id,
            session_id: e.session_id,
            user_id: e.user_id,
            error_name: e.error_name,
            message: clamp_to(e.message, MAX_LOG_MESSAGE_BYTES),
            stack: clamp_to(e.stack, MAX_STACK_BYTES),
            stack_hash: e.stack_hash,
            path: e.path,
            kind: e.kind,
            user_agent: e.user_agent,
            trace_id: e.trace_id,
            span_id: e.span_id,
        };
        insert.write(&row).await.map_err(|err| {
            OxyError::RuntimeError(format!(
                "ClickHouse custom_app_client_errors write failed: {err}"
            ))
        })?;
    }

    insert.end().await.map_err(|e| {
        OxyError::RuntimeError(format!(
            "ClickHouse custom_app_client_errors insert failed: {e}"
        ))
    })
}

/// Distinct client errors over a window, most-recent-first.
///
/// Grouped by `stack_hash` rather than listed raw: the same fault recurring is
/// one problem with a count, not N problems. `argMax(..., timestamp)` picks the
/// most recent occurrence's text for each group, so the sample shown is the one
/// an engineer would get by reproducing now rather than whichever landed first.
fn client_errors_sql(org_id: &str, app_id: &str, hours: u32, limit: u32, build_id: &str) -> String {
    let build_clause = if build_id.is_empty() {
        String::new()
    } else {
        format!(" AND build_id = '{}'", escape_sql_literal(build_id))
    };
    format!(
        "SELECT stack_hash, \
         argMax(error_name, timestamp) AS error_name, \
         argMax(message, timestamp) AS message, \
         argMax(stack, timestamp) AS stack, \
         argMax(build_id, timestamp) AS build_id, \
         argMax(path, timestamp) AS path, \
         argMax(kind, timestamp) AS kind, \
         count() AS occurrences, \
         uniqExact(session_id) AS sessions, \
         {first_seen} AS first_seen, \
         {last_seen} AS last_seen \
         FROM custom_app_client_errors \
         WHERE org_id = '{org}' AND app_id = '{app}' \
         AND timestamp >= now() - INTERVAL {hours} HOUR{build_clause} \
         GROUP BY stack_hash \
         ORDER BY max(timestamp) DESC \
         LIMIT {limit}",
        org = escape_sql_literal(org_id),
        app = escape_sql_literal(app_id),
        // The crate has ONE helper for this, and it exists because a bare
        // `formatDateTime` omits the `'UTC'` arg: on a ClickHouse whose server
        // timezone is not UTC the value renders in server-local time and then
        // gets a literal `Z` stapled on, so the browser parses a consistent,
        // plausible, wrong instant. That is the failure nobody notices during
        // an incident.
        first_seen = super::iso_utc("min(timestamp)"),
        last_seen = super::iso_utc("max(timestamp)"),
    )
}

pub(super) async fn get_client_errors(
    storage: &ClickHouseObservabilityStorage,
    org_id: &str,
    app_id: &str,
    hours: u32,
    limit: u32,
    build_id: &str,
) -> Result<Vec<ClientErrorGroup>, OxyError> {
    let sql = client_errors_sql(org_id, app_id, hours, limit, build_id);
    let rows = storage
        .read_client()
        .query(&sql)
        .fetch_all::<ClientErrorGroupRow>()
        .await
        .map_err(|e| {
            OxyError::RuntimeError(format!("ClickHouse client-error query failed: {e}"))
        })?;
    Ok(rows
        .into_iter()
        .map(|r| ClientErrorGroup {
            stack_hash: r.stack_hash,
            error_name: r.error_name,
            message: r.message,
            stack: r.stack,
            build_id: r.build_id,
            path: r.path,
            kind: r.kind,
            occurrences: r.occurrences,
            sessions: r.sessions,
            first_seen: r.first_seen,
            last_seen: r.last_seen,
        })
        .collect())
}

/// Function log lines over a window, newest first.
///
/// `seq DESC` after `timestamp DESC` so the lines of one invocation stay in the
/// order the function wrote them once the page is reversed for display — a
/// millisecond holds many log lines, and ordering by timestamp alone shuffles
/// them.
fn function_logs_sql(
    org_id: &str,
    app_id: &str,
    hours: u32,
    limit: u32,
    invocation_id: &str,
    request_id: &str,
) -> String {
    let invocation_clause = if invocation_id.is_empty() {
        String::new()
    } else {
        format!(
            " AND invocation_id = '{}'",
            escape_sql_literal(invocation_id)
        )
    };
    let request_clause = if request_id.is_empty() {
        String::new()
    } else {
        format!(" AND request_id = '{}'", escape_sql_literal(request_id))
    };
    format!(
        "SELECT {ts} AS timestamp, \
         build_id, invocation_id, request_id, function_name, mode, log_level, seq, message, \
         trace_id \
         FROM custom_app_logs \
         WHERE org_id = '{org}' AND app_id = '{app}' \
         AND timestamp >= now() - INTERVAL {hours} HOUR{invocation_clause}{request_clause} \
         ORDER BY timestamp DESC, seq DESC \
         LIMIT {limit}",
        org = escape_sql_literal(org_id),
        app = escape_sql_literal(app_id),
        // Same helper, same reason — see `client_errors_sql`.
        ts = super::iso_utc("timestamp"),
    )
}

pub(super) async fn get_function_logs(
    storage: &ClickHouseObservabilityStorage,
    org_id: &str,
    app_id: &str,
    hours: u32,
    limit: u32,
    invocation_id: &str,
    request_id: &str,
) -> Result<Vec<FunctionLogRow>, OxyError> {
    let sql = function_logs_sql(org_id, app_id, hours, limit, invocation_id, request_id);
    let rows = storage
        .read_client()
        .query(&sql)
        .fetch_all::<FunctionLogQueryRow>()
        .await
        .map_err(|e| {
            OxyError::RuntimeError(format!("ClickHouse function-log query failed: {e}"))
        })?;
    Ok(rows
        .into_iter()
        .map(|r| FunctionLogRow {
            timestamp: r.timestamp,
            build_id: r.build_id,
            invocation_id: r.invocation_id,
            request_id: r.request_id,
            function_name: r.function_name,
            mode: r.mode,
            log_level: r.log_level,
            seq: r.seq,
            message: r.message,
            trace_id: r.trace_id,
        })
        .collect())
}

/// The surfaces that count toward availability, as a SQL fragment.
///
/// One definition, shared by the per-app SLI and the fleet roll-up. Two copies
/// would eventually disagree, and the symptom would be the fleet table and the
/// app's own Availability panel reporting different numbers for the same app on
/// the same screen — with nothing to say which is right.
///
/// `asset` is excluded on purpose: a browser cancelling an image is not the app
/// being unavailable, and asset volume outnumbers page loads by enough to bury a
/// real shell failure. `client` is excluded because a browser-reported event is
/// already counted as the serve it belongs to.
const AVAILABILITY_KINDS: &str = "kind IN ('serve', 'fn', 'data')";

/// SQL for one availability window. Pure, so the shape can be regression-tested
/// without a live ClickHouse — which is the only way the `outcome != 'ok'`
/// clause below stays honest.
fn availability_sql(org_id: &str, app_id: &str, window_minutes: u32) -> String {
    format!(
        "SELECT count() AS total, \
         countIf(outcome != 'ok') AS failed \
         FROM custom_app_events \
         WHERE org_id = '{org}' AND app_id = '{app}' \
         AND timestamp >= now() - INTERVAL {minutes} MINUTE \
         AND {AVAILABILITY_KINDS}",
        org = escape_sql_literal(org_id),
        app = escape_sql_literal(app_id),
        minutes = window_minutes,
    )
}

/// `(org_id, app_id) IN (('o','a'), …)` — the fleet predicate.
///
/// A tuple `IN` rather than `app_id IN (…)`, because the table's sort key is
/// `(org_id, app_id, timestamp)` and it is org-first. Filtering on `app_id`
/// alone would be correct and would scan every tenant's parts — the same trap
/// the per-app query's own test guards against, and it matters more here, where
/// one query covers the whole fleet.
fn fleet_predicate(apps: &[(String, String)]) -> String {
    let pairs = apps
        .iter()
        .map(|(org, app)| {
            format!(
                "('{}','{}')",
                escape_sql_literal(org),
                escape_sql_literal(app)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("(org_id, app_id) IN ({pairs})")
}

/// SQL for one availability window across many apps at once.
///
/// The per-app query runs one statement per window; doing that per app would be
/// `windows × apps` round trips for a page of the fleet table. This is one
/// statement per window, grouped by app.
fn fleet_availability_sql(apps: &[(String, String)], window_minutes: u32) -> String {
    format!(
        "SELECT app_id, count() AS total, \
         countIf(outcome != 'ok') AS failed \
         FROM custom_app_events \
         WHERE {predicate} \
         AND timestamp >= now() - INTERVAL {window_minutes} MINUTE \
         AND {AVAILABILITY_KINDS} \
         GROUP BY app_id",
        predicate = fleet_predicate(apps),
    )
}

/// SQL for the heartbeat baseline: the same window, one cycle back.
///
/// **Both** bounds shift. A predicate with only a lower bound would compare the
/// current window against everything since a week ago — a week of traffic
/// against six hours of it — and no live app would ever read as silent.
fn fleet_baseline_sql(apps: &[(String, String)], window_minutes: u32, cycle_days: u32) -> String {
    format!(
        "SELECT app_id, count() AS total, \
         toUInt64(0) AS failed \
         FROM custom_app_events \
         WHERE {predicate} \
         AND timestamp >= now() - INTERVAL {cycle_days} DAY - INTERVAL {window_minutes} MINUTE \
         AND timestamp < now() - INTERVAL {cycle_days} DAY \
         AND {AVAILABILITY_KINDS} \
         GROUP BY app_id",
        predicate = fleet_predicate(apps),
    )
}

/// Success/failure counts for one app across several windows.
///
/// Windows are queried independently rather than derived from one another: a
/// burn-rate rule compares a short window against a long one, and computing the
/// short from a bucketed long window rounds exactly the spike it exists to
/// catch.
pub(super) async fn get_app_availability(
    storage: &ClickHouseObservabilityStorage,
    org_id: &str,
    app_id: &str,
    windows_minutes: &[u32],
) -> Result<Vec<AppAvailabilityWindow>, OxyError> {
    let mut out = Vec::with_capacity(windows_minutes.len());
    for window in windows_minutes {
        let sql = availability_sql(org_id, app_id, *window);
        let rows = storage
            .read_client()
            .query(&sql)
            .fetch_all::<AvailabilityRow>()
            .await
            .map_err(|e| {
                OxyError::RuntimeError(format!("ClickHouse availability query failed: {e}"))
            })?;
        let row = rows.first();
        out.push(AppAvailabilityWindow {
            window_minutes: *window,
            total: row.map(|r| r.total).unwrap_or(0),
            failed: row.map(|r| r.failed).unwrap_or(0),
        });
    }
    Ok(out)
}

/// Availability counts for many apps at once, keyed by `app_id`.
///
/// An app absent from the result served nothing in that window — ClickHouse
/// returns no group for it. The caller must read that as a zero rather than as
/// missing data, which is why this returns totals rather than an `Option`: the
/// distinction between "served nothing" and "we did not ask" belongs to whether
/// this function returned `Ok` at all.
pub(super) async fn get_fleet_availability(
    storage: &ClickHouseObservabilityStorage,
    apps: &[(String, String)],
    windows_minutes: &[u32],
) -> Result<HashMap<String, Vec<AppAvailabilityWindow>>, OxyError> {
    // An empty `IN ()` is a ClickHouse syntax error, and an empty fleet is a
    // perfectly ordinary state (a dev box with no published apps).
    if apps.is_empty() {
        return Ok(HashMap::new());
    }

    let mut out: HashMap<String, Vec<AppAvailabilityWindow>> = apps
        .iter()
        .map(|(_, app_id)| (app_id.clone(), Vec::with_capacity(windows_minutes.len())))
        .collect();

    for window in windows_minutes {
        let rows = storage
            .read_client()
            .query(&fleet_availability_sql(apps, *window))
            .fetch_all::<FleetAvailabilityRow>()
            .await
            .map_err(|e| {
                OxyError::RuntimeError(format!("ClickHouse fleet availability query failed: {e}"))
            })?;
        let by_app: HashMap<&str, &FleetAvailabilityRow> =
            rows.iter().map(|r| (r.app_id.as_str(), r)).collect();

        // Iterate the requested apps, not the returned rows: every app must end
        // with one entry per window, including the ones that served nothing.
        for (_, app_id) in apps {
            let row = by_app.get(app_id.as_str());
            if let Some(windows) = out.get_mut(app_id) {
                windows.push(AppAvailabilityWindow {
                    window_minutes: *window,
                    total: row.map(|r| r.total).unwrap_or(0),
                    failed: row.map(|r| r.failed).unwrap_or(0),
                });
            }
        }
    }
    Ok(out)
}

/// Request counts for the same window one cycle back, keyed by `app_id`, for
/// the absence-of-traffic heartbeat.
///
/// An app absent from the result had no traffic in that window a cycle ago,
/// which [`crate::heartbeat::evaluate`] reads as "no baseline" — no rhythm to
/// have broken.
pub(super) async fn get_fleet_heartbeat_baseline(
    storage: &ClickHouseObservabilityStorage,
    apps: &[(String, String)],
    window_minutes: u32,
    cycle_days: u32,
) -> Result<HashMap<String, u64>, OxyError> {
    if apps.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = storage
        .read_client()
        .query(&fleet_baseline_sql(apps, window_minutes, cycle_days))
        .fetch_all::<FleetAvailabilityRow>()
        .await
        .map_err(|e| {
            OxyError::RuntimeError(format!("ClickHouse heartbeat baseline query failed: {e}"))
        })?;
    Ok(rows.into_iter().map(|r| (r.app_id, r.total)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Availability counts **every** non-`ok` outcome, not just 5xx. The whole
    /// point of storing `outcome` beside `status` is that a white-screened app
    /// serves 200s: a `status >= 500` predicate would report it as perfectly
    /// available, which is the failure this table was added to catch.
    #[test]
    fn availability_counts_every_failure_class_not_just_5xx() {
        let sql = availability_sql("org", "app", 60);
        assert!(
            sql.contains("countIf(outcome != 'ok')"),
            "availability must be outcome-driven, not status-driven: {sql}"
        );
        assert!(
            !sql.contains("status >= 500"),
            "a status predicate misses the implicit (white-screen) failure class: {sql}"
        );
    }

    /// The sort key is `(org_id, app_id, timestamp)`. Filtering on app alone
    /// would be correct and would scan every tenant.
    #[test]
    fn availability_is_scoped_by_org_and_app() {
        let sql = availability_sql("acme", "app-1", 5);
        assert!(sql.contains("org_id = 'acme'"), "{sql}");
        assert!(sql.contains("app_id = 'app-1'"), "{sql}");
        assert!(sql.contains("INTERVAL 5 MINUTE"), "{sql}");
    }

    /// Assets are excluded from the SLI on purpose: a browser cancelling an
    /// image request is not the app being unavailable, and asset volume
    /// outnumbers page loads by enough to bury a real shell failure.
    #[test]
    fn availability_excludes_assets_and_client_beacons() {
        let sql = availability_sql("o", "a", 60);
        assert!(sql.contains("kind IN ('serve', 'fn', 'data')"), "{sql}");
    }

    #[test]
    fn quotes_in_an_id_cannot_break_out_of_the_literal() {
        let sql = availability_sql("o'; DROP TABLE custom_app_events; --", "a", 60);
        assert!(sql.contains("''; DROP"), "quote must be doubled: {sql}");
    }

    #[test]
    fn long_log_lines_are_truncated_on_a_char_boundary() {
        let message = "é".repeat(MAX_LOG_MESSAGE_BYTES);
        let clamped = clamp_message(message);
        assert!(clamped.ends_with("… [truncated]"));
        // Round-trips as UTF-8 — the truncation did not split a code point.
        assert_eq!(
            clamped,
            String::from_utf8(clamped.clone().into_bytes()).unwrap()
        );
    }

    #[test]
    fn short_log_lines_are_left_alone() {
        assert_eq!(clamp_message("hello".into()), "hello");
    }

    /// An app with no traffic has no availability opinion. Reporting 100% for a
    /// silent app is how a dead app pages nobody.
    #[test]
    fn an_empty_window_has_no_failure_ratio() {
        let window = AppAvailabilityWindow {
            window_minutes: 5,
            total: 0,
            failed: 0,
        };
        assert_eq!(window.failure_ratio(), None);
    }

    // ── Fleet queries ────────────────────────────────────────────────────────

    fn fleet() -> Vec<(String, String)> {
        vec![
            ("acme".into(), "app-1".into()),
            ("globex".into(), "app-2".into()),
        ]
    }

    /// The sort key is `(org_id, app_id, timestamp)`. A fleet query that filtered
    /// on `app_id` alone would be correct and would scan every tenant's parts on
    /// every window — the per-app query has the same guard, and it matters more
    /// here, where one statement covers the whole fleet.
    #[test]
    fn the_fleet_predicate_leads_with_org_so_it_uses_the_sort_key() {
        let sql = fleet_availability_sql(&fleet(), 60);
        assert!(
            sql.contains("(org_id, app_id) IN"),
            "must be a tuple IN on the sort key, not an app_id list: {sql}"
        );
        assert!(sql.contains("('acme','app-1')"), "{sql}");
        assert!(sql.contains("('globex','app-2')"), "{sql}");
    }

    /// The fleet roll-up and the per-app SLI must count the same surfaces, or
    /// the fleet table and the app's own Availability panel disagree about the
    /// same app on the same screen.
    #[test]
    fn fleet_and_per_app_availability_count_the_same_surfaces() {
        let per_app = availability_sql("acme", "app-1", 60);
        let fleet_sql = fleet_availability_sql(&fleet(), 60);
        assert!(per_app.contains(AVAILABILITY_KINDS), "{per_app}");
        assert!(fleet_sql.contains(AVAILABILITY_KINDS), "{fleet_sql}");
        // Outcome-driven, not status-driven, on both paths: a white-screened app
        // serves 200s, and `status >= 500` would call that available.
        assert!(
            fleet_sql.contains("countIf(outcome != 'ok')"),
            "{fleet_sql}"
        );
        assert!(!fleet_sql.contains("status >= 500"), "{fleet_sql}");
    }

    /// Grouped, or every app in the fleet collapses into one row and the table
    /// reports the fleet's aggregate as each app's own health.
    #[test]
    fn the_fleet_query_groups_by_app() {
        let sql = fleet_availability_sql(&fleet(), 60);
        assert!(sql.contains("GROUP BY app_id"), "{sql}");
        assert!(
            sql.contains("SELECT app_id"),
            "the group key must come back or rows cannot be attributed: {sql}"
        );
    }

    /// **Both** bounds shift by a cycle. With only a lower bound the baseline
    /// would be a week of traffic compared against six hours of it, and no live
    /// app would ever read as silent — the heartbeat would be permanently,
    /// invisibly off.
    #[test]
    fn the_baseline_window_is_bounded_on_both_sides() {
        let sql = fleet_baseline_sql(&fleet(), 360, 7);
        assert!(
            sql.contains("timestamp >= now() - INTERVAL 7 DAY - INTERVAL 360 MINUTE"),
            "missing the lower bound one cycle back: {sql}"
        );
        assert!(
            sql.contains("timestamp < now() - INTERVAL 7 DAY"),
            "missing the upper bound; the window would run to now: {sql}"
        );
    }

    /// An empty `IN ()` is a ClickHouse syntax error, and an empty fleet is an
    /// ordinary state — a dev box with nothing published.
    #[test]
    fn an_empty_fleet_builds_no_predicate_list() {
        // The callers short-circuit before building SQL; this pins the reason.
        assert_eq!(fleet_predicate(&[]), "(org_id, app_id) IN ()");
    }

    /// The slice-0 meters have to exist in BOTH the fresh-create DDL and the
    /// idempotent ALTER, or a deployment gets them depending on when its tables
    /// were made — and the insert names every column, so the half without them
    /// fails every write.
    #[test]
    fn the_invocation_meters_are_in_the_create_and_the_alter() {
        use super::super::schema;
        for col in ["host_calls", "init_ms"] {
            assert!(
                schema::CREATE_CUSTOM_APP_EVENTS_TABLE.contains(col),
                "{col} missing from CREATE"
            );
            assert!(
                schema::CUSTOM_APP_METER_ALTERS
                    .iter()
                    .any(|a| a.contains(col)),
                "{col} missing from the ALTERs"
            );
        }
        assert!(
            schema::CUSTOM_APP_METER_ALTERS
                .iter()
                .all(|a| a.contains("IF NOT EXISTS")),
            "the ALTERs run on every boot, so they must be idempotent"
        );
    }

    /// Single quotes are the one escape ClickHouse takes unconditionally, and a
    /// slug reaches this predicate from the database.
    #[test]
    fn fleet_predicate_escapes_quotes() {
        let apps = vec![("o'rg".to_string(), "app".to_string())];
        assert!(fleet_predicate(&apps).contains("('o''rg','app')"));
    }
}
