//! Trace queries against ClickHouse observability tables.

use clickhouse::Row;
use oxy_shared::errors::OxyError;
use serde::Deserialize;

use super::ClickHouseObservabilityStorage;
use crate::scope::WorkspaceScope;
use crate::types::{
    ClusterInfoRow, ClusterMapDataRow, SpanRecord, TraceDetailRow, TraceEnrichmentRow, TraceRow,
};

#[derive(Debug, Deserialize, Row)]
struct TraceQueryRow {
    trace_id: String,
    span_id: String,
    timestamp: String,
    span_name: String,
    service_name: String,
    duration_ns: i64,
    status_code: String,
    status_message: String,
    span_attributes: String,
    event_data: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
}

#[derive(Debug, Deserialize, Row)]
struct TraceDetailQueryRow {
    timestamp: String,
    trace_id: String,
    span_id: String,
    parent_span_id: String,
    span_name: String,
    service_name: String,
    span_attributes: String,
    duration_ns: i64,
    status_code: String,
    status_message: String,
    event_data: String,
}

#[derive(Debug, Deserialize, Row)]
struct ClusterMapQueryRow {
    trace_id: String,
    question: String,
    embedding: Vec<f32>,
    cluster_id: i32,
    intent_name: String,
    confidence: f32,
    // Deliberately NOT named `classified_at`: ClickHouse resolves a `WHERE
    // classified_at …` reference against a SELECT-list alias of the same name
    // instead of the source column, even though the alias's expression
    // (`formatDateTime(...)`, a String) shares no type with the DateTime the
    // WHERE clause compares it against — `NO_COMMON_TYPE`. Keeping this name
    // distinct from the raw `classified_at` column used in `WHERE`/`ORDER BY`
    // below is the fix; see `get_cluster_map_data`.
    classified_at_iso: String,
    source: String,
}

#[derive(Debug, Deserialize, Row)]
struct ClusterInfoQueryRow {
    cluster_id: i32,
    intent_name: String,
    intent_description: String,
    sample_questions: String,
}

#[derive(Debug, Deserialize, Row)]
struct TraceEnrichmentQueryRow {
    trace_id: String,
    status_code: String,
    duration_ns: i64,
}

#[derive(Debug, Deserialize, Row)]
struct CountOnly {
    count: u64,
}

/// ClickHouse row mirror for inserts into `observability_spans`.
#[derive(Debug, serde::Serialize, Row)]
struct SpanInsertRow {
    trace_id: String,
    span_id: String,
    parent_span_id: String,
    span_name: String,
    service_name: String,
    span_attributes: String,
    duration_ns: i64,
    status_code: String,
    status_message: String,
    event_data: String,
    /// Unix nanoseconds (DateTime64(9) stored as Int64 on the wire).
    timestamp: i64,
    workspace_id: String,
}

/// [`SpanInsertRow`] for a table that has no `workspace_id` column yet. An
/// insert names every column it writes, so writing the tenant column to a
/// table without one fails the whole batch — and a missing ALTER privilege
/// must not cost the deployment its traces. See `spans_are_scoped`.
#[derive(Debug, serde::Serialize, Row)]
struct UnscopedSpanInsertRow {
    trace_id: String,
    span_id: String,
    parent_span_id: String,
    span_name: String,
    service_name: String,
    span_attributes: String,
    duration_ns: i64,
    status_code: String,
    status_message: String,
    event_data: String,
    timestamp: i64,
}

impl From<SpanInsertRow> for UnscopedSpanInsertRow {
    fn from(row: SpanInsertRow) -> Self {
        Self {
            trace_id: row.trace_id,
            span_id: row.span_id,
            parent_span_id: row.parent_span_id,
            span_name: row.span_name,
            service_name: row.service_name,
            span_attributes: row.span_attributes,
            duration_ns: row.duration_ns,
            status_code: row.status_code,
            status_message: row.status_message,
            event_data: row.event_data,
            timestamp: row.timestamp,
        }
    }
}

fn duration_interval(dur: Option<&str>) -> Option<&'static str> {
    crate::duration::clickhouse_interval(dur)
}

