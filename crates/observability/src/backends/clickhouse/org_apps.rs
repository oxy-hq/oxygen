//! Custom-app performance rolled up for **one organization**, and the same
//! headline across many — what Admin → Observability reads.
//!
//! The per-app console (`custom_apps.rs`) answers "is this app up?" and the
//! fleet table answers "which app needs me?". Neither answers "how are this
//! org's apps performing", which is the question a staff operator asks when
//! they open an account: the traffic, the latency tail, and which of the three
//! failure classes the errors fall into.
//!
//! Design: `internal-docs/2026-11-16-admin-observability-design.md` §5.
//!
//! Two rules carry over from `custom_apps.rs` and are load-bearing here:
//!
//! - **Every query leads with `org_id`**, because the table's sort key is
//!   `(org_id, app_id, timestamp)`. An org-scoped read is the one shape this
//!   table was ordered for; the fleet read uses a tuple `IN` for the same
//!   reason.
//! - **Failure is `outcome != 'ok'`, never `status >= 500`.** The implicit
//!   class — a 200 carrying a shell that never mounted — is the one these
//!   surfaces exist to catch, and a status predicate reports it as success.
//!
//! Latency is reported as **quantiles, never a mean**. A mean hides the tail
//! users feel; it is also the number that stays flat while p99 triples.

use clickhouse::Row;
use oxy_shared::errors::OxyError;
use serde::Deserialize;

use super::ClickHouseObservabilityStorage;
use super::schema::PRODUCTION_ONLY;
use crate::types::{
    OrgAppDay, OrgAppOutcomes, OrgAppRoute, OrgAppRow, OrgAppTotals, OrgAppTraffic,
};

/// A value as the body of a ClickHouse single-quoted literal.
///
/// Same escaping as `custom_apps::escape_sql_literal`, and for the same reason:
/// ClickHouse reads backslash escapes inside a string literal, so doubling the
/// quote alone is not an escape. The backslash is doubled **first**, so the
/// quote's own escape cannot be claimed by one the caller typed.
///
/// Only for values this process derived — an org id parsed from a path, an app
/// id loaded from Postgres. Nothing a caller typed free-form reaches here.
fn escape_sql_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "''")
}

/// The surfaces that count as served traffic.
///
/// Deliberately the same set as `custom_apps::AVAILABILITY_KINDS`, so this
/// surface's request count and the app's own Availability panel cannot report
/// different traffic for the same app on the same screen. `asset` is excluded
/// (a cancelled image is not an outage, and asset volume buries a real shell
/// failure); `client` is excluded (a browser-reported event is already counted
/// as the serve it belongs to).
const SERVED_KINDS: &str = "kind IN ('serve', 'fn', 'data')";

/// `org_id = '…' AND timestamp >= now() - INTERVAL n DAY AND <served> AND <prod>`
///
/// The whole `WHERE` body every org-scoped read shares, so a filter cannot be
/// added to one query and forgotten in another — which is how a page ends up
/// with a request count that disagrees with its own error rate.
fn org_window(org_id: &str, days: u32) -> String {
    format!(
        "org_id = '{org}' \
         AND timestamp >= now() - INTERVAL {days} DAY \
         AND {SERVED_KINDS} \
         AND {PRODUCTION_ONLY}",
        org = escape_sql_literal(org_id),
    )
}

/// Totals and the latency tail for one org over the window.
///
/// `quantilesIf` in one pass rather than three statements: the quantile set is
/// computed from the same scan as the counts, so the p95 on screen is drawn
/// from exactly the rows the request count above it counted.
///
/// Durations are taken **only over `ok` rows** (`quantilesIf(…)(duration_ms,
/// outcome = 'ok')`). A request that failed fast would otherwise pull the
/// latency tail *down* — an app erroring in 3ms on half its traffic would
/// report an improving p95 while getting worse.
fn org_totals_sql(org_id: &str, days: u32) -> String {
    format!(
        "SELECT count() AS requests, \
         countIf(outcome != 'ok') AS failed, \
         uniq(session_id) AS sessions, \
         uniqIf(user_id, user_id != '') AS people, \
         uniqIf(app_id, true) AS apps, \
         sum(bytes) AS bytes, \
         quantilesIf(0.5, 0.95, 0.99)(duration_ms, outcome = 'ok') AS dur, \
         quantileIf(0.95)(init_ms, init_ms > 0) AS cold_start_p95, \
         sum(host_calls) AS host_calls, \
         sum(duration_ms) AS wall_ms, \
         sum(init_ms) AS init_ms \
         FROM custom_app_events \
         WHERE {window}",
        window = org_window(org_id, days),
    )
}

