//! ClickHouse DDL for observability tables.
//!
//! # Partitioning
//!
//! Event tables partition by **day**, not month. This is not a tuning
//! preference — monthly partitioning wedged the oxy-dev observability
//! ClickHouse on 2026-09-03.
//!
//! A month of spans accumulates into parts of millions of rows. Merging those
//! re-reads hundreds of MiB, which fills the page cache inside the container's
//! cgroup, which squeezes ClickHouse's memory tracker, which fails the merge,
//! which is then retried — re-reading the same parts again. Self-sustaining:
//! the failing merges cause the memory pressure that fails them. That instance
//! reached a 121-part backlog and ~54% merge failure while the log-store
//! ClickHouse beside it, identical but partitioned by day, sat at 0 failures.
//!
//! Daily partitioning also makes retention honest. `apply_retention_ttl` sets
//! a 90-day TTL, and with `ttl_only_drop_parts = 1` a part is dropped only
//! once every row in it has expired. Under monthly partitioning that rounds
//! retention up to as much as ~120 days; under daily it drops the day that
//! actually expired.
//!
//! **`observability_intent_classifications` is deliberately left
//! unpartitioned.** It is a `ReplacingMergeTree` keyed
//! `ORDER BY (trace_id, question)` with `classified_at` as the version column,
//! and deduplication happens WITHIN a partition. Partitioning it by date would
//! put a re-classification of the same question in a different partition from
//! the original, and the two would never collapse. `observability_executions`
//! is safe to partition because its partition key derives from `timestamp`,
//! which is part of its own `ORDER BY` — a replayed row lands in the same
//! partition at any granularity.
//!
//! ## This does not migrate existing tables
//!
//! These are `CREATE TABLE IF NOT EXISTS`, and ClickHouse has no
//! `ALTER TABLE ... MODIFY PARTITION BY` — a partition key is fixed when the
//! table is created. So an existing deployment keeps monthly partitions until
//! someone migrates it deliberately, by creating a day-partitioned table,
//! copying, and swapping via `EXCHANGE TABLES`. New deployments get the right
//! shape from the start; that is the whole of what this file can do on its own.

pub const CREATE_SPANS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS observability_spans (
    trace_id String,
    span_id String,
    parent_span_id String DEFAULT '',
    span_name LowCardinality(String),
    service_name LowCardinality(String) DEFAULT 'oxy',
    span_attributes String DEFAULT '{}',
    duration_ns Int64 DEFAULT 0,
    status_code LowCardinality(String) DEFAULT 'UNSET',
    status_message String DEFAULT '',
    event_data String DEFAULT '[]',
    timestamp DateTime64(9) DEFAULT now64(9)
) ENGINE = MergeTree()
PARTITION BY toDate(timestamp)
ORDER BY (trace_id, span_id, timestamp)
SETTINGS ttl_only_drop_parts = 1
"#;

pub const CREATE_INTENT_CLUSTERS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS observability_intent_clusters (
    cluster_id Int32,
    intent_name String,
    intent_description String DEFAULT '',
    centroid Array(Float32),
    sample_questions String DEFAULT '[]',
    question_count Int64 DEFAULT 0,
    created_at DateTime64(3) DEFAULT now64(3),
    updated_at DateTime64(3) DEFAULT now64(3)
) ENGINE = ReplacingMergeTree(updated_at)
ORDER BY cluster_id
"#;

pub const CREATE_INTENT_CLASSIFICATIONS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS observability_intent_classifications (
    trace_id String,
    question String,
    cluster_id Int32 DEFAULT 0,
    intent_name LowCardinality(String) DEFAULT 'unknown',
    confidence Float32 DEFAULT 0.0,
    embedding Array(Float32),
    source_type LowCardinality(String) DEFAULT 'agent',
    source LowCardinality(String) DEFAULT '',
    classified_at DateTime64(3) DEFAULT now64(3)
) ENGINE = ReplacingMergeTree(classified_at)
ORDER BY (trace_id, question)
"#;

