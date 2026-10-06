//! The paged read: event ranking, the counts behind `total`, and the
//! per-event bucket cap.
//!
//! Paging is by **event**, not by row — a multi-bucket event is one row in the
//! UI, and a row-limited page would strand an event's later buckets off it.
//! Everything in here follows from that: the two-phase ranking, `total` in
//! event units, and a cap that trims within an event rather than across a page.

use agentic_http::AgenticState;
use axum::extract::{Extension, Json, Query};
use axum::http::StatusCode;
use entity::metric_anomalies::{self, Entity as AnomaliesEntity};
use entity::metric_monitor_coverage::{self, Entity as CoverageEntity};
use oxy_metric_monitoring as monitoring;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use super::cap::cap_buckets_per_event;
use super::error::AnomalyError;
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    /// Filter by status. `None` returns every status.
    pub status: Option<String>,
    /// Max **events** (not rows) for the default severity ranking; max **rows**
    /// for `order=recent`. Defaults to 100. Under the default ranking every
    /// bucket of a returned event comes back, so the row count is
    /// `limit × buckets-per-event`, bounded defensively in [`load_ranked_events`].
    pub limit: Option<u64>,
    /// How many **events** to skip before the page (rows, for `order=recent`).
    /// Defaults to 0. Same unit as `limit` on both paths, so page `n` is
    /// `offset = (n - 1) × limit`.
    pub offset: Option<u64>,
    /// `recent` orders by `detected_at DESC, id DESC` (row-limited; the `id`
    /// tiebreak is what makes paging stable, at the cost of a sort node — see
    /// [`list_recent`]) for consumers that want the latest firings — e.g. the
    /// Monitors tab's "last anomaly" column. Any other value (default) ranks
    /// worst-first by event severity for the Insights Inbox.
    pub order: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub anomalies: Vec<metric_anomalies::Model>,
    /// Total matching the filter across every page — **events** under the
    /// default ranking, rows under `order=recent`, so it is the same unit as
    /// `limit`/`offset` and a client can divide for a page count. Counted
    /// independently of the page — never truncated to it — though a short first
    /// page is its own answer and skips the count query.
    ///
    /// **Absent** for a caller that sent neither `limit` nor `offset`: that
    /// caller asked for "the top N", not for page 1 of N, and there is no total
    /// behind it. Omitted rather than filled with the page's own size — a field
    /// named `total` quietly holding a page length is the kind of wrong answer
    /// that reads as right. Pass a `limit` to get one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The page actually served — `limit` is clamped to 1..=500, so a pager
    /// that assumed its request was honoured would mis-compute page numbers.
    /// (`offset` is echoed for symmetry; too deep an offset is refused rather
    /// than adjusted.)
    pub limit: u64,
    pub offset: u64,
    /// The deepest `offset` this endpoint will serve — past it a request is
    /// refused with a 400.
    ///
    /// Echoed so a pager can stop offering pages that don't exist. The
    /// alternative is the client hardcoding the same number, which drifts
    /// silently the moment the server's changes: the last-page link keeps
    /// rendering and its click becomes a hard error.
    pub max_offset: u64,
    /// Event keys whose buckets were trimmed to [`super::cap::MAX_BUCKETS_PER_EVENT`], in
    /// the client's key space (`event_id`, or `ungrouped:<row id>`).
    ///
    /// Without this a client cannot tell a 50-bucket event from a 200-bucket
    /// one — the row would either state a trimmed count as fact or guess at a
    /// "+" from the count alone, which is wrong for an event that genuinely has
    /// exactly the cap, and wrong again in a filtered tab where the cap applies
    /// after the status filter.
    ///
    /// **Only meaningful under the default ranking.** That path pages *events*
    /// and returns each one whole (modulo this cap), so an absence here means
    /// complete. `order=recent` pages rows, which is exactly what lets an event
    /// straddle the page boundary — a client grouping those results by
    /// `event_id` must treat every event as possibly partial, and this list
    /// stays empty because there is nothing it could honestly report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub truncated_events: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ListMonitorsResponse {
    pub monitors: Vec<monitoring::config::MonitorEntry>,
    /// One row per **scanned segment** (a `group_by` monitor fans out to many),
    /// recording how much history it has against how much it needs.
    ///
    /// Without this the tab cannot distinguish a healthy monitor that found
    /// nothing from one that is not being scored at all — both show an empty
    /// inbox. Empty until the first scan after this shipped.
    #[serde(default)]
    pub coverage: Vec<entity::metric_monitor_coverage::Model>,
    /// The file's `notify:` block, so the tab can say whether new insights go
    /// anywhere but this inbox. Absent when the file has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify: Option<monitoring::NotifyConfig>,
}