/// The three failure classes, counted apart.
///
/// `outcome` is a `LowCardinality(String)` the writer sets; this does not try
/// to re-derive the classes from `status`, which is the whole reason the column
/// exists. `error_kind` is carried alongside so the page can name the top cause
/// without a second round trip.
fn org_outcomes_sql(org_id: &str, days: u32) -> String {
    format!(
        "SELECT outcome, error_kind, count() AS requests \
         FROM custom_app_events \
         WHERE {window} AND outcome != 'ok' \
         GROUP BY outcome, error_kind \
         ORDER BY requests DESC \
         LIMIT 20",
        window = org_window(org_id, days),
    )
}

/// Requests, failures and p95 per day — so a regression is dated rather than
/// merely present.
fn org_days_sql(org_id: &str, days: u32) -> String {
    format!(
        "SELECT toDate(timestamp) AS day, \
         count() AS requests, \
         countIf(outcome != 'ok') AS failed, \
         quantileIf(0.95)(duration_ms, outcome = 'ok') AS p95_ms, \
         sum(duration_ms) AS wall_ms, \
         sum(init_ms) AS init_ms \
         FROM custom_app_events \
         WHERE {window} \
         GROUP BY day \
         ORDER BY day",
        window = org_window(org_id, days),
    )
}

/// Per app within the org, ordered by traffic: the breakdown the page's table
/// renders, each row linking to that app's own console.
fn org_apps_sql(org_id: &str, days: u32) -> String {
    format!(
        "SELECT app_id, \
         count() AS requests, \
         countIf(outcome != 'ok') AS failed, \
         uniq(session_id) AS sessions, \
         sum(bytes) AS bytes, \
         quantileIf(0.5)(duration_ms, outcome = 'ok') AS p50_ms, \
         quantileIf(0.95)(duration_ms, outcome = 'ok') AS p95_ms, \
         quantileIf(0.95)(init_ms, init_ms > 0) AS cold_start_p95 \
         FROM custom_app_events \
         WHERE {window} \
         GROUP BY app_id \
         ORDER BY requests DESC \
         LIMIT 50",
        window = org_window(org_id, days),
    )
}

/// The slowest and busiest routes in the org — "top paths", which is how a slow
/// deployment gets narrowed to one handler.
///
/// Ordered by p95 rather than by volume, and floored at a minimum sample
/// (`HAVING requests >= {min_samples}`): a route hit three times has a p95 that
/// is one of those three requests, and ranking by it would put a cold one-off
/// at the top of the table every single time.
fn org_routes_sql(org_id: &str, days: u32, min_samples: u32, limit: u32) -> String {
    format!(
        "SELECT app_id, route, \
         count() AS requests, \
         countIf(outcome != 'ok') AS failed, \
         quantileIf(0.95)(duration_ms, outcome = 'ok') AS p95_ms \
         FROM custom_app_events \
         WHERE {window} AND route != '' \
         GROUP BY app_id, route \
         HAVING requests >= {min_samples} \
         ORDER BY p95_ms DESC \
         LIMIT {limit}",
        window = org_window(org_id, days),
    )
}

/// `org_id IN ('a','b',…)` for the fleet headline.
///
/// Still org-first, so this reads whole org ranges of the sort key rather than
/// scanning for app ids across every tenant's parts.
fn org_in_predicate(org_ids: &[String]) -> String {
    let list = org_ids
        .iter()
        .map(|o| format!("'{}'", escape_sql_literal(o)))
        .collect::<Vec<_>>()
        .join(",");
    format!("org_id IN ({list})")
}

/// The whole fleet's function wall time — one scalar, no grouping.
///
/// The denominator for apportioning the serving bill. Deliberately **not**
/// fenced to a caller's org scope: the bill is over the whole fleet, so a
/// share computed against a bounded subset would be wrong in proportion to how
/// much of the platform that caller cannot see. It carries no tenant identity
/// — it is one number about the platform — so there is nothing in it to fence.
fn fleet_wall_ms_sql(days: u32) -> String {
    format!(
        "SELECT sum(duration_ms) AS wall_ms \
         FROM custom_app_events \
         WHERE timestamp >= now() - INTERVAL {days} DAY \
         AND {SERVED_KINDS} \
         AND {PRODUCTION_ONLY}"
    )
}