pub const CREATE_METRIC_USAGE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS observability_metric_usage (
    id UUID DEFAULT generateUUIDv4(),
    metric_name LowCardinality(String),
    source_type LowCardinality(String) DEFAULT '',
    source_ref String DEFAULT '',
    context String DEFAULT '',
    context_types String DEFAULT '[]',
    trace_id String DEFAULT '',
    created_at DateTime64(3) DEFAULT now64(3)
) ENGINE = MergeTree()
PARTITION BY toDate(created_at)
ORDER BY (metric_name, source_type, created_at)
SETTINGS ttl_only_drop_parts = 1
"#;

/// Flattened, one-row-per-`tool_call`-span execution rollup. Each completed
/// tool-call span is immutable, so its execution record is derived exactly once
/// (at insert, by `CREATE_EXECUTIONS_MV`) instead of being re-derived from raw
/// spans on every Execution Analytics panel load. `ReplacingMergeTree` keyed by
/// the full `ORDER BY` (which ends in `span_id`) makes the MV and the one-shot
/// backfill idempotent on replay. Column order MUST match the MV's `SELECT`.
pub const CREATE_EXECUTIONS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS observability_executions (
    trace_id String,
    span_id String,
    timestamp DateTime64(9),
    agent_ref LowCardinality(String) DEFAULT '',
    user_question String DEFAULT '',
    execution_type LowCardinality(String) DEFAULT '',
    is_verified UInt8 DEFAULT 0,
    error_message String DEFAULT '',
    is_success UInt8 DEFAULT 1,
    duration_ns Int64 DEFAULT 0,
    source_type LowCardinality(String) DEFAULT '',
    source_ref String DEFAULT '',
    database String DEFAULT '',
    topic String DEFAULT '',
    semantic_query_params String DEFAULT '',
    generated_sql String DEFAULT '',
    sql String DEFAULT '',
    sql_ref String DEFAULT '',
    integration String DEFAULT '',
    endpoint String DEFAULT '',
    workflow_ref String DEFAULT '',
    tool_input String DEFAULT '',
    tool_output String DEFAULT ''
) ENGINE = ReplacingMergeTree
PARTITION BY toDate(timestamp)
ORDER BY (execution_type, agent_ref, timestamp, span_id)
SETTINGS ttl_only_drop_parts = 1
"#;

/// The SELECT that flattens a `tool_call` span into an execution row. Shared by
/// the materialized view (`CREATE_EXECUTIONS_MV`) and the historical backfill so
/// both produce byte-identical rows. `{filter}` is the row-selection predicate;
/// callers append their time bound. All extraction is scalar (`arrayFirst`, not
/// `arrayJoin`) so exactly one row is produced per source span.
pub const EXECUTIONS_SELECT: &str = r#"
SELECT
    trace_id,
    span_id,
    timestamp,
    JSONExtractString(span_attributes, 'oxy.agent.ref') AS agent_ref,
    JSONExtractString(span_attributes, 'agent.prompt') AS user_question,
    JSONExtractString(span_attributes, 'oxy.execution_type') AS execution_type,
    toUInt8(JSONExtractString(span_attributes, 'oxy.is_verified') = 'true') AS is_verified,
    JSONExtractString(arrayFirst(
        x -> JSONExtractString(x, 'name') = 'tool_call.output'
             AND JSONExtractString(x, 'attributes', 'status') = 'error',
        JSONExtractArrayRaw(event_data)), 'attributes', 'error.message') AS error_message,
    -- Success iff the error output event's message is empty — matches the
    -- legacy read-time `error_message IS NULL OR = ''` semantics exactly.
    toUInt8(JSONExtractString(arrayFirst(
        x -> JSONExtractString(x, 'name') = 'tool_call.output'
             AND JSONExtractString(x, 'attributes', 'status') = 'error',
        JSONExtractArrayRaw(event_data)), 'attributes', 'error.message') = '') AS is_success,
    duration_ns,
    JSONExtractString(span_attributes, 'oxy.source_type') AS source_type,
    JSONExtractString(span_attributes, 'oxy.agent.ref') AS source_ref,
    JSONExtractString(span_attributes, 'oxy.database') AS database,
    JSONExtractString(span_attributes, 'oxy.topic') AS topic,
    JSONExtractString(span_attributes, 'oxy.semantic_query_params') AS semantic_query_params,
    JSONExtractString(span_attributes, 'oxy.generated_sql') AS generated_sql,
    JSONExtractString(span_attributes, 'oxy.sql') AS sql,
    JSONExtractString(span_attributes, 'oxy.sql_ref') AS sql_ref,
    JSONExtractString(span_attributes, 'oxy.integration') AS integration,
    JSONExtractString(span_attributes, 'oxy.endpoint') AS endpoint,
    JSONExtractString(span_attributes, 'oxy.workflow_ref') AS workflow_ref,
    JSONExtractString(arrayFirst(
        x -> JSONExtractString(x, 'name') = 'tool_call.input',
        JSONExtractArrayRaw(event_data)), 'attributes', 'input') AS tool_input,
    JSONExtractString(arrayFirst(
        x -> JSONExtractString(x, 'name') = 'tool_call.output'
             AND JSONExtractString(x, 'attributes', 'status') = 'success',
        JSONExtractArrayRaw(event_data)), 'attributes', 'output') AS tool_output