/// Per-segment scan coverage for a workspace.
///
/// A persisted-data read, so it stays `FleetOk` — unlike the `.monitor.yml`
/// fallback in the caller, which is why the compiled fast path exists at all.
///
/// Never fails the request: coverage is advisory, and losing the monitor list
/// over a status column would be a worse outcome than the ambiguity this column
/// exists to remove.
async fn load_coverage(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
) -> Vec<entity::metric_monitor_coverage::Model> {
    CoverageEntity::find()
        .filter(metric_monitor_coverage::Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(metric_monitor_coverage::Column::Measure)
        .all(db)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                workspace_id = %workspace_id,
                error = %e,
                "list_monitors: coverage lookup failed; returning monitors without it"
            );
            Vec::new()
        })
}

/// `GET /workspaces/{workspace_id}/semantic/monitors` — list every entry in
/// `.monitor.yml`, plus per-segment scan coverage. Returns an empty list when
/// the file is missing or empty. Returns 400 when it exists but fails to parse.
pub async fn list_monitors(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Extension(state): Extension<Arc<AgenticState>>,
) -> Result<Json<ListMonitorsResponse>, (StatusCode, String)> {
    let workspace_id = workspace_manager.workspace_id;
    let coverage = load_coverage(&state.db, workspace_id).await;

    // `monitor_config()` IS both arms. It reads the compiled row, and on a miss
    // reads `<root>/.monitor.yml` itself (`ConfigManager::root_singleton`) —
    // the same path `monitoring::config::default_config_path` returns. The
    // filesystem fallback that used to live here read the file the manager had
    // just read, and needed a working copy in the signature to do it.
    let definition = match workspace_manager.config_manager.monitor_config().await {
        Ok(Some(definition)) => definition,
        // No monitor file, from either source. The documented answer.
        Ok(None) => {
            return Ok(Json(ListMonitorsResponse {
                monitors: vec![],
                coverage,
                notify: None,
            }));
        }
        // Retryable means no source could be read — not compiled yet, or no
        // working copy on this node. Anything else is the file itself failing
        // to parse, which is the customer's YAML and a 400. This used to be an
        // unconditional 503, so a broken `.monitor.yml` was reported as a
        // platform fault.
        Err(e) => {
            let status = if e.retryable() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            };
            tracing::warn!(
                workspace_id = %workspace_id,
                error = ?e,
                retryable = e.retryable(),
                "list_monitors: could not read the monitor config"
            );
            return Err((status, e.to_string()));
        }
    };

    let cfg = serde_json::from_value::<monitoring::config::MonitorConfig>(definition)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(ListMonitorsResponse {
        monitors: cfg.monitors,
        coverage,
        notify: cfg.notify,
    }))
}