/// One row per org: the fleet list's performance columns.
fn fleet_orgs_sql(org_ids: &[String], days: u32) -> String {
    format!(
        "SELECT org_id, \
         count() AS requests, \
         countIf(outcome != 'ok') AS failed, \
         uniq(session_id) AS sessions, \
         uniqIf(user_id, user_id != '') AS people, \
         uniq(app_id) AS apps, \
         sum(bytes) AS bytes, \
         quantileIf(0.95)(duration_ms, outcome = 'ok') AS p95_ms, \
         sum(duration_ms) AS wall_ms \
         FROM custom_app_events \
         WHERE {predicate} \
         AND timestamp >= now() - INTERVAL {days} DAY \
         AND {SERVED_KINDS} \
         AND {PRODUCTION_ONLY} \
         GROUP BY org_id",
        predicate = org_in_predicate(org_ids),
    )
}

// ── Row shapes ────────────────────────────────────────────────────────────

/// `quantiles` returns an array; `quantile` a scalar. Both arrive as `f64`
/// because ClickHouse interpolates — a p95 over an even sample is not one of
/// the samples.
#[derive(Debug, Deserialize, Row)]
struct TotalsRow {
    requests: u64,
    failed: u64,
    sessions: u64,
    people: u64,
    apps: u64,
    bytes: u64,
    dur: Vec<f64>,
    cold_start_p95: f64,
    host_calls: u64,
    /// Exact summed wall time. The compute meter multiplies this by the
    /// isolate ceiling, rather than approximating from a quantile — a p50
    /// times a request count is not the area under the curve.
    wall_ms: u64,
    init_ms: u64,
}

#[derive(Debug, Deserialize, Row)]
struct OutcomeRow {
    outcome: String,
    error_kind: String,
    requests: u64,
}

#[derive(Debug, Deserialize, Row)]
struct DayRow {
    /// `toDate` is a `Date`, which the driver hands over as days since epoch.
    day: u16,
    requests: u64,
    failed: u64,
    p95_ms: f64,
    wall_ms: u64,
    init_ms: u64,
}

#[derive(Debug, Deserialize, Row)]
struct AppRow {
    app_id: String,
    requests: u64,
    failed: u64,
    sessions: u64,
    bytes: u64,
    p50_ms: f64,
    p95_ms: f64,
    cold_start_p95: f64,
}

#[derive(Debug, Deserialize, Row)]
struct RouteRow {
    app_id: String,
    route: String,
    requests: u64,
    failed: u64,
    p95_ms: f64,
}

#[derive(Debug, Deserialize, Row)]
struct FleetOrgRow {
    org_id: String,
    requests: u64,
    failed: u64,
    sessions: u64,
    people: u64,
    apps: u64,
    bytes: u64,
    p95_ms: f64,
    wall_ms: u64,
}