/// Escape a string for inclusion as a ClickHouse SQL string literal.
///
/// ClickHouse reads backslash escapes inside a string literal, so doubling
/// the quote alone is not an escape: `\'` would arrive as `\''`, which is an
/// escaped quote followed by the literal's terminator, and whatever follows
/// is SQL. The backslash is doubled **first**, so the quote's own escape
/// cannot be claimed by one the caller typed.
fn escape_sql_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "''")
}

/// Escape LIKE/ILIKE wildcard metacharacters (`\`, `%`, `_`) so free-text
/// search matches literally instead of as a pattern — a query like `50%` or
/// `user_id` must not turn `%`/`_` into wildcards. `\` is ClickHouse's default
/// LIKE escape character. The result must still be passed through
/// [`escape_sql_literal`] for the SQL string literal it's interpolated into —
/// which doubles each backslash written here, so the literal ClickHouse
/// decodes is exactly this pattern.
fn escape_like_pattern(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// The `WHERE` of the trace list: `scope`'s root spans, narrowed by the
/// caller's filters. One clause set for both the count and the page, so the
/// two cannot disagree about whose rows they are counting.
fn trace_list_where(
    scope: &WorkspaceScope,
    agent_ref: Option<&str>,
    status: Option<&str>,
    duration_filter: Option<&str>,
    search: Option<&str>,
    from_ts: Option<i64>,
    to_ts: Option<i64>,
) -> String {
    let mut conditions = vec![
        // First, and not optional: every other condition narrows a set that is
        // already this workspace's.
        scope.spans_predicate("s."),
        "s.span_name IN ('workflow.run_workflow', 'agent.run_agent', 'analytics.run')".to_string(),
        "s.parent_span_id = ''".to_string(),
    ];

    if let Some(agent) = agent_ref {
        conditions.push(format!(
            "JSONExtractString(s.span_attributes, 'oxy.agent.ref') = '{}'",
            escape_sql_literal(agent)
        ));
    }

    if let Some(st) = status {
        conditions.push(format!("s.status_code = '{}'", escape_sql_literal(st)));
    }

    // Absolute time range (Theme 3) overrides the preset duration window when set
    // (epoch seconds).
    if from_ts.is_some() || to_ts.is_some() {
        if let Some(from) = from_ts {
            conditions.push(format!("s.timestamp >= toDateTime({from})"));
        }
        if let Some(to) = to_ts {
            conditions.push(format!("s.timestamp <= toDateTime({to})"));
        }
    } else if let Some(interval) = duration_interval(duration_filter) {
        conditions.push(format!("s.timestamp >= now() - {interval}"));
    }

    // Free-text search (Theme 3): trace id (exact) OR case-insensitive substring
    // on span name / agent ref / prompt. Kept to specific keys — not a full
    // span_attributes scan — to stay cheap on the big spans table.
    if let Some(q) = search.map(str::trim).filter(|q| !q.is_empty()) {
        // trace_id matches exactly; the ILIKE substring branches match the
        // query literally (LIKE metacharacters escaped) so `%`/`_` typed into
        // a prompt search aren't treated as wildcards.
        let exact = escape_sql_literal(q);
        let like = escape_sql_literal(&escape_like_pattern(q));
        conditions.push(format!(
            "(s.trace_id = '{exact}' \
             OR s.span_name ILIKE '%{like}%' \
             OR JSONExtractString(s.span_attributes, 'oxy.agent.ref') ILIKE '%{like}%' \
             OR JSONExtractString(s.span_attributes, 'agent.prompt') ILIKE '%{like}%')"
        ));
    }

    conditions.join(" AND ")
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn list_traces(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    limit: i64,
    offset: i64,
    agent_ref: Option<&str>,
    status: Option<&str>,
    duration_filter: Option<&str>,
    search: Option<&str>,
    from_ts: Option<i64>,
    to_ts: Option<i64>,
) -> Result<(Vec<TraceRow>, i64), OxyError> {
    storage.require_scoped_spans()?;
    let where_clause = trace_list_where(
        scope,
        agent_ref,
        status,
        duration_filter,
        search,
        from_ts,
        to_ts,
    );

    let count_sql =
        format!("SELECT count() AS count FROM observability_spans s WHERE {where_clause}");
    let total: u64 = super::with_query_timeout("traces count", async {
        storage
            .read_client()
            .query(&count_sql)
            .fetch_one::<CountOnly>()
            .await
            .map(|r| r.count)
            .map_err(|e| OxyError::RuntimeError(format!("Count query failed: {e}")))
    })
    .await?;

    let ts = super::iso_utc("r.timestamp");
    let data_sql = format!(
        "WITH root_traces AS (
            SELECT trace_id, span_id, timestamp, span_name, service_name,
                   duration_ns, status_code, status_message,
                   span_attributes, event_data
            FROM observability_spans s
            WHERE {where_clause}
            ORDER BY s.timestamp DESC
            LIMIT {limit} OFFSET {offset}
        ),
        token_agg AS (
            SELECT
                s2.trace_id,
                sum(toInt64OrZero(JSONExtractString(ev, 'attributes', 'prompt_tokens'))) AS prompt_tokens,
                sum(toInt64OrZero(JSONExtractString(ev, 'attributes', 'completion_tokens'))) AS completion_tokens,
                sum(toInt64OrZero(JSONExtractString(ev, 'attributes', 'total_tokens'))) AS total_tokens
            FROM observability_spans AS s2
            ARRAY JOIN JSONExtractArrayRaw(s2.event_data) AS ev
            WHERE s2.trace_id IN (SELECT trace_id FROM root_traces)
              AND JSONExtractString(ev, 'name') = 'llm.usage'
            GROUP BY s2.trace_id
        )
        SELECT
            r.trace_id AS trace_id,
            r.span_id AS span_id,
            {ts} AS timestamp,
            r.span_name AS span_name,
            r.service_name AS service_name,
            r.duration_ns AS duration_ns,
            r.status_code AS status_code,
            r.status_message AS status_message,
            r.span_attributes AS span_attributes,
            r.event_data AS event_data,
            coalesce(t.prompt_tokens, 0) AS prompt_tokens,
            coalesce(t.completion_tokens, 0) AS completion_tokens,
            coalesce(t.total_tokens, 0) AS total_tokens
        FROM root_traces r
        LEFT JOIN token_agg t ON r.trace_id = t.trace_id
        ORDER BY r.timestamp DESC"
    );

    let rows: Vec<TraceQueryRow> = super::with_query_timeout("traces list", async {
        storage
            .read_client()
            .query(&data_sql)
            .fetch_all()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Traces query failed: {e}")))
    })
    .await?;

    let traces = rows
        .into_iter()
        .map(|r| TraceRow {
            trace_id: r.trace_id,
            span_id: r.span_id,
            timestamp: r.timestamp,
            span_name: r.span_name,
            service_name: r.service_name,
            duration_ns: r.duration_ns,
            status_code: r.status_code,
            status_message: r.status_message,
            span_attributes: r.span_attributes,
            event_data: r.event_data,
            prompt_tokens: r.prompt_tokens,
            completion_tokens: r.completion_tokens,
            total_tokens: r.total_tokens,
        })
        .collect();

    Ok((traces, total as i64))
}

/// Every span of one trace, if the trace is `scope`'s.
///
/// The workspace predicate is on each row rather than on the root alone: the
/// layer stamps a trace's every span, so this cannot return a mixed trace, and
/// a trace id from another workspace matches no row — the same answer as an id
/// that does not exist, which is what lets the handler 404 both alike.
fn trace_detail_sql(scope: &WorkspaceScope, trace_id: &str) -> String {
    // A single trace should never approach this many spans; the cap guards the
    // request path (and the instance's memory) against a pathological or
    // colliding trace_id returning an unbounded result set.
    const MAX_SPANS: usize = 100_000;
    let ts = super::iso_utc("timestamp");
    let trace = escape_sql_literal(trace_id);
    let in_scope = scope.spans_predicate("");
    format!(
        "SELECT
            {ts} AS timestamp,
            trace_id,
            span_id,
            parent_span_id,
            span_name,
            service_name,
            span_attributes,
            duration_ns,
            status_code,
            status_message,
            event_data
        FROM observability_spans
        WHERE trace_id = '{trace}' AND {in_scope}
        ORDER BY timestamp ASC
        LIMIT {MAX_SPANS}"
    )
}

pub(super) async fn get_trace_detail(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    trace_id: &str,
) -> Result<Vec<TraceDetailRow>, OxyError> {
    storage.require_scoped_spans()?;
    let sql = trace_detail_sql(scope, trace_id);

    let rows: Vec<TraceDetailQueryRow> = super::with_query_timeout("trace detail", async {
        storage
            .read_client()
            .query(&sql)
            .fetch_all()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Trace detail query failed: {e}")))
    })
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| TraceDetailRow {
            timestamp: r.timestamp,
            trace_id: r.trace_id,
            span_id: r.span_id,
            parent_span_id: r.parent_span_id,
            span_name: r.span_name,
            service_name: r.service_name,
            span_attributes: r.span_attributes,
            duration_ns: r.duration_ns,
            status_code: r.status_code,
            status_message: r.status_message,
            event_data: r.event_data,
        })
        .collect())
}