/// `GET /workspaces/{workspace_id}/semantic/anomalies?status=new&limit=100&offset=0`
/// (`limit`/`offset` count **events**, not rows; `total` in the response counts
/// them too, so a client can page without the row grouping skewing its maths).
///
/// Two-phase so the inbox ranks on severity without truncating an event
/// mid-way:
///  1. `rank_event_keys` ranks *events* worst-first and takes the `limit`
///     starting at `offset`.
///  2. `load_ranked_events` fetches every row of exactly those events (modulo
///     the status filter — a filtered tab still returns only its status).
///
/// Ranking per event (not per row) is what keeps the per-event Ack loop and the
/// `worst of N` bucket count honest *within the selected tab*: a waived-band
/// continuation bucket is filed `low`, so a row-limited severity sort would
/// strand those buckets off the page.
pub async fn list_anomalies(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Extension(state): Extension<Arc<AgenticState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResponse>, AnomalyError> {
    let workspace_id = workspace_manager.workspace_id;
    // A caller that named neither `limit` nor `offset` is not paging — it wants
    // "the latest N" or "the worst N" — so it gets no count query. The Monitors
    // tab is exactly this caller, and it discards `total`; counting for it
    // would put an extra scan on every post-scan poll and every status update.
    let paging = q.limit.is_some() || q.offset.is_some();
    // Floored as well as capped. `?limit=0` is `LIMIT 0`: an empty page that
    // still pays for the count, and an `offset += limit` loop that never
    // advances and never ends.
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // Refused, not clamped. A large offset makes Postgres build the whole
    // `GROUP BY` ranking only to discard it — unbounded work on an
    // authenticated endpoint — and one past `i64::MAX` wraps negative into a
    // 500 for what is really a malformed request. Clamping instead would serve
    // the same page under every deeper offset, so the `offset += limit` loop
    // the SDK documents would never terminate; an error ends it.
    let offset = q.offset.unwrap_or(0);
    if offset > MAX_OFFSET {
        return Err(AnomalyError::OffsetTooDeep(offset));
    }
    let status = q.status.as_deref();

    // Recency path: a plain `detected_at DESC` row-limited scan for consumers
    // that want the latest firings rather than the worst. Kept separate so the
    // Insights Inbox's severity ranking can't regress this (a measure whose
    // recent anomalies are all `low` would fall off a severity-ranked page).
    if q.order.as_deref() == Some("recent") {
        let anomalies = list_recent(&state.db, workspace_id, status, limit, offset).await?;
        let total = match (paging, short_circuit_total(anomalies.len(), limit, offset)) {
            (false, _) => None,
            (true, Some(total)) => Some(total),
            (true, None) => soften(count_rows(&state.db, workspace_id, status).await),
        };
        return Ok(Json(ListResponse {
            anomalies,
            total,
            limit,
            offset,
            max_offset: MAX_OFFSET,
            // Nothing to report: this path never applies the per-event cap.
            // It is row-limited, which means an event can straddle the page
            // boundary instead — see the field's own note.
            truncated_events: Vec::new(),
        }));
    }

    let keys = rank_event_keys(&state.db, workspace_id, status, limit, offset).await?;
    let total = match (paging, short_circuit_total(keys.len(), limit, offset)) {
        (false, _) => None,
        (true, Some(total)) => Some(total),
        (true, None) => soften(count_events(&state.db, workspace_id, status).await),
    };
    if keys.is_empty() {
        return Ok(Json(ListResponse {
            anomalies: vec![],
            total,
            limit,
            offset,
            max_offset: MAX_OFFSET,
            truncated_events: Vec::new(),
        }));
    }
    let (anomalies, truncated_events) =
        load_ranked_events(&state.db, workspace_id, status, &keys).await?;
    Ok(Json(ListResponse {
        anomalies,
        total,
        limit,
        offset,
        max_offset: MAX_OFFSET,
        truncated_events,
    }))
}

/// A failed count is a missing `total`, not a failed request.
///
/// The page rows are already in hand by the time this runs, and `total` is only
/// the pager's denominator. Propagating the error would blank an inbox that has
/// its data — and the client does not retry — over a number it can do without.
fn soften(counted: Result<u64, AnomalyError>) -> Option<u64> {
    match counted {
        Ok(total) => Some(total),
        Err(e) => {
            tracing::warn!(error = ?e, "list_anomalies: total count failed; serving the page without it");
            None
        }
    }
}

/// Deepest page the endpoint will serve — 2000 pages at the inbox's 25, far
/// past where anyone pages by hand. Deep offsets are the expensive shape (the
/// ranking is built and thrown away), so this is a ceiling on wasted work, not
/// a product limit. Past it the request is refused rather than clamped: a
/// clamped page repeats forever under rising offsets, which is a paging loop
/// that never ends.
pub(super) const MAX_OFFSET: u64 = 50_000;

/// The total, when the page itself already proves it: a first page that came
/// back short is the whole result set, so `total` is what we just counted in
/// memory. `None` means it has to be queried.
///
/// (Only consulted for a caller that *is* paging — one that sent no `limit` and
/// no `offset` gets no `total` at all, rather than a page length wearing the
/// name of a total.)
///
/// Worth being precise about the reach, because it shrank when the inbox chose
/// a 25-event page: this spares the `COUNT` only for a workspace whose *whole
/// filtered list* fits in one page. Past that — 25 open anomalies — every list
/// request pays it, which is why [`count_events`] is written to stay cheap
/// rather than to be avoided.
fn short_circuit_total(page_len: usize, limit: u64, offset: u64) -> Option<u64> {
    let page_len = page_len as u64;
    (offset == 0 && page_len < limit).then_some(page_len)
}

/// Total anomaly **rows** matching the filter — the page-count denominator for
/// the `order=recent` path, which is row-limited.
async fn count_rows(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    status: Option<&str>,
) -> Result<u64, AnomalyError> {
    let mut query =
        AnomaliesEntity::find().filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id));
    if let Some(status) = status {
        query = query.filter(metric_anomalies::Column::Status.eq(status));
    }
    Ok(query.count(db).await?)
}

