//! Execution analytics queries against the `observability_executions` rollup.
//!
//! Each row is one flattened `tool_call` span (produced by
//! [`super::schema::EXECUTIONS_SELECT`]), so every panel here is a plain
//! scan/aggregation over a pre-reduced table — no `spans ⋈ spans` self-join and
//! no per-row `JSONExtract` over `span_attributes`/`event_data` (axiom S2). The
//! rollup already encodes `is_verified`, `is_success`, `agent_ref`,
//! `user_question`, and the error/input/output text, so the read path just reads
//! columns.

use clickhouse::Row;
use oxy_shared::errors::OxyError;
use serde::Deserialize;

use super::ClickHouseObservabilityStorage;
use crate::scope::WorkspaceScope;
use crate::types::{
    AgentExecutionStatsData, ExecutionDetailData, ExecutionListData, ExecutionSummaryData,
    ExecutionTimeBucketData,
};

/// What every rollup read starts from: `scope`'s executions of the last
/// `days`. The time bound and the tenant bound travel together on purpose —
/// each query in this file builds its `WHERE` from this one function, so there
/// is no way to write one that has the window and not the workspace.
fn since(scope: &WorkspaceScope, days: u32) -> String {
    format!(
        "timestamp >= now() - INTERVAL {days} DAY AND {}",
        scope.rollup_predicate(days)
    )
}

fn escape_sql_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

#[derive(Debug, Deserialize, Row)]
struct ExecutionSummaryDbRow {
    total_executions: u64,
    verified_count: u64,
    generated_count: u64,
    success_count_verified: u64,
    success_count_generated: u64,
    semantic_query_count: u64,
    omni_query_count: u64,
    sql_generated_count: u64,
    workflow_count: u64,
    agent_tool_count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct ExecutionTimeBucketDbRow {
    date: String,
    verified_count: u64,
    generated_count: u64,
    semantic_query_count: u64,
    omni_query_count: u64,
    sql_generated_count: u64,
    workflow_count: u64,
    agent_tool_count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct AgentStatsDbRow {
    agent_ref: String,
    total_executions: u64,
    verified_count: u64,
    generated_count: u64,
    success_count: u64,
    semantic_query_count: u64,
    omni_query_count: u64,
    sql_generated_count: u64,
    workflow_count: u64,
    agent_tool_count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct ExecutionDetailDbRow {
    trace_id: String,
    span_id: String,
    // Deliberately NOT named `timestamp`: ClickHouse resolves a bare `WHERE
    // timestamp …`/`ORDER BY timestamp` reference against a SELECT-list alias
    // of the same name in preference to the source column, even though the
    // alias's expression (`formatDateTime(...)`, a String) shares no type
    // with the DateTime `since()` compares it against — `NO_COMMON_TYPE`
    // (ClickHouse error code 386). See `execution_list_sql`.
    timestamp_iso: String,
    execution_type: String,
    // Deliberately NOT named `is_verified`, for the reason `timestamp_iso`
    // above is not named `timestamp`: the `?is_verified=` filter adds `AND
    // is_verified = 0` to the `WHERE`, ClickHouse resolves that name to this
    // SELECT-list alias (a String) instead of the UInt8 column, and the query
    // fails with `NO_COMMON_TYPE` — so filtering the list by verified answered
    // a 500 every time.
    is_verified_label: String,
    source_type: String,
    source_ref: String,
    status: String,
    duration_ns: i64,
    database: String,
    topic: String,
    semantic_query_params: String,
    generated_sql: String,
    integration: String,
    endpoint: String,
    sql: String,
    sql_ref: String,
    user_question: String,
    workflow_ref: String,
    agent_ref: String,
    tool_input: String,
    input: String,
    output: String,
    error: String,
}

#[derive(Debug, Deserialize, Row)]
struct CountOnly {
    count: u64,
}

/// Per-execution-type count expressions shared by summary + time series.
const TYPE_COUNTS: &str = "\
    countIf(execution_type = 'semantic_query') AS semantic_query_count, \
    countIf(execution_type = 'omni_query') AS omni_query_count, \
    countIf(execution_type = 'sql_generated') AS sql_generated_count, \
    countIf(execution_type = 'workflow') AS workflow_count, \
    countIf(execution_type = 'agent_tool') AS agent_tool_count";

pub(super) async fn get_execution_summary(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
) -> Result<ExecutionSummaryData, OxyError> {
    let sql = format!(
        "SELECT
            count() AS total_executions,
            countIf(is_verified = 1) AS verified_count,
            countIf(is_verified = 0) AS generated_count,
            countIf(is_verified = 1 AND is_success = 1) AS success_count_verified,
            countIf(is_verified = 0 AND is_success = 1) AS success_count_generated,
            {TYPE_COUNTS}
        FROM observability_executions FINAL
        WHERE {}",
        since(scope, days)
    );

    let row = storage
        .read_client()
        .query(&sql)
        .fetch_optional::<ExecutionSummaryDbRow>()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Execution summary query failed: {e}")))?
        .unwrap_or(ExecutionSummaryDbRow {
            total_executions: 0,
            verified_count: 0,
            generated_count: 0,
            success_count_verified: 0,
            success_count_generated: 0,
            semantic_query_count: 0,
            omni_query_count: 0,
            sql_generated_count: 0,
            workflow_count: 0,
            agent_tool_count: 0,
        });

    Ok(ExecutionSummaryData {
        total_executions: row.total_executions,
        verified_count: row.verified_count,
        generated_count: row.generated_count,
        success_count_verified: row.success_count_verified,
        success_count_generated: row.success_count_generated,
        semantic_query_count: row.semantic_query_count,
        omni_query_count: row.omni_query_count,
        sql_generated_count: row.sql_generated_count,
        workflow_count: row.workflow_count,
        agent_tool_count: row.agent_tool_count,
    })
}

pub(super) async fn get_execution_time_series(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
) -> Result<Vec<ExecutionTimeBucketData>, OxyError> {
    let sql = format!(
        "SELECT
            formatDateTime(toDate(timestamp), '%Y-%m-%d') AS date,
            countIf(is_verified = 1) AS verified_count,
            countIf(is_verified = 0) AS generated_count,
            {TYPE_COUNTS}
        FROM observability_executions FINAL
        WHERE {}
        GROUP BY date
        ORDER BY date ASC",
        since(scope, days)
    );

    let rows: Vec<ExecutionTimeBucketDbRow> =
        storage
            .read_client()
            .query(&sql)
            .fetch_all()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Time series query failed: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| ExecutionTimeBucketData {
            date: r.date,
            verified_count: r.verified_count,
            generated_count: r.generated_count,
            semantic_query_count: r.semantic_query_count,
            omni_query_count: r.omni_query_count,
            sql_generated_count: r.sql_generated_count,
            workflow_count: r.workflow_count,
            agent_tool_count: r.agent_tool_count,
        })
        .collect())
}

pub(super) async fn get_execution_agent_stats(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
    limit: usize,
) -> Result<Vec<AgentExecutionStatsData>, OxyError> {
    let sql = format!(
        "SELECT
            agent_ref,
            count() AS total_executions,
            countIf(is_verified = 1) AS verified_count,
            countIf(is_verified = 0) AS generated_count,
            countIf(is_success = 1) AS success_count,
            {TYPE_COUNTS}
        FROM observability_executions FINAL
        WHERE {}
        GROUP BY agent_ref
        ORDER BY total_executions DESC
        LIMIT {limit}",
        since(scope, days)
    );

    let rows: Vec<AgentStatsDbRow> = storage
        .read_client()
        .query(&sql)
        .fetch_all()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Agent stats query failed: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| AgentExecutionStatsData {
            agent_ref: r.agent_ref,
            total_executions: r.total_executions,
            verified_count: r.verified_count,
            generated_count: r.generated_count,
            success_count: r.success_count,
            semantic_query_count: r.semantic_query_count,
            omni_query_count: r.omni_query_count,
            sql_generated_count: r.sql_generated_count,
            workflow_count: r.workflow_count,
            agent_tool_count: r.agent_tool_count,
        })
        .collect())
}