/// Builds the `get_cluster_map_data` query text. Pulled out of the `async fn`
/// so `cluster_map_sql_alias_does_not_shadow_the_where_column` can assert on
/// its shape without a live ClickHouse.
///
/// The output alias must NOT be named `classified_at` — see the doc comment on
/// `ClusterMapQueryRow::classified_at_iso`. `WHERE`/`ORDER BY` stay on the
/// bare, unaliased `classified_at`, which now unambiguously means the real
/// DateTime column.
fn cluster_map_sql(where_clause: &str, limit: usize) -> String {
    let ca = super::iso_utc("classified_at");
    format!(
        "SELECT
            trace_id,
            question,
            embedding,
            cluster_id,
            intent_name,
            confidence,
            {ca} AS classified_at_iso,
            source
        FROM observability_intent_classifications FINAL
        WHERE {where_clause}
        ORDER BY classified_at DESC
        LIMIT {limit}"
    )
}

/// The `WHERE` of the cluster map: classifications of questions asked in
/// `scope`. The classification table has a `trace_id` and no tenant column, so
/// the workspace comes from the spans that trace is made of — windowed like
/// the map itself, or every page load would walk the whole retention of the
/// largest table to find them. A question classified more than a day after
/// the window its run started in is left out: hidden, never leaked, the same
/// rule the rollup reads follow.
fn cluster_map_where(scope: &WorkspaceScope, days: u32, source: Option<&str>) -> String {
    let mut conditions = vec![
        scope.rollup_predicate(days),
        format!("classified_at >= now() - INTERVAL {days} DAY"),
    ];
    if let Some(src) = source {
        conditions.push(format!("source = '{}'", escape_sql_literal(src)));
    }
    conditions.join(" AND ")
}