/// Total anomaly **events** matching the filter — the page-count denominator
/// for the default ranking, whose `limit`/`offset` count events too. Counting
/// rows here would overstate the page count for any multi-bucket event.
///
/// Deliberately does not reuse the ranking query: the count needs no ordering
/// and no severity `CASE`, just the distinct event keys.
///
/// Cost: `idx_metric_anomalies_ws_status_event_key` indexes exactly this shape
/// — `(workspace_id, status, (COALESCE(event_id, id)))` — so the count runs
/// index-only rather than reading every matching row to aggregate. That index
/// exists for this query and nothing else; it was added when the inbox started
/// paging, because `total` runs on every list request past one page, including
/// the Semantic Model badge, which mounts whether or not anyone opens the
/// inbox. Keep the expression here and the one in the index identical, or the
/// planner quietly falls back to the heap scan.
async fn count_events(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    status: Option<&str>,
) -> Result<u64, AnomalyError> {
    use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};
    let mut params: Vec<Value> = vec![workspace_id.into()];
    let status_clause = match status {
        Some(s) => {
            params.push(s.to_string().into());
            "AND status = $2 "
        }
        None => "",
    };
    // `COALESCE(event_id, id)` over uuids, not the `::text` the ranking query
    // needs: nothing here compares against string keys, and distinct-ing uuids
    // skips two casts per row.
    let sql = format!(
        "SELECT COUNT(DISTINCT COALESCE(event_id, id)) AS total \
         FROM metric_anomalies \
         WHERE workspace_id = $1 {status_clause}"
    );
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            params,
        ))
        .await?
        .ok_or_else(|| AnomalyError::Internal("count query returned no row".into()))?;
    // `COUNT` is `bigint`; it is never negative, but the cast is explicit so a
    // driver quirk can't wrap into a huge page count.
    let total: i64 = row.try_get("", "total").map_err(AnomalyError::Db)?;
    Ok(Ord::max(total, 0) as u64)
}

/// Latest firings first — a `detected_at DESC, id DESC` scan.
///
/// The `id` tiebreak is what makes the order total, and paging needs that: a
/// scan stamps a whole batch with one `detected_at`, and an ordering with ties
/// lets a row show up on two pages or on none. It does cost the index-served
/// sort this path used to get — `idx_metric_anomalies_workspace_status_detected`
/// is `(workspace_id, status, detected_at)`, so it still serves the `WHERE` and
/// the leading sort key but Postgres now adds an (incremental) sort for `id`.
/// Correct paging is worth that; the alternative is duplicate and missing rows.
///
/// The Monitors-tab caller passes no status, so there Postgres scans the
/// workspace's anomaly rows and sorts before the `LIMIT` (the same shape the
/// endpoint had pre-PR — a `(workspace_id, detected_at, id)` index would bound
/// both paths if this ever gets hot).
async fn list_recent(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    status: Option<&str>,
    limit: u64,
    offset: u64,
) -> Result<Vec<metric_anomalies::Model>, AnomalyError> {
    let mut query = AnomaliesEntity::find()
        .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
        // `detected_at` alone is not unique — repeat scans stamp a batch
        // identically — and a non-deterministic tiebreak lets a row appear on
        // two pages (or on none). `id` makes the paged order total.
        .order_by_desc(metric_anomalies::Column::DetectedAt)
        .order_by_desc(metric_anomalies::Column::Id)
        .limit(limit)
        .offset(offset);
    if let Some(status) = status {
        query = query.filter(metric_anomalies::Column::Status.eq(status));
    }
    Ok(query.all(db).await?)
}

/// The event key for a row — its `event_id`, or its own id for pre-event rows.
///
/// Internal to the ranking: it exists to group rows against `rank_event_keys`'
/// `COALESCE(event_id::text, id::text)`, and shares that key space. It is NOT
/// the frontend's key, which prefixes pre-event rows with `ungrouped:` — see
/// `cap::client_event_key` for what crosses the wire.
pub(super) fn event_key_of(m: &metric_anomalies::Model) -> String {
    m.event_id
        .map(|e| e.to_string())
        .unwrap_or_else(|| m.id.to_string())
}