/// Builds the paginated `data_sql` for [`get_execution_list`]. Pulled out of
/// the `async fn` so a unit test can assert on its shape without a live
/// ClickHouse (see `execution_list_sql_alias_does_not_shadow_the_where_column`).
///
/// The output alias must NOT be named `timestamp` — see the doc comment on
/// `ExecutionDetailDbRow::timestamp_iso`. `WHERE`/`ORDER BY` stay on the bare,
/// unaliased `timestamp`, which now unambiguously means the real DateTime
/// column `since()` filters on.
fn execution_list_data_sql(
    scope: &WorkspaceScope,
    extra_where: &str,
    days: u32,
    limit: usize,
    offset: usize,
) -> String {
    let ts = super::iso_utc("timestamp");
    let since_clause = since(scope, days);
    format!(
        "SELECT
            trace_id,
            span_id,
            {ts} AS timestamp_iso,
            execution_type,
            if(is_verified = 1, 'true', 'false') AS is_verified_label,
            source_type,
            source_ref,
            if(is_success = 1, 'success', 'error') AS status,
            duration_ns,
            database,
            topic,
            semantic_query_params,
            generated_sql,
            integration,
            endpoint,
            sql,
            sql_ref,
            user_question,
            workflow_ref,
            agent_ref,
            tool_input,
            tool_input AS input,
            tool_output AS output,
            error_message AS error
        FROM observability_executions FINAL
        WHERE {since_clause}{extra_where}
        ORDER BY timestamp DESC
        LIMIT {limit} OFFSET {offset}"
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn get_execution_list(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
    limit: usize,
    offset: usize,
    execution_type: Option<&str>,
    is_verified: Option<bool>,
    source_ref: Option<&str>,
    status: Option<&str>,
) -> Result<ExecutionListData, OxyError> {
    let mut extra_conditions: Vec<String> = Vec::new();

    if let Some(et) = execution_type {
        extra_conditions.push(format!("execution_type = '{}'", escape_sql_literal(et)));
    }
    if let Some(verified) = is_verified {
        extra_conditions.push(format!("is_verified = {}", if verified { 1 } else { 0 }));
    }
    if let Some(sr) = source_ref {
        extra_conditions.push(format!("source_ref = '{}'", escape_sql_literal(sr)));
    }
    if let Some(st) = status {
        match st {
            "error" => extra_conditions.push("is_success = 0".into()),
            "success" => extra_conditions.push("is_success = 1".into()),
            _ => {}
        }
    }

    let extra_where = if extra_conditions.is_empty() {
        String::new()
    } else {
        format!(" AND {}", extra_conditions.join(" AND "))
    };

    let count_sql = format!(
        "SELECT count() AS count FROM observability_executions FINAL WHERE {}{extra_where}",
        since(scope, days)
    );

    let total = storage
        .read_client()
        .query(&count_sql)
        .fetch_one::<CountOnly>()
        .await
        .map(|r| r.count)
        .map_err(|e| OxyError::RuntimeError(format!("Count query failed: {e}")))?;

    let data_sql = execution_list_data_sql(scope, &extra_where, days, limit, offset);

    let rows: Vec<ExecutionDetailDbRow> = storage
        .read_client()
        .query(&data_sql)
        .fetch_all()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Execution list query failed: {e}")))?;

    let executions = rows
        .into_iter()
        .map(|r| ExecutionDetailData {
            trace_id: r.trace_id,
            span_id: r.span_id,
            timestamp: r.timestamp_iso,
            execution_type: r.execution_type,
            is_verified: r.is_verified_label,
            source_type: r.source_type,
            source_ref: r.source_ref,
            status: r.status,
            duration_ns: r.duration_ns,
            database: r.database,
            topic: r.topic,
            semantic_query_params: r.semantic_query_params,
            generated_sql: r.generated_sql,
            integration: r.integration,
            endpoint: r.endpoint,
            sql: r.sql,
            sql_ref: r.sql_ref,
            user_question: r.user_question,
            workflow_ref: r.workflow_ref,
            agent_ref: r.agent_ref,
            tool_input: r.tool_input,
            input: r.input,
            output: r.output,
            error: r.error,
        })
        .collect();

    Ok(ExecutionListData {
        executions,
        total,
        limit,
        offset,
    })
}

#[cfg(test)]
mod tests {
    use super::{execution_list_data_sql, since};
    use crate::scope::WorkspaceScope;

    fn scope() -> WorkspaceScope {
        WorkspaceScope::of(uuid::Uuid::parse_str("70787bb2-e11b-5488-b2c3-02e60d5fc7d3").unwrap())
    }

    /// The rollup carries questions, generated SQL and tool output for every
    /// tenant. Each read in this file takes its `WHERE` from `since`, so this
    /// is the one place the workspace can be dropped from all of them at once.
    #[test]
    fn every_rollup_read_starts_inside_the_workspace() {
        let window = since(&scope(), 30);
        assert_eq!(
            window,
            format!(
                "timestamp >= now() - INTERVAL 30 DAY AND {}",
                scope().rollup_predicate(30)
            )
        );
        // The page query is built separately; it has to start from the same place.
        let page = execution_list_data_sql(&scope(), " AND is_success = 0", 30, 10, 0);
        assert!(
            page.contains(&format!("WHERE {window} AND is_success = 0")),
            "{page}"
        );
    }

    /// Regression for "Recent Executions" swallowing a `NO_COMMON_TYPE`
    /// (ClickHouse error code 386): `since(days)` filters `WHERE timestamp >=
    /// now() - INTERVAL n DAY` on the bare column, so a SELECT-list alias of
    /// the same name gets resolved by ClickHouse's analyzer in place of the
    /// column even inside WHERE — comparing the alias's `formatDateTime(...)`
    /// String against a DateTime. The frontend then reads the failed request
    /// as an empty list (`data?.executions ?? []`), which is what makes this
    /// worse than a visible 500. Reverting `execution_list_data_sql` to alias
    /// the formatted column back to `timestamp` reproduces the collision this
    /// asserts against.
    #[test]
    fn execution_list_sql_alias_does_not_shadow_the_where_column() {
        let sql = execution_list_data_sql(&scope(), "", 7, 10, 0);
        assert!(
            sql.contains("AS timestamp_iso"),
            "expected the formatted column aliased to a name distinct from \
             the raw `timestamp` column:\n{sql}"
        );
        assert!(
            !sql.contains("AS timestamp,") && !sql.contains("AS timestamp\n"),
            "the SELECT list re-introduced `timestamp` as an alias, which \
             shadows the bare `timestamp` filtered in WHERE:\n{sql}"
        );
        // The same collision one column over: `?is_verified=` puts the bare
        // `is_verified` in the WHERE, so the label cannot be aliased to it.
        // Found by running the filtered list against a real server
        // (`scope_live_tests`), where it had been failing with code 386.
        assert!(
            sql.contains("AS is_verified_label,") && !sql.contains("AS is_verified,"),
            "the verified label is aliased over the `is_verified` column the \
             filter compares:\n{sql}"
        );
    }
}