FROM observability_spans
WHERE JSONExtractString(span_attributes, 'oxy.span_type') = 'tool_call'
  AND JSONExtractString(span_attributes, 'oxy.execution_type')
      IN ('semantic_query', 'omni_query', 'sql_generated', 'workflow', 'agent_tool')
  AND JSONExtractString(span_attributes, 'oxy.agent.ref') != ''"#;

/// Materialized view that populates `observability_executions` on every insert
/// into `observability_spans`. Built by concatenating `EXECUTIONS_SELECT` after
/// the `TO` clause in [`super::ClickHouseObservabilityStorage::ensure_schema`],
/// so the flatten logic lives in exactly one place.
pub const CREATE_EXECUTIONS_MV_PREFIX: &str = "CREATE MATERIALIZED VIEW IF NOT EXISTS observability_executions_mv TO observability_executions AS";

/// One wide event per custom-app request — the serve of an HTML shell or asset,
/// an Oxy Function invocation, a data-plane query, or a client-side beacon.
///
/// **This is what makes per-app availability a query rather than a probe.** A
/// custom-app subdomain answers 200 with the SPA shell for every path and every
/// hostname, so an outside-in prober cannot distinguish a healthy app from a
/// typo'd one; but Oxy terminates every request for every app, so the success
/// ratio of real traffic is already in hand and only needed somewhere to land.
/// See `internal-docs/2026-09-04-custom-app-observability-design.md` §3 Layer 1.
///
/// `ORDER BY (org_id, app_id, timestamp)` is **app-first on purpose**: every
/// read is "this app, this window", matching the cache-key rule the
/// `oxy-customer-apps-perf` skill states for the same reason. A timestamp-first
/// key would make the common query scan every tenant.
///
/// `outcome` is stored rather than derived from `status` because the three SRE
/// error classes do not all show up in a status code: an explicit failure is a
/// 5xx, but an *implicit* one is a 200 carrying the wrong thing (an HTML shell
/// that never mounted), and a *policy* one is a 200 that took too long. A
/// derived `status >= 500` would silently report the white-screen case as
/// success, which is the exact failure this table exists to catch.
pub const CREATE_CUSTOM_APP_EVENTS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS custom_app_events (
    timestamp DateTime64(3) DEFAULT now64(3),
    org_id String,
    app_id String,
    build_id String DEFAULT '',
    request_id String DEFAULT '',
    session_id String DEFAULT '',
    user_id String DEFAULT '',
    kind LowCardinality(String),
    route String DEFAULT '',
    status UInt16 DEFAULT 0,
    duration_ms UInt32 DEFAULT 0,
    host_calls UInt32 DEFAULT 0,
    init_ms UInt32 DEFAULT 0,
    bytes UInt64 DEFAULT 0,
    app_role LowCardinality(String) DEFAULT '',
    outcome LowCardinality(String) DEFAULT 'ok',
    error_kind LowCardinality(String) DEFAULT '',
    error_detail String DEFAULT '',
    trace_id String DEFAULT '',
    span_id String DEFAULT ''
) ENGINE = MergeTree()
PARTITION BY toDate(timestamp)
ORDER BY (org_id, app_id, timestamp)
SETTINGS ttl_only_drop_parts = 1
"#;