pub(super) async fn get_cluster_map_data(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
    limit: usize,
    source: Option<&str>,
) -> Result<Vec<ClusterMapDataRow>, OxyError> {
    storage.require_scoped_spans()?;
    let where_clause = cluster_map_where(scope, days, source);
    let sql = cluster_map_sql(&where_clause, limit);

    let rows: Vec<ClusterMapQueryRow> = storage
        .read_client()
        .query(&sql)
        .fetch_all()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Cluster map query failed: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| ClusterMapDataRow {
            trace_id: r.trace_id,
            question: r.question,
            embedding: r.embedding,
            cluster_id: r.cluster_id,
            intent_name: r.intent_name,
            confidence: r.confidence,
            classified_at: r.classified_at_iso,
            source: r.source,
        })
        .collect())
}

pub(super) async fn get_cluster_infos(
    storage: &ClickHouseObservabilityStorage,
) -> Result<Vec<ClusterInfoRow>, OxyError> {
    let sql = "SELECT cluster_id, intent_name, intent_description, sample_questions
        FROM observability_intent_clusters FINAL
        ORDER BY cluster_id";

    let rows: Vec<ClusterInfoQueryRow> = storage
        .read_client()
        .query(sql)
        .fetch_all()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Cluster info query failed: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| ClusterInfoRow {
            cluster_id: r.cluster_id,
            intent_name: r.intent_name,
            intent_description: r.intent_description,
            sample_questions: r.sample_questions,
        })
        .collect())
}