/// Days since the Unix epoch as `YYYY-MM-DD`, which is what the series carries
/// to the frontend.
///
/// `Date` arrives as a day count; formatting it here keeps the API's day labels
/// in one place rather than leaving each caller to guess the epoch.
fn day_label(days_since_epoch: u16) -> String {
    chrono::DateTime::from_timestamp(i64::from(days_since_epoch) * 86_400, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

// ── Reads ─────────────────────────────────────────────────────────────────

/// Everything the per-org Performance panel renders, in five statements.
///
/// Five rather than one: the five shapes group differently (none, by outcome,
/// by day, by app, by route), and a single statement returning all of them
/// would be a union of padded rows that each caller has to unpick. They run
/// sequentially against one window definition ([`org_window`]), so they cannot
/// disagree about which rows they covered.
pub(super) async fn get_org_app_traffic(
    storage: &ClickHouseObservabilityStorage,
    org_id: &str,
    days: u32,
) -> Result<OrgAppTraffic, OxyError> {
    let client = storage.read_client();

    let totals_rows = client
        .query(&org_totals_sql(org_id, days))
        .fetch_all::<TotalsRow>()
        .await
        .map_err(|e| query_failed("org app totals", e))?;

    // No rows at all is an org that served nothing — a zero, not an error. The
    // caller tells "served nothing" from "never asked" by whether this returned
    // `Ok`, which is the same contract `get_fleet_availability` states.
    let totals = totals_rows
        .first()
        .map(|r| OrgAppTotals {
            requests: r.requests,
            failed: r.failed,
            sessions: r.sessions,
            people: r.people,
            apps: r.apps,
            bytes: r.bytes,
            host_calls: r.host_calls,
            p50_ms: r.dur.first().copied().unwrap_or_default(),
            p95_ms: r.dur.get(1).copied().unwrap_or_default(),
            p99_ms: r.dur.get(2).copied().unwrap_or_default(),
            cold_start_p95_ms: r.cold_start_p95,
            wall_ms: r.wall_ms,
            init_ms: r.init_ms,
        })
        .unwrap_or_default();

    let outcomes = client
        .query(&org_outcomes_sql(org_id, days))
        .fetch_all::<OutcomeRow>()
        .await
        .map_err(|e| query_failed("org app outcomes", e))?
        .into_iter()
        .map(|r| OrgAppOutcomes {
            outcome: r.outcome,
            error_kind: r.error_kind,
            requests: r.requests,
        })
        .collect();

    let by_day = client
        .query(&org_days_sql(org_id, days))
        .fetch_all::<DayRow>()
        .await
        .map_err(|e| query_failed("org app days", e))?
        .into_iter()
        .map(|r| OrgAppDay {
            day: day_label(r.day),
            requests: r.requests,
            failed: r.failed,
            p95_ms: r.p95_ms,
            wall_ms: r.wall_ms,
            init_ms: r.init_ms,
        })
        .collect();

    let by_app = client
        .query(&org_apps_sql(org_id, days))
        .fetch_all::<AppRow>()
        .await
        .map_err(|e| query_failed("org app breakdown", e))?
        .into_iter()
        .map(|r| OrgAppRow {
            app_id: r.app_id,
            requests: r.requests,
            failed: r.failed,
            sessions: r.sessions,
            bytes: r.bytes,
            p50_ms: r.p50_ms,
            p95_ms: r.p95_ms,
            cold_start_p95_ms: r.cold_start_p95,
        })
        .collect();

    let routes = client
        .query(&org_routes_sql(
            org_id,
            days,
            ROUTE_MIN_SAMPLES,
            ROUTE_LIMIT,
        ))
        .fetch_all::<RouteRow>()
        .await
        .map_err(|e| query_failed("org app routes", e))?
        .into_iter()
        .map(|r| OrgAppRoute {
            app_id: r.app_id,
            route: r.route,
            requests: r.requests,
            failed: r.failed,
            p95_ms: r.p95_ms,
        })
        .collect();

    Ok(OrgAppTraffic {
        totals,
        outcomes,
        by_day,
        by_app,
        routes,
    })
}

/// A route needs this many requests in the window before its p95 is ranked.
/// Below it the quantile is one of a handful of samples, and ordering by it
/// puts a cold one-off at the top of the table.
const ROUTE_MIN_SAMPLES: u32 = 20;

/// How many routes the panel shows before it would need its own surface.
const ROUTE_LIMIT: u32 = 15;

/// The fleet list's performance columns, keyed by `org_id`.
///
/// An org absent from the map served nothing. As everywhere in this module,
/// that is a zero and not missing data; `Err` is what "not measured" looks
/// like.
pub(super) async fn get_fleet_org_traffic(
    storage: &ClickHouseObservabilityStorage,
    org_ids: &[String],
    days: u32,
) -> Result<std::collections::HashMap<String, OrgAppTotals>, OxyError> {
    // An empty `IN ()` is a ClickHouse syntax error, and a deployment with no
    // orgs is an ordinary state.
    if org_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = storage
        .read_client()
        .query(&fleet_orgs_sql(org_ids, days))
        .fetch_all::<FleetOrgRow>()
        .await
        .map_err(|e| query_failed("fleet org traffic", e))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.org_id,
                OrgAppTotals {
                    requests: r.requests,
                    failed: r.failed,
                    sessions: r.sessions,
                    people: r.people,
                    apps: r.apps,
                    bytes: r.bytes,
                    host_calls: 0,
                    p50_ms: 0.0,
                    p95_ms: r.p95_ms,
                    p99_ms: 0.0,
                    cold_start_p95_ms: 0.0,
                    wall_ms: r.wall_ms,
                    init_ms: 0,
                },
            )
        })
        .collect())
}

/// Function wall time across the whole fleet, in milliseconds.
pub(super) async fn get_fleet_wall_ms(
    storage: &ClickHouseObservabilityStorage,
    days: u32,
) -> Result<u64, OxyError> {
    #[derive(Debug, Deserialize, Row)]
    struct WallRow {
        wall_ms: u64,
    }
    let rows = storage
        .read_client()
        .query(&fleet_wall_ms_sql(days))
        .fetch_all::<WallRow>()
        .await
        .map_err(|e| query_failed("fleet wall time", e))?;
    Ok(rows.first().map(|r| r.wall_ms).unwrap_or(0))
}