/// Durable `ctx.log()` / `console.*` output from Oxy Functions.
///
/// Route-mode logs previously existed only inside the HTTP response that
/// carried them: if the caller was a browser that had already navigated away,
/// they were gone. Background runs persisted theirs as `agentic_run_events`
/// rows, so the two modes disagreed about whether a log line survived its run.
/// This table is the shared home; the span events emitted in phase 1 remain the
/// *correlated* view, this is the *complete* one.
///
/// Retention is deliberately shorter than the 90 days the analytics tables get
/// (see `RETENTION_TABLES`): a log line can carry application data a usage
/// count cannot, so it ages out faster.
pub const CREATE_CUSTOM_APP_LOGS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS custom_app_logs (
    timestamp DateTime64(3) DEFAULT now64(3),
    org_id String,
    app_id String,
    build_id String DEFAULT '',
    invocation_id String DEFAULT '',
    request_id String DEFAULT '',
    function_name LowCardinality(String) DEFAULT '',
    mode LowCardinality(String) DEFAULT '',
    log_level LowCardinality(String) DEFAULT 'info',
    seq UInt32 DEFAULT 0,
    message String,
    trace_id String DEFAULT '',
    span_id String DEFAULT ''
) ENGINE = MergeTree()
PARTITION BY toDate(timestamp)
ORDER BY (org_id, app_id, timestamp)
SETTINGS ttl_only_drop_parts = 1
"#;

/// Uncaught browser errors from a custom app, **with message and stack**.
///
/// A separate table from `custom_app_events` on purpose, and the separation is
/// the privacy posture rather than a schema convenience. The analytics tables
/// hold counts and paths and keep 90 days; this holds free text a page threw,
/// which can contain application data — a function that interpolates a row
/// value into an error puts that value here — so it keeps **30 days** (see
/// `RETENTION_TABLES`) behind the same app-admin gate.
///
/// Before this existed the platform stored error *names and counts only*, which
/// made a white-screened app report `{TypeError: 3}` and nothing an engineer
/// could act on. Relaxing that was a deliberate decision, recorded in
/// `internal-docs/2026-09-04-custom-app-observability-design.md` §5.1.
///
/// `stack_hash` is the dedup and grouping key: it is computed client-side over
/// the normalised stack so the same fault recurring 400 times in a render loop
/// arrives as a handful of rows, and so the read path can group occurrences
/// without parsing text.
///
/// `build_id` is what makes a stack resolvable — a minified frame means nothing
/// without the exact build's source map — and what makes "this started with
/// build abc123" answerable.
pub const CREATE_CUSTOM_APP_CLIENT_ERRORS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS custom_app_client_errors (
    timestamp DateTime64(3) DEFAULT now64(3),
    org_id String,
    app_id String,
    build_id String DEFAULT '',
    session_id String DEFAULT '',
    user_id String DEFAULT '',
    error_name LowCardinality(String) DEFAULT 'Error',
    message String DEFAULT '',
    stack String DEFAULT '',
    stack_hash String DEFAULT '',
    path String DEFAULT '',
    kind LowCardinality(String) DEFAULT 'error',
    user_agent String DEFAULT '',
    trace_id String DEFAULT '',
    span_id String DEFAULT ''
) ENGINE = MergeTree()
PARTITION BY toDate(timestamp)
ORDER BY (org_id, app_id, timestamp)
SETTINGS ttl_only_drop_parts = 1
"#;