/// Status and duration of those of `trace_ids` that are `scope`'s. An id from
/// another workspace is simply not enriched.
fn trace_enrichments_sql(scope: &WorkspaceScope, trace_ids: &[String]) -> String {
    let list = trace_ids
        .iter()
        .map(|id| format!("'{}'", escape_sql_literal(id)))
        .collect::<Vec<_>>()
        .join(", ");
    let in_scope = scope.spans_predicate("");

    format!(
        "SELECT trace_id, status_code, duration_ns
        FROM observability_spans
        WHERE parent_span_id = ''
          AND {in_scope}
          AND trace_id IN ({list})"
    )
}

pub(super) async fn get_trace_enrichments(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    trace_ids: &[String],
) -> Result<Vec<TraceEnrichmentRow>, OxyError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }
    storage.require_scoped_spans()?;

    let sql = trace_enrichments_sql(scope, trace_ids);

    let rows: Vec<TraceEnrichmentQueryRow> = storage
        .read_client()
        .query(&sql)
        .fetch_all()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Trace enrichment query failed: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| TraceEnrichmentRow {
            trace_id: r.trace_id,
            status_code: r.status_code,
            duration_ns: r.duration_ns,
        })
        .collect())
}

/// Write `rows` to `observability_spans` in one insert.
async fn write_span_rows<T>(
    storage: &ClickHouseObservabilityStorage,
    rows: Vec<T>,
) -> Result<(), OxyError>
where
    T: clickhouse::RowOwned + clickhouse::RowWrite,
{
    let mut insert = storage
        .client()
        .insert::<T>("observability_spans")
        .await
        .map_err(|e| OxyError::RuntimeError(format!("ClickHouse insert init failed: {e}")))?;

    for row in &rows {
        insert
            .write(row)
            .await
            .map_err(|e| OxyError::RuntimeError(format!("ClickHouse span write failed: {e}")))?;
    }

    insert
        .end()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("ClickHouse span insert end failed: {e}")))
}

pub(super) async fn insert_spans(
    storage: &ClickHouseObservabilityStorage,
    spans: Vec<SpanRecord>,
) -> Result<(), OxyError> {
    if spans.is_empty() {
        return Ok(());
    }

    let rows: Vec<SpanInsertRow> = spans
        .into_iter()
        .map(|span| SpanInsertRow {
            timestamp: parse_timestamp_ns(&span.timestamp),
            trace_id: span.trace_id,
            span_id: span.span_id,
            parent_span_id: span.parent_span_id,
            span_name: span.span_name,
            service_name: span.service_name,
            span_attributes: span.span_attributes,
            duration_ns: span.duration_ns,
            status_code: span.status_code,
            status_message: span.status_message,
            event_data: span.event_data,
            workspace_id: span.workspace_id,
        })
        .collect();

    // The shape the table has. Without the tenant column the spans are still
    // kept — nobody can read them per workspace until it exists, and nothing
    // reads them across workspaces either.
    if storage.spans_are_scoped() {
        write_span_rows(storage, rows).await
    } else {
        let unscoped: Vec<UnscopedSpanInsertRow> = rows.into_iter().map(Into::into).collect();
        write_span_rows(storage, unscoped).await
    }
}