/// Rank the workspace's anomaly **events** worst-first and return the top
/// `limit` event keys.
///
/// Grouped, not windowed. `GROUP BY` + `ORDER BY MAX(...)` can't push the
/// `LIMIT` into the scan, so this still reads every row matching the `WHERE`
/// — but at a much smaller constant than the window version it replaced (narrow
/// projection, one sort of *groups* instead of a `WindowAgg` plus a full-row
/// sort). The status predicate is appended only when set, so the default
/// **New** tab keeps the `status` column of
/// `idx_metric_anomalies_workspace_status_detected` sargable rather than folding
/// it into a non-sargable `OR`. The `LIMIT` bounds *events*, and phase 2 fetches
/// each returned event whole — the property the row-limited window ordering
/// could not give. Active events sort ahead of fully-dismissed ones so the
/// `all` tab doesn't spend its page budget on dismissed rows.
async fn rank_event_keys(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    status: Option<&str>,
    limit: u64,
    offset: u64,
) -> Result<Vec<String>, AnomalyError> {
    use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};
    let mut params: Vec<Value> = vec![workspace_id.into()];
    let status_clause = match status {
        Some(s) => {
            params.push(s.to_string().into());
            "AND status = $2 "
        }
        None => "",
    };
    params.push((limit as i64).into());
    let limit_param = params.len(); // $2 when unfiltered, $3 when filtered
    params.push((offset as i64).into());
    let offset_param = params.len();
    // The trailing `event_key` tiebreak is what makes paging safe: the three
    // ranking keys ahead of it are all ties-prone (severity buckets, a shared
    // scan timestamp), and without a total order Postgres may return an event
    // on two pages or on none.
    let sql = format!(
        "SELECT COALESCE(event_id::text, id::text) AS event_key \
         FROM metric_anomalies \
         WHERE workspace_id = $1 {status_clause}\
         GROUP BY 1 \
         ORDER BY \
           MAX(CASE WHEN status <> 'dismissed' THEN 1 ELSE 0 END) DESC, \
           MAX({rank}) DESC, \
           MAX(detected_at) DESC, \
           event_key \
         LIMIT ${limit_param} OFFSET ${offset_param}",
        rank = monitoring::detect::severity_rank_case_sql(),
    );
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            params,
        ))
        .await?;
    rows.into_iter()
        .map(|r| r.try_get::<String>("", "event_key"))
        .collect::<Result<Vec<_>, _>>()
        .map_err(AnomalyError::Db)
}

/// Fetch every row of the given events, ordered to match the phase-1 event rank
/// (then `period_start` within an event). Keys are UUID strings: a key is an
/// `event_id` for grouped rows or a row `id` for pre-event ones, so match on
/// either — `event_id IN keys` never matches a `NULL`, and `id IN keys` picks up
/// the ungrouped rows.
async fn load_ranked_events(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    status: Option<&str>,
    keys: &[String],
) -> Result<(Vec<metric_anomalies::Model>, Vec<String>), AnomalyError> {
    let uuids: Vec<Uuid> = keys
        .iter()
        .filter_map(|k| Uuid::parse_str(k).ok())
        .collect();
    let mut query = AnomaliesEntity::find()
        .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
        .filter(
            sea_orm::Condition::any()
                .add(metric_anomalies::Column::EventId.is_in(uuids.clone()))
                .add(metric_anomalies::Column::Id.is_in(uuids)),
        );
    if let Some(status) = status {
        query = query.filter(metric_anomalies::Column::Status.eq(status));
    }
    let mut rows = query.all(db).await?;

    // Order by phase-1 event rank, then oldest-first within an event. The rank
    // is authoritative; anything not in it (a scan landing between phase 1 and
    // phase 2 can add a row to a ranked event, or make one return no rows —
    // self-correcting on the next load) sorts last rather than panicking.
    // `sort_by_cached_key` computes the key once per row, not once per compare,
    // so `event_key_of`'s allocation happens n times, not n·log n.
    let rank: std::collections::HashMap<&str, usize> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();
    rows.sort_by_cached_key(|m| {
        let r = rank
            .get(event_key_of(m).as_str())
            .copied()
            .unwrap_or(usize::MAX);
        (r, m.period_start)
    });

    let truncated = cap_buckets_per_event(&mut rows);
    Ok((rows, truncated))
}

#[cfg(test)]
mod tests {
    use super::short_circuit_total;

    #[test]
    fn short_circuits_a_short_first_page() {
        // 3 of a possible 25 on page 1 — there is no page 2 to count.
        assert_eq!(short_circuit_total(3, 25, 0), Some(3));
        assert_eq!(short_circuit_total(0, 25, 0), Some(0));
    }

    #[test]
    fn queries_the_total_when_the_page_could_be_hiding_more() {
        // A full page says nothing about what follows it.
        assert_eq!(short_circuit_total(25, 25, 0), None);
        // Any page but the first has rows behind it by definition.
        assert_eq!(short_circuit_total(3, 25, 25), None);
    }
}