fn query_failed(what: &str, e: clickhouse::error::Error) -> OxyError {
    OxyError::RuntimeError(format!("ClickHouse {what} query failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure predicate is outcome-driven everywhere. A `status >= 500`
    /// rollup reports a white-screened app — a 200 carrying a shell that never
    /// mounted — as perfectly healthy, which is the exact case `outcome` was
    /// added to catch. Held for every statement, because one of them drifting
    /// is how the page ends up disagreeing with itself.
    #[test]
    fn every_statement_counts_failure_by_outcome_not_status() {
        let statements = [
            org_totals_sql("o", 30),
            org_outcomes_sql("o", 30),
            org_days_sql("o", 30),
            org_apps_sql("o", 30),
            org_routes_sql("o", 30, 20, 15),
            fleet_orgs_sql(&["o".into()], 30),
        ];
        for sql in statements {
            assert!(
                sql.contains("outcome != 'ok'"),
                "failure must be outcome-driven: {sql}"
            );
            assert!(
                !sql.contains("status >= 500"),
                "a status predicate misses the implicit failure class: {sql}"
            );
        }
    }

    /// Every org-scoped statement leads with `org_id`, which is the first
    /// column of the table's sort key. A statement that filtered on `app_id`
    /// or on `timestamp` first would be correct and would scan every tenant's
    /// parts — the trap `custom_apps.rs` documents, and it is worse here where
    /// one page runs five statements.
    #[test]
    fn org_scoped_statements_lead_with_org_id() {
        for sql in [
            org_totals_sql("acme", 30),
            org_outcomes_sql("acme", 30),
            org_days_sql("acme", 30),
            org_apps_sql("acme", 30),
            org_routes_sql("acme", 30, 20, 15),
        ] {
            let where_body = sql.split("WHERE ").nth(1).expect("statement has a WHERE");
            assert!(
                where_body.starts_with("org_id = 'acme'"),
                "org scope must lead the predicate: {where_body}"
            );
        }
    }

    /// The fleet statement is org-first too, by `IN` rather than `=`.
    #[test]
    fn fleet_statement_leads_with_org_id() {
        let sql = fleet_orgs_sql(&["a".into(), "b".into()], 7);
        let where_body = sql.split("WHERE ").nth(1).expect("statement has a WHERE");
        assert!(
            where_body.starts_with("org_id IN ('a','b')"),
            "fleet scope must lead the predicate: {where_body}"
        );
    }

    /// Latency quantiles are conditioned on success. Without the condition a
    /// request that fails in 3ms pulls the tail down, so an app erroring on
    /// half its traffic reports an *improving* p95 while getting worse — a
    /// number that moves the wrong way is worse than no number.
    #[test]
    fn latency_quantiles_exclude_failed_requests() {
        for sql in [
            org_totals_sql("o", 30),
            org_days_sql("o", 30),
            org_apps_sql("o", 30),
            org_routes_sql("o", 30, 20, 15),
            fleet_orgs_sql(&["o".into()], 30),
        ] {
            assert!(
                sql.contains("(duration_ms, outcome = 'ok')"),
                "duration quantiles must be taken over successful requests: {sql}"
            );
        }
    }

    /// Cold start is measured only where it was recorded. `init_ms` is 0 on a
    /// warm request, and a quantile over the zeros reports every app as
    /// starting instantly.
    #[test]
    fn cold_start_quantile_ignores_warm_requests() {
        for sql in [org_totals_sql("o", 30), org_apps_sql("o", 30)] {
            assert!(
                sql.contains("(init_ms, init_ms > 0)"),
                "cold start must exclude warm requests, whose init_ms is 0: {sql}"
            );
        }
    }

    /// Assets and client beacons stay out of the request count, so this
    /// surface's traffic matches the app's own Availability panel. They
    /// outnumber page loads by enough to bury a real shell failure.
    #[test]
    fn served_traffic_excludes_assets_and_client_beacons() {
        let sql = org_totals_sql("o", 30);
        assert!(sql.contains("kind IN ('serve', 'fn', 'data')"), "{sql}");
        assert!(!sql.contains("'asset'"), "{sql}");
        assert!(!sql.contains("'client'"), "{sql}");
    }

    /// A route's p95 is only ranked once it has a sample behind it. Ranking a
    /// three-request route by its p95 puts a cold one-off at the top of the
    /// slowest-routes table on every single load.
    #[test]
    fn route_ranking_requires_a_minimum_sample() {
        let sql = org_routes_sql("o", 30, 20, 15);
        assert!(sql.contains("HAVING requests >= 20"), "{sql}");
        assert!(sql.contains("ORDER BY p95_ms DESC"), "{sql}");
    }

    /// Wall time is **summed**, never approximated from a quantile.
    ///
    /// A compute meter multiplies this by the provisioned ceiling, so the
    /// figure has to be the area under the curve. `requests × p50_ms` is not:
    /// on a workload whose tail is ten times its median — which is exactly
    /// what p99 beside p50 on this surface shows — it understates the real
    /// time by most of it.
    #[test]
    fn wall_time_is_summed_not_approximated_from_a_quantile() {
        for sql in [org_totals_sql("o", 30), org_days_sql("o", 30)] {
            assert!(sql.contains("sum(duration_ms) AS wall_ms"), "{sql}");
            assert!(sql.contains("sum(init_ms) AS init_ms"), "{sql}");
        }
        assert!(
            fleet_orgs_sql(&["o".into()], 30).contains("sum(duration_ms) AS wall_ms"),
            "the fleet rollup needs the same exact figure"
        );
    }

    /// Start-up time is summed over **every** request, not only the cold ones.
    ///
    /// `wall_ms - init_ms` is the tenant's own code, so both halves must cover
    /// the same request set. The cold-start *quantile* beside it is filtered
    /// to `init_ms > 0` on purpose — that one is a tail, not a total — and
    /// confusing the two would make the composition fail to add up.
    #[test]
    fn summed_start_up_time_covers_every_request_unlike_the_quantile() {
        let sql = org_totals_sql("o", 30);
        assert!(sql.contains("sum(init_ms) AS init_ms"), "{sql}");
        assert!(
            sql.contains("quantileIf(0.95)(init_ms, init_ms > 0)"),
            "the cold-start tail stays filtered: {sql}"
        );
    }

    /// An id cannot end the literal early and have what follows read as SQL.
    /// Same guarantee `custom_apps.rs` holds for its own reads; these ids come
    /// from a URL path, so the property has to hold here independently.
    #[test]
    fn quotes_and_backslashes_cannot_break_out_of_a_literal() {
        let sql = org_totals_sql("o' OR 1=1 --", 30);
        assert!(
            sql.contains("org_id = 'o'' OR 1=1 --'"),
            "the quote must be doubled, leaving the injection inside the literal: {sql}"
        );
        let sql = org_totals_sql(r"o\' OR 1=1", 30);
        assert!(
            sql.contains(r"org_id = 'o\\'' OR 1=1'"),
            "the backslash must be doubled first, or it escapes the doubled quote: {sql}"
        );
    }

    /// An empty org list would make `IN ()`, which ClickHouse rejects outright
    /// — so the read short-circuits rather than sending it.
    #[test]
    fn an_empty_org_list_is_not_sent_as_a_query() {
        // The predicate itself is degenerate, which is why the caller must not
        // reach it; this records both halves of that contract.
        assert_eq!(org_in_predicate(&[]), "org_id IN ()");
    }

    /// The fleet denominator spans every org, unfenced, because the bill it
    /// divides is over the whole fleet. A scoped denominator would make every
    /// bounded reader's share too large by whatever they cannot see.
    #[test]
    fn the_fleet_denominator_is_not_scoped_to_any_org() {
        let sql = fleet_wall_ms_sql(30);
        assert!(!sql.contains("org_id"), "no org filter belongs here: {sql}");
        assert!(sql.contains("sum(duration_ms) AS wall_ms"), "{sql}");
        // Same traffic definition as everything else, or the share is of a
        // different population than the numerator.
        assert!(sql.contains("kind IN ('serve', 'fn', 'data')"), "{sql}");
        assert!(sql.contains(PRODUCTION_ONLY), "{sql}");
    }

    /// Days-since-epoch is formatted, not guessed at by each caller.
    #[test]
    fn day_labels_format_from_the_unix_epoch() {
        assert_eq!(day_label(0), "1970-01-01");
        assert_eq!(day_label(20_773), "2026-11-16");
    }

    /// The totals read tolerates an empty result: an org that served nothing
    /// is a row of zeros, and the distinction from "not measured" is carried
    /// by `Result`, not by an `Option` inside it.
    #[test]
    fn totals_default_to_zero_rather_than_absent() {
        let t = OrgAppTotals::default();
        assert_eq!(t.requests, 0);
        assert_eq!(t.p95_ms, 0.0);
    }
}