/// Parse an RFC3339 timestamp into nanoseconds since Unix epoch.
/// On parse failure, falls back to the current wall clock.
fn parse_timestamp_ns(ts: &str) -> i64 {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => dt.timestamp_nanos_opt().unwrap_or(0),
        Err(_) => chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClickHouseObservabilityStorage, cluster_map_sql, cluster_map_where, escape_like_pattern,
        escape_sql_literal, trace_detail_sql, trace_enrichments_sql, trace_list_where,
    };
    use crate::scope::WorkspaceScope;

    const WORKSPACE: &str = "70787bb2-e11b-5488-b2c3-02e60d5fc7d3";

    fn scope() -> WorkspaceScope {
        WorkspaceScope::of(uuid::Uuid::parse_str(WORKSPACE).unwrap())
    }

    /// The agent tables are shared by every tenant, so a tenant-facing read
    /// that does not name its workspace returns everyone's rows. Each query a
    /// handler under `/{workspace_id}/traces` can reach is listed here; a new
    /// one belongs in this list before it belongs in a route.
    #[test]
    fn every_trace_read_names_its_workspace() {
        let on_spans = format!("workspace_id = '{WORKSPACE}'");
        let reads = [
            (
                "list",
                trace_list_where(&scope(), None, None, None, None, None, None),
            ),
            (
                "list, every filter set",
                trace_list_where(
                    &scope(),
                    Some("agents/sales"),
                    Some("Error"),
                    Some("7d"),
                    Some("revenue"),
                    Some(1_700_000_000),
                    Some(1_700_003_600),
                ),
            ),
            ("detail", trace_detail_sql(&scope(), "abc123")),
            (
                "enrichments",
                trace_enrichments_sql(&scope(), &["abc123".to_string()]),
            ),
            (
                "cluster map",
                cluster_map_where(&scope(), 30, Some("agent")),
            ),
        ];
        for (read, sql) in reads {
            assert!(sql.contains(&on_spans), "{read} is not scoped:\n{sql}");
        }
    }

    /// The caller's filters are `AND`ed onto a set that is already the
    /// workspace's. The search clause is the only `OR` in the list, and it has
    /// to stay inside its own parentheses: bare, `a AND b OR c` would let any
    /// search match rows outside the workspace.
    #[test]
    fn list_filters_narrow_the_workspace_and_cannot_widen_it() {
        let sql = trace_list_where(&scope(), None, None, None, Some("revenue"), None, None);
        assert!(
            sql.starts_with(&format!("s.workspace_id = '{WORKSPACE}' AND ")),
            "{sql}"
        );
        let search = sql
            .find("(s.trace_id = 'revenue'")
            .expect("the search group");
        assert!(sql[search..].ends_with(')'), "{sql}");
        assert_eq!(
            sql[..search].matches(" OR ").count(),
            0,
            "an OR outside the search group:\n{sql}"
        );
    }

    /// A trace id is caller input sitting next to the tenant predicate. It
    /// stays inside its literal, and the predicate still follows it.
    #[test]
    fn a_trace_id_cannot_argue_its_way_out_of_the_workspace() {
        let hostile = "x' OR workspace_id != '";
        let sql = trace_detail_sql(&scope(), hostile);
        assert!(
            sql.contains(&format!(
                "WHERE trace_id = '{}' AND workspace_id = '{WORKSPACE}'",
                escape_sql_literal(hostile)
            )),
            "{sql}"
        );
        let enrich = trace_enrichments_sql(&scope(), &[hostile.to_string()]);
        assert!(
            enrich.contains(&format!("trace_id IN ('{}')", escape_sql_literal(hostile))),
            "{enrich}"
        );
    }

    /// The classification table has no tenant column, so the cluster map is
    /// confined through the spans its traces are made of — and by nothing
    /// looser than this workspace's own roots.
    #[test]
    fn the_cluster_map_is_confined_through_this_workspaces_traces() {
        let sql = cluster_map_where(&scope(), 30, None);
        assert!(
            sql.starts_with(&format!("trace_id IN ({}", scope().trace_ids())),
            "{sql}"
        );
        // …and through no more of the spans table than the window it shows: an
        // unbounded subquery reads all 90 days of it on every page load.
        assert!(
            sql.starts_with(&scope().rollup_predicate(30)),
            "the scope subquery is not windowed:\n{sql}"
        );
    }

    /// The probe's answer is latched for the life of the process. "The server
    /// did not answer" must not be recorded as "the column is absent": that
    /// instance would then write every span unstamped — in nobody's console,
    /// permanently — against a table that has the column.
    #[tokio::test]
    async fn a_probe_that_cannot_be_answered_fails_the_open_instead_of_latching_unscoped() {
        // Nothing listens here, so the ALTER and the probe both fail to connect.
        let storage =
            ClickHouseObservabilityStorage::new("http://127.0.0.1:1", "u", "p", "observability")
                .unwrap();
        let failed = storage
            .ensure_scoped_spans()
            .await
            .expect_err("an unanswered probe is an error, not a `false`")
            .to_string();
        assert!(failed.contains("tenant-column probe failed"), "{failed}");
        assert!(!storage.spans_are_scoped());
    }

    /// Until `ensure_schema` has seen the tenant column, a read is refused. An
    /// empty list here would be read as "this workspace has no traces".
    #[test]
    fn trace_reads_are_refused_until_the_tenant_column_is_known_to_exist() {
        let storage =
            ClickHouseObservabilityStorage::new("http://127.0.0.1:1", "u", "p", "observability")
                .unwrap();
        assert!(!storage.spans_are_scoped());
        let refused = storage.require_scoped_spans().unwrap_err().to_string();
        assert!(refused.contains("workspace_id"), "{refused}");
    }

    /// Regression for the cluster map's `NO_COMMON_TYPE` (ClickHouse error
    /// code 386): the query filters `WHERE classified_at >= now() - INTERVAL
    /// n DAY` on the bare column, so a SELECT-list alias of the same name
    /// gets resolved by ClickHouse's analyzer in place of the column even
    /// inside WHERE — comparing the alias's `formatDateTime(...)` String
    /// against a DateTime. Reverting `cluster_map_sql` to alias the formatted
    /// column back to `classified_at` reproduces the collision this asserts
    /// against (verified with `git stash` — see the branch's test commit).
    #[test]
    fn cluster_map_sql_alias_does_not_shadow_the_where_column() {
        let sql = cluster_map_sql("classified_at >= now() - INTERVAL 7 DAY", 50);
        assert!(
            sql.contains("AS classified_at_iso"),
            "expected the formatted column aliased to a name distinct from \
             the raw `classified_at` column:\n{sql}"
        );
        assert!(
            !sql.contains("AS classified_at,") && !sql.contains("AS classified_at\n"),
            "the SELECT list re-introduced `classified_at` as an alias, which \
             shadows the bare `classified_at` filtered in WHERE:\n{sql}"
        );
    }

    #[test]
    fn sql_literal_doubles_single_quotes() {
        assert_eq!(escape_sql_literal("O'Brien"), "O''Brien");
        assert_eq!(escape_sql_literal("plain"), "plain");
    }

    /// Whether `body`, written between two quotes, is one whole literal to
    /// ClickHouse's lexer: a backslash takes the next character with it, a
    /// doubled quote is a quote, and a lone quote would end the literal.
    fn stays_inside_its_literal(body: &str) -> bool {
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            let paired = match c {
                '\\' => chars.next().is_some(),
                '\'' => chars.next() == Some('\''),
                _ => true,
            };
            if !paired {
                return false;
            }
        }
        true
    }

    /// A doubled quote alone does not hold a value in: a backslash in front
    /// of it makes the first half an escaped quote and the second the
    /// terminator. The trace search box, the agent and status filters, a
    /// trace id and an enrichment's ids all reach a literal this way.
    #[test]
    fn a_backslash_cannot_end_the_literal_early() {
        assert_eq!(escape_sql_literal("\\"), "\\\\");
        assert_eq!(escape_sql_literal("x\\' OR 1=1 -- "), "x\\\\'' OR 1=1 -- ");
        for typed in [
            "\\",
            "\\' OR 1=1 -- ",
            "x\\' OR 1=1 -- ",
            "\\\\' OR 1=1 -- ",
            "' OR '1'='1",
        ] {
            assert!(
                stays_inside_its_literal(&escape_sql_literal(typed)),
                "{typed:?} ends its literal: {}",
                escape_sql_literal(typed)
            );
            assert!(
                stays_inside_its_literal(&escape_sql_literal(&escape_like_pattern(typed))),
                "{typed:?} ends its LIKE literal"
            );
        }
    }

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(escape_like_pattern("50%"), "50\\%");
        assert_eq!(escape_like_pattern("user_id"), "user\\_id");
        assert_eq!(escape_like_pattern("a\\b"), "a\\\\b");
        assert_eq!(escape_like_pattern("plain"), "plain");
    }

    #[test]
    fn like_pattern_composes_with_sql_literal() {
        // A prompt containing both a quote and a wildcard: metacharacters get a
        // backslash, then the surrounding literal doubles that backslash and
        // the quote — ClickHouse decodes it back to the pattern `it's 50\%`.
        assert_eq!(
            escape_sql_literal(&escape_like_pattern("it's 50%")),
            "it''s 50\\\\%"
        );
    }
}