/// Columns added after a table first shipped. `CREATE TABLE IF NOT EXISTS` is
/// a no-op on an existing table, so a deployment that created these tables
/// before the platform trace existed needs the columns added in place; each
/// statement is idempotent and runs on every boot after `ALL_DDL`. The
/// `DEFAULT ''` keeps old rows readable and the insert row structs (which
/// name every column) valid on both shapes.
/// Slice-0 invocation meters (2026-09-16): the subrequest count and the
/// platform-setup split. Same idempotent shape as the trace-id alters above and
/// applied the same way, so a deployment whose tables predate them gets the
/// columns in place with old rows reading `0`.
pub const CUSTOM_APP_METER_ALTERS: &[&str] = &[
    "ALTER TABLE custom_app_events ADD COLUMN IF NOT EXISTS host_calls UInt32 DEFAULT 0, ADD COLUMN IF NOT EXISTS init_ms UInt32 DEFAULT 0",
];

pub const CUSTOM_APP_TRACE_ID_ALTERS: &[&str] = &[
    "ALTER TABLE custom_app_events ADD COLUMN IF NOT EXISTS trace_id String DEFAULT '', ADD COLUMN IF NOT EXISTS span_id String DEFAULT ''",
    "ALTER TABLE custom_app_logs ADD COLUMN IF NOT EXISTS trace_id String DEFAULT '', ADD COLUMN IF NOT EXISTS span_id String DEFAULT ''",
    "ALTER TABLE custom_app_client_errors ADD COLUMN IF NOT EXISTS trace_id String DEFAULT '', ADD COLUMN IF NOT EXISTS span_id String DEFAULT ''",
];

pub const ALL_DDL: &[&str] = &[
    CREATE_SPANS_TABLE,
    CREATE_INTENT_CLUSTERS_TABLE,
    CREATE_INTENT_CLASSIFICATIONS_TABLE,
    CREATE_METRIC_USAGE_TABLE,
    CREATE_EXECUTIONS_TABLE,
    CREATE_CUSTOM_APP_EVENTS_TABLE,
    CREATE_CUSTOM_APP_LOGS_TABLE,
    CREATE_CUSTOM_APP_CLIENT_ERRORS_TABLE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executions_select_flattens_tool_call_spans_only() {
        // The rollup reads from spans and picks only tool_call executions that
        // carry a denormalized agent ref (the MV cannot join to the run span).
        assert!(EXECUTIONS_SELECT.contains("FROM observability_spans"));
        assert!(EXECUTIONS_SELECT.contains("'oxy.span_type') = 'tool_call'"));
        assert!(EXECUTIONS_SELECT.contains("'oxy.agent.ref') != ''"));
    }

    #[test]
    fn executions_select_is_one_row_per_span() {
        // Scalar extraction only — an `arrayJoin` would fan a single span into
        // many rows and silently multiply every aggregate.
        assert!(
            !EXECUTIONS_SELECT.contains("arrayJoin"),
            "arrayJoin fans one span into many rollup rows"
        );
        assert!(EXECUTIONS_SELECT.contains("arrayFirst"));
    }

    #[test]
    fn executions_table_and_select_agree_on_columns() {
        // ClickHouse inserts MV rows positionally, so every rollup column must
        // be produced by the shared SELECT. Guards against silent column drift.
        for col in [
            "agent_ref",
            "user_question",
            "execution_type",
            "is_verified",
            "is_success",
            "error_message",
            "source_type",
            "source_ref",
            "generated_sql",
            "tool_input",
            "tool_output",
        ] {
            assert!(CREATE_EXECUTIONS_TABLE.contains(col), "table missing {col}");
            assert!(
                EXECUTIONS_SELECT.contains(&format!("AS {col}")),
                "select missing alias {col}"
            );
        }
    }

    #[test]
    fn executions_mv_prefix_targets_the_rollup_table() {
        assert!(CREATE_EXECUTIONS_MV_PREFIX.contains("MATERIALIZED VIEW"));
        assert!(CREATE_EXECUTIONS_MV_PREFIX.contains("TO observability_executions"));
    }
}

#[cfg(test)]
mod partitioning_tests {
    use super::*;

    /// Regression guard for the 2026-09-03 oxy-dev wedge. Monthly partitioning
    /// let `observability_spans` accumulate parts of millions of rows; merging
    /// them re-read enough to fill the container cgroup's page cache, which
    /// squeezed the memory tracker, which failed the merge, which retried —
    /// a loop the failing merges sustained themselves. 121-part backlog, ~54%
    /// merge failure, against 0 failures on the day-partitioned log store
    /// beside it.
    #[test]
    fn event_tables_partition_by_day_not_month() {
        for (name, ddl) in [
            ("spans", CREATE_SPANS_TABLE),
            ("metric_usage", CREATE_METRIC_USAGE_TABLE),
            ("executions", CREATE_EXECUTIONS_TABLE),
            ("custom_app_events", CREATE_CUSTOM_APP_EVENTS_TABLE),
            ("custom_app_logs", CREATE_CUSTOM_APP_LOGS_TABLE),
            (
                "custom_app_client_errors",
                CREATE_CUSTOM_APP_CLIENT_ERRORS_TABLE,
            ),
        ] {
            assert!(
                ddl.contains("PARTITION BY toDate("),
                "{name} must partition by day"
            );
            assert!(
                !ddl.contains("toYYYYMM("),
                "{name} is back on monthly partitioning; see the module docs"
            );
        }
    }

    /// `ttl_only_drop_parts` is what makes the 90-day TTL cheap: expiry drops
    /// whole days instead of rewriting every part to strip expired rows. It is
    /// only sound alongside a partition key, which is why it rides with the
    /// three tables above and not with the unpartitioned one.
    #[test]
    fn partitioned_tables_drop_parts_rather_than_rewriting() {
        for (name, ddl) in [
            ("spans", CREATE_SPANS_TABLE),
            ("metric_usage", CREATE_METRIC_USAGE_TABLE),
            ("executions", CREATE_EXECUTIONS_TABLE),
            ("custom_app_events", CREATE_CUSTOM_APP_EVENTS_TABLE),
            ("custom_app_logs", CREATE_CUSTOM_APP_LOGS_TABLE),
            (
                "custom_app_client_errors",
                CREATE_CUSTOM_APP_CLIENT_ERRORS_TABLE,
            ),
        ] {
            assert!(
                ddl.contains("ttl_only_drop_parts = 1"),
                "{name} should expire by dropping parts"
            );
        }
    }

    /// The custom-app tables are read "this app, this window" on every path, so
    /// an org/app-first sort key is the difference between touching one tenant's
    /// parts and scanning every tenant's. Same rule the customer-apps caches
    /// follow, for the same reason.
    #[test]
    fn custom_app_tables_sort_app_first() {
        for (name, ddl) in [
            ("custom_app_events", CREATE_CUSTOM_APP_EVENTS_TABLE),
            ("custom_app_logs", CREATE_CUSTOM_APP_LOGS_TABLE),
            (
                "custom_app_client_errors",
                CREATE_CUSTOM_APP_CLIENT_ERRORS_TABLE,
            ),
        ] {
            assert!(
                ddl.contains("ORDER BY (org_id, app_id, timestamp)"),
                "{name} must sort org/app first, not timestamp first"
            );
        }
    }

    /// Intent classifications MUST stay unpartitioned. It is a
    /// `ReplacingMergeTree` keyed `ORDER BY (trace_id, question)` whose version
    /// column is `classified_at`, and dedup happens WITHIN a partition — so
    /// partitioning by date would file a re-classification of the same question
    /// in a different partition from the original and the two would never
    /// collapse. Executions is safe only because its partition key derives from
    /// `timestamp`, which is already part of its own `ORDER BY`.
    #[test]
    fn intent_classifications_stays_unpartitioned_for_dedup() {
        assert!(
            !CREATE_INTENT_CLASSIFICATIONS_TABLE.contains("PARTITION BY"),
            "partitioning this table breaks ReplacingMergeTree dedup across days"
        );
        assert!(
            CREATE_EXECUTIONS_TABLE
                .contains("ORDER BY (execution_type, agent_ref, timestamp, span_id)"),
            "executions may only be partitioned while timestamp remains in its ORDER BY"
        );
    }
}
