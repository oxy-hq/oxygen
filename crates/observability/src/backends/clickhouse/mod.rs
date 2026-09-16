//! ClickHouse-backed implementation of [`crate::store::ObservabilityStore`].
//!
//! Uses the `clickhouse` crate's HTTP API to execute SQL against the
//! `observability_*` tables.

mod custom_apps;
mod execution_analytics;
mod intents;
mod latency_cost;
mod metrics;
pub mod schema;
mod traces;

use std::collections::HashMap;

use async_trait::async_trait;
use clickhouse::{Client, Row};
use oxy_shared::errors::OxyError;
use serde::Deserialize;

/// Single-column `count()` result used by the rollup backfill guard.
#[derive(Debug, Deserialize, Row)]
struct RowCount {
    c: u64,
}

/// Render a stored instant column as an unambiguous ISO-8601 / RFC3339 **UTC**
/// string, e.g. `2026-07-12T14:30:00.123456Z`.
///
/// The trailing `Z` is a literal (ClickHouse copies non-`%` chars verbatim) and
/// the `'UTC'` timezone arg forces UTC output. Without both, `formatDateTime`
/// emits a zoneless `2026-07-12 14:30:00…` form that the frontend `new Date(…)`
/// parses as *browser-local* time — silently shifting every timestamp, and
/// throwing `RangeError: Invalid time value` on an unparseable one.
///
/// **Minutes MUST use `%i`, not `%M`.** ClickHouse's `formatDateTime` follows
/// MySQL syntax: `%M` is the *full month name* and `%i` is the minute. With
/// `%M` the helper renders `2026-07-12T14:July:00.123456Z`, which `new Date(…)`
/// parses as `NaN`. Every span's `offsetMs` then becomes `NaN`, the frontend's
/// `max(offsetMs + durationMs)` trace-total falls back to `0`, and every
/// waterfall bar collapses to its 3px `minWidth` pinned at `left:0` instead of
/// filling the timeline — this was the ClickHouse-vs-Airhouse "spans don't fit
/// the trace" bug (Airhouse casts the timestamp to text directly, so it was
/// never affected). `%f` renders the sub-second part as 6 digits (microseconds)
/// regardless of the column's DateTime64 scale; that's fine — `new Date(…)`
/// truncates it to milliseconds.
///
/// Kept in one place so every serving query renders timestamps identically.
pub(super) fn iso_utc(col: &str) -> String {
    format!("formatDateTime({col}, '%Y-%m-%dT%H:%i:%S.%fZ', 'UTC')")
}

/// Hard client-side ceiling for a single serving query. Set slightly above the
/// server-side `max_execution_time` (see [`ClickHouseObservabilityStorage::read_client`])
/// so a *reachable* ClickHouse aborts the query itself first; this only fires as
/// a backstop when ClickHouse is unreachable and the HTTP call would otherwise
/// hang forever — the default `clickhouse` client sets no timeout, so an
/// unbounded await would pin the request task and eventually 503 the instance.
const SERVING_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35);

/// Run a serving query future under [`SERVING_QUERY_TIMEOUT`], mapping an
/// elapsed timeout to a clean error instead of a hung task. `what` names the
/// query for the error message (e.g. `"trace detail"`).
pub(super) async fn with_query_timeout<T, F>(what: &str, fut: F) -> Result<T, OxyError>
where
    F: std::future::Future<Output = Result<T, OxyError>>,
{
    match tokio::time::timeout(SERVING_QUERY_TIMEOUT, fut).await {
        Ok(res) => res,
        Err(_) => Err(OxyError::RuntimeError(format!(
            "ClickHouse {what} query exceeded {}s",
            SERVING_QUERY_TIMEOUT.as_secs()
        ))),
    }
}

#[cfg(test)]
mod iso_utc_tests {
    use super::iso_utc;

    /// Regression guard for the "Invalid time value" trace crash: serving
    /// timestamps must render as ISO-8601 **UTC** (`…Z`, forced `'UTC'` tz) so
    /// the frontend `new Date(…)` parses them as UTC rather than browser-local.
    #[test]
    fn renders_iso_8601_utc_with_zulu_suffix() {
        assert_eq!(
            iso_utc("timestamp"),
            "formatDateTime(timestamp, '%Y-%m-%dT%H:%i:%S.%fZ', 'UTC')"
        );
    }

    /// Regression guard for the "spans don't fit the waterfall" bug: minutes
    /// MUST use `%i`. ClickHouse's `%M` is the full month name (e.g. `July`),
    /// which yields an unparseable `…T14:July:00…Z` timestamp → `NaN` offsets →
    /// a zero trace-total → collapsed waterfall bars. Verified against
    /// ClickHouse 25.12: `%M` emits the month name, `%i` emits the minute.
    #[test]
    fn uses_minute_specifier_not_month_name() {
        let sql = iso_utc("timestamp");
        assert!(sql.contains("%H:%i:%S"), "minutes must use %i, got: {sql}");
        assert!(
            !sql.contains("%M"),
            "%M is the month name in ClickHouse's formatDateTime, not minutes: {sql}"
        );
    }
}

/// (table, ttl column, has a partition key)
///
/// PARTITIONED_FOR_TTL_DROP: the third field gates
/// `ttl_only_drop_parts`, which is only correct on a partitioned table.
/// `observability_intent_classifications` is `ReplacingMergeTree`
/// ORDER BY (trace_id, question) with NO `PARTITION BY` (schema.rs), so
/// its whole contents live in the single `all` partition and merges
/// freely combine old parts with new. Under `ttl_only_drop_parts` a part
/// is removed only once EVERY row in it has expired — and a part that
/// keeps absorbing fresh rows never gets there. Setting it on that table
/// would silently convert "TTL rewrites parts" into "TTL deletes
/// nothing": the ALTER succeeds, no error, no log. It is also the table
/// least at risk from the rewrite path, being small and unpartitioned.
///
/// The fourth field is a per-table retention **override** in days. `None` takes
/// the process-wide `RETENTION_DAYS`. It exists for one reason: `custom_app_logs`
/// holds `ctx.log()` output, which can carry application data that a usage count
/// cannot — a function that interpolates a row value into a log line puts that
/// value here. Shorter retention is the posture that pays for capturing it at
/// all, so it is a property of the table rather than an operator setting that
/// could drift back to 90 days without anyone deciding to.
const RETENTION_TABLES: &[(&str, &str, bool, Option<u32>)] = &[
    ("observability_spans", "timestamp", true, None),
    ("observability_executions", "timestamp", true, None),
    (
        "observability_intent_classifications",
        "classified_at",
        false,
        None,
    ),
    ("observability_metric_usage", "created_at", true, None),
    ("custom_app_events", "timestamp", true, None),
    ("custom_app_logs", "timestamp", true, Some(30)),
    // Same 30 days as the logs, for the same reason: free text a page threw can
    // carry application data that a usage count cannot.
    ("custom_app_client_errors", "timestamp", true, Some(30)),
];

/// Build the retention DDL for one observability table. Pure so the
/// `toDateTime` wrapper can be regression-tested without a live ClickHouse.
///
/// `retention_days == 0` removes any existing TTL.
fn retention_ttl_sql(table: &str, column: &str, retention_days: u32) -> String {
    if retention_days == 0 {
        format!("ALTER TABLE {table} REMOVE TTL")
    } else {
        // `toDateTime(...)` is NOT optional. Every timestamp column on these
        // tables is `DateTime64` (spans and executions are `DateTime64(9)`,
        // intent classifications and metric usage `DateTime64(3)`), and
        // ClickHouse rejects a TTL expression that resolves to `DateTime64`:
        //
        //   Code: 450. TTL expression result column should have DateTime or
        //   Date type, but has DateTime64(9). (BAD_TTL_EXPRESSION)
        //
        // Without the cast this ALTER failed on the FIRST table in the list and
        // returned early, so none of the four ever received a TTL and
        // observability data grew without bound.
        format!(
            "ALTER TABLE {table} MODIFY TTL toDateTime({column}) + INTERVAL {retention_days} DAY DELETE"
        )
    }
}

#[cfg(test)]
mod retention_ttl_tests {
    use super::retention_ttl_sql;

    /// Regression guard for the retention feature never having worked. Every
    /// timestamp column on these tables is `DateTime64`, which ClickHouse
    /// refuses as a TTL expression (`Code: 450 BAD_TTL_EXPRESSION`). The ALTER
    /// therefore failed on the first table and returned early, so no
    /// observability table ever received a TTL.
    ///
    /// Observed in oxy-dev on 2026-09-03: `observability_spans` held 54 days
    /// and 12.9M rows with no TTL clause at all, and its 510 MiB parts could no
    /// longer be merged inside the server's 3.2 GiB memory cap.
    #[test]
    fn casts_datetime64_columns_to_datetime() {
        let sql = retention_ttl_sql("observability_spans", "timestamp", 90);
        assert!(
            sql.contains("toDateTime(timestamp)"),
            "TTL column must be cast out of DateTime64, got: {sql}"
        );
        assert!(
            !sql.contains("TTL timestamp +"),
            "bare DateTime64 column is rejected by ClickHouse: {sql}"
        );
    }

    /// The interval and DELETE action must survive the cast.
    #[test]
    fn keeps_interval_and_delete_action() {
        assert_eq!(
            retention_ttl_sql("observability_metric_usage", "created_at", 30),
            "ALTER TABLE observability_metric_usage MODIFY TTL toDateTime(created_at) + INTERVAL 30 DAY DELETE"
        );
    }

    /// `ttl_only_drop_parts` is only sound on a partitioned table: it removes a
    /// part only once EVERY row in it has expired, and on an unpartitioned
    /// table merges keep folding fresh rows into the same part, so that never
    /// happens and the TTL silently deletes nothing.
    ///
    /// This pins the third field of `RETENTION_TABLES` to the actual DDL rather
    /// than to a comment, so adding `PARTITION BY` to a table — or adding a new
    /// unpartitioned one to the list — fails here instead of silently
    /// disabling its retention in production.
    #[test]
    fn partition_flag_matches_the_schema_ddl() {
        for (table, _, partitioned, _) in super::RETENTION_TABLES {
            let ddl = crate::backends::clickhouse::schema::ALL_DDL
                .iter()
                .find(|d| d.contains(&format!("CREATE TABLE IF NOT EXISTS {table} (")))
                .unwrap_or_else(|| panic!("no DDL in ALL_DDL for {table}"));
            assert_eq!(
                ddl.contains("PARTITION BY"),
                *partitioned,
                "{table}: RETENTION_TABLES says partitioned={partitioned}, DDL disagrees"
            );
        }
    }

    /// Zero means "no retention": remove the TTL rather than setting a
    /// zero-day one, which would delete everything on the next merge.
    #[test]
    fn zero_days_removes_ttl_rather_than_expiring_everything() {
        let sql = retention_ttl_sql("observability_spans", "timestamp", 0);
        assert_eq!(sql, "ALTER TABLE observability_spans REMOVE TTL");
        assert!(!sql.contains("INTERVAL 0 DAY"), "must not set a 0-day TTL");
    }
}

use crate::intent_types::IntentCluster;
use crate::store::ObservabilityStore;
use crate::types::{
    AgentExecutionStatsData, AppAvailabilityWindow, ClientErrorGroup, ClusterInfoRow,
    ClusterMapDataRow, CustomAppClientErrorRecord, CustomAppEventRecord, CustomAppLogRecord,
    ExecutionListData, ExecutionSummaryData, ExecutionTimeBucketData, FunctionLogRow,
    IntentAnalyticsRow, LatencyHistogramData, LatencyPercentilesData, MetricAnalyticsData,
    MetricDetailData, MetricUsageRecord, MetricsListData, ModelUsageData, SpanRecord,
    TraceDetailRow, TraceEnrichmentRow, TraceRow,
};

pub struct ClickHouseObservabilityStorage {
    client: Client,
    /// Target database (from `OXY_CLICKHOUSE_DATABASE`). Kept so `ensure_schema`
    /// can `CREATE DATABASE` it before the unqualified table DDL runs.
    database: String,
}

impl std::fmt::Debug for ClickHouseObservabilityStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClickHouseObservabilityStorage")
            .finish_non_exhaustive()
    }
}

impl ClickHouseObservabilityStorage {
    /// Construct from explicit ClickHouse connection parameters.
    pub fn new(url: &str, user: &str, password: &str, database: &str) -> Result<Self, OxyError> {
        let client = Client::default()
            .with_url(url)
            .with_user(user)
            .with_password(password)
            .with_database(database);
        Ok(Self {
            client,
            database: database.to_string(),
        })
    }

    /// Construct from standard `OXY_CLICKHOUSE_*` environment variables.
    pub async fn from_env() -> Result<Self, OxyError> {
        let url = std::env::var("OXY_CLICKHOUSE_URL")
            .unwrap_or_else(|_| "http://localhost:8123".to_string());
        let user = std::env::var("OXY_CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_string());
        let password = std::env::var("OXY_CLICKHOUSE_PASSWORD").unwrap_or_default();
        let database = std::env::var("OXY_CLICKHOUSE_DATABASE")
            .unwrap_or_else(|_| "observability".to_string());
        Self::new(&url, &user, &password, &database)
    }

    /// The cheapest round trip that proves the server answers queries:
    /// `SELECT 1` against the always-present `default` database (the configured
    /// one may not exist yet). Carries no timeout of its own — like every call on
    /// the default client — so callers must bound it.
    pub async fn ping(&self) -> Result<(), OxyError> {
        self.client
            .clone()
            .with_database("default")
            .query("SELECT 1")
            .execute()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("ClickHouse ping failed: {e}")))
    }

    /// Accessor for the underlying ClickHouse client.
    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    /// A client clone carrying server-side guards for user-facing **read**
    /// queries: it caps wall-clock (`max_execution_time`) and result size, and
    /// makes an oversized result an error rather than an unbounded stream. A
    /// slow or pathological query then fails fast as a clean error instead of
    /// pinning a request task and starving the instance until the load balancer
    /// 503s it. Intentionally not used for DDL/backfill, which legitimately run
    /// longer than a serving request should.
    pub(super) fn read_client(&self) -> Client {
        self.client
            .clone()
            .with_option("max_execution_time", "30")
            .with_option("max_result_rows", "1000000")
            .with_option("max_result_bytes", "268435456") // 256 MiB
            .with_option("result_overflow_mode", "throw")
    }

    /// Ensure all observability tables exist.
    ///
    /// Safe to call on every startup; uses `CREATE TABLE IF NOT EXISTS` DDL.
    pub async fn ensure_schema(&self) -> Result<(), OxyError> {
        // The unqualified `CREATE TABLE` DDL below targets the connection's
        // database, which must already exist. A CHI / managed ClickHouse (unlike
        // the local docker image's `CLICKHOUSE_DB`) does not pre-create it, so
        // create it here via a client scoped to the always-present `default` db.
        // `database` is operator config (backtick-quoted defensively).
        self.client
            .clone()
            .with_database("default")
            .query(&format!(
                "CREATE DATABASE IF NOT EXISTS `{}`",
                self.database
            ))
            .execute()
            .await
            .map_err(|e| {
                OxyError::RuntimeError(format!("ClickHouse CREATE DATABASE failed: {e}"))
            })?;

        for ddl in schema::ALL_DDL {
            self.client.query(ddl).execute().await.map_err(|e| {
                OxyError::RuntimeError(format!("ClickHouse schema DDL failed: {e}"))
            })?;
        }
        // Additive columns for tables that predate them — see the constant.
        // Not fatal: a ClickHouse user with CREATE but not ALTER would otherwise
        // lose the whole store over two optional columns. The custom-app inserts
        // name every column and fail loudly on their own until the columns
        // exist; product traces are unaffected either way.
        for alter in schema::CUSTOM_APP_TRACE_ID_ALTERS
            .iter()
            .chain(schema::CUSTOM_APP_METER_ALTERS)
        {
            if let Err(e) = self.client.query(alter).execute().await {
                tracing::warn!(error = %e, alter, "ClickHouse schema ALTER skipped");
            }
        }

        // The execution rollup MV shares its flatten logic with the backfill via
        // `EXECUTIONS_SELECT`, so it's assembled here rather than sitting in
        // `ALL_DDL` as a frozen string.
        let mv = format!(
            "{} {}",
            schema::CREATE_EXECUTIONS_MV_PREFIX,
            schema::EXECUTIONS_SELECT
        );
        self.client.query(&mv).execute().await.map_err(|e| {
            OxyError::RuntimeError(format!("ClickHouse executions MV DDL failed: {e}"))
        })?;

        self.backfill_executions_history().await?;
        Ok(())
    }

    /// Seed `observability_executions` from spans already on disk.
    ///
    /// The materialized view only captures spans inserted *after* it exists, so
    /// on the first deploy the Execution Analytics panels would read an empty
    /// rollup until 90 days of fresh traffic accrued. This one-shot backfill
    /// flattens the existing retention window using the same `EXECUTIONS_SELECT`
    /// the MV uses (so rows are identical).
    ///
    /// The guard is deliberately *not* `count() == 0`: the MV is already live by
    /// the time we reach here, so a single span inserted between MV creation and
    /// the check would flip an empty-table guard and silently skip the whole
    /// historical seed — forever, since the guard would then stay non-zero. We
    /// instead ask whether any rollup row is older than an hour: a fresh MV row
    /// carries a ~now() timestamp so it can't trip this guard, but a prior
    /// backfill (or an hour of live traffic) does. A concurrent insert during
    /// the backfill window is deduped by `ReplacingMergeTree` on the `span_id`
    /// key (and reads use `FINAL`).
    async fn backfill_executions_history(&self) -> Result<(), OxyError> {
        let seeded = self
            .client
            .query(
                "SELECT count() AS c FROM observability_executions \
                 WHERE timestamp < now() - INTERVAL 1 HOUR",
            )
            .fetch_one::<RowCount>()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("executions rollup count failed: {e}")))?;
        if seeded.c > 0 {
            return Ok(());
        }

        let backfill = format!(
            "INSERT INTO observability_executions {} AND timestamp >= now() - INTERVAL {} DAY",
            schema::EXECUTIONS_SELECT,
            crate::RETENTION_DAYS
        );
        self.client.query(&backfill).execute().await.map_err(|e| {
            OxyError::RuntimeError(format!("executions rollup backfill failed: {e}"))
        })?;
        tracing::info!("observability: backfilled execution rollup from existing spans");
        Ok(())
    }

    /// Apply or remove TTL on event tables so ClickHouse's background merge
    /// expires old rows automatically. `retention_days = 0` removes any
    /// existing TTL ("REMOVE TTL"); non-zero sets
    /// `TTL toDateTime(<column>) + INTERVAL N DAY DELETE`. Intent clusters
    /// never get a TTL because they're aggregated labels, not event data.
    ///
    /// Called on every pod boot, so it is written to be cheap and idempotent.
    /// `materialize_ttl_after_modify = 0` keeps the ALTER metadata-only rather
    /// than queuing a full-table rewrite each time.
    ///
    /// `ttl_only_drop_parts = 1` makes the eventual expiry drop whole parts
    /// instead of rewriting them — but ONLY on the three tables that have a
    /// partition key. `observability_intent_classifications` is deliberately
    /// excluded, for the reason recorded at `RETENTION_TABLES` above. The
    /// inline comments below cover why each setting matters.
    pub async fn apply_retention_ttl(&self, retention_days: u32) -> Result<(), OxyError> {
        for (table, column, partitioned, override_days) in RETENTION_TABLES {
            // A per-table override never *extends* retention past the
            // process-wide setting: if an operator dials the global down to 7
            // days, the log table must not keep 30. Min, not "override wins".
            // `retention_days == 0` means "remove the TTL" and is not a
            // ceiling — it stays 0 so the removal still happens.
            let retention_days = match (retention_days, override_days) {
                (0, _) => 0,
                (global, Some(days)) => global.min(*days),
                (global, None) => global,
            };
            // Expire by dropping whole parts rather than rewriting each one to
            // strip expired rows. The three partitioned tables partition
            // monthly (`toYYYYMM(...)`) against a 90-day TTL, so a part becomes
            // wholly expired well within a partition's lifetime and can simply
            // be dropped.
            //
            // The default (0) rewrites every part containing at least one
            // expired row, which is precisely the operation already OOMing on
            // oxy-dev: 510 MiB parts against a 3.2 GiB server cap. Since this
            // change is what makes expiry start happening at all, shipping the
            // cheap expiry path with it rather than after the first incident.
            //
            // Trade-off, deliberate: a part holding a mix of expired and live
            // rows is left alone until all of it expires, so retention is
            // coarser than exactly 90 days. That is the same choice
            // ClickHouse's own ClickStack observability schema makes.
            //
            // Skipped when retention is being REMOVED — the setting only
            // describes how a TTL behaves, and there is about to be no TTL.
            if *partitioned && retention_days > 0 {
                // Deliberately NOT fatal. `MODIFY SETTING` needs the
                // ALTER SETTINGS privilege, which is distinct from the
                // ALTER TTL privilege the statement below needs, and managed
                // ClickHouse deployments restrict table settings more often
                // than they restrict TTL. This is an optimisation; the TTL is
                // the fix. Propagating here would reproduce the exact bug this
                // change exists to remove — one statement failing on the first
                // table leaves `observability_spans` with no TTL and the three
                // tables after it never attempted.
                let drop_parts =
                    format!("ALTER TABLE {table} MODIFY SETTING ttl_only_drop_parts = 1");
                if let Err(e) = self.client.query(&drop_parts).execute().await {
                    tracing::warn!(
                        table,
                        error = %e,
                        "ttl_only_drop_parts not applied; TTL will rewrite parts instead of dropping them"
                    );
                }
            }

            // `materialize_ttl_after_modify = 0` is load-bearing. ClickHouse
            // defaults it to 1 and does NOT diff MODIFY TTL against the
            // existing TTL, so every successful ALTER queues a MATERIALIZE TTL
            // mutation that rewrites all existing parts.
            //
            // This runs on EVERY boot of every ide and serve pod
            // (`open_clickhouse_store`). While the statement was failing at
            // parse that was harmless; now that it succeeds, each deploy,
            // scale event or crashloop would queue a fresh full-table rewrite
            // of a 12.9M-row table on a server already at its memory cap — and
            // buy nothing, since nothing is old enough to expire yet.
            //
            // Metadata-only is all this needs to be: the TTL still takes
            // effect on parts as they merge from here on.
            self.client
                .clone()
                .with_setting("materialize_ttl_after_modify", "0")
                .query(&retention_ttl_sql(table, column, retention_days))
                .execute()
                .await
                .map_err(|e| {
                    OxyError::RuntimeError(format!("ClickHouse TTL update on {table} failed: {e}"))
                })?;
        }
        Ok(())
    }
}

#[async_trait]
impl ObservabilityStore for ClickHouseObservabilityStorage {
    async fn list_traces(
        &self,
        limit: i64,
        offset: i64,
        agent_ref: Option<&str>,
        status: Option<&str>,
        duration_filter: Option<&str>,
    ) -> Result<(Vec<TraceRow>, i64), OxyError> {
        traces::list_traces(
            self,
            limit,
            offset,
            agent_ref,
            status,
            duration_filter,
            None,
            None,
            None,
        )
        .await
    }

    async fn search_traces(
        &self,
        limit: i64,
        offset: i64,
        agent_ref: Option<&str>,
        status: Option<&str>,
        duration_filter: Option<&str>,
        search: Option<&str>,
        from_ts: Option<i64>,
        to_ts: Option<i64>,
    ) -> Result<(Vec<TraceRow>, i64), OxyError> {
        traces::list_traces(
            self,
            limit,
            offset,
            agent_ref,
            status,
            duration_filter,
            search,
            from_ts,
            to_ts,
        )
        .await
    }

    async fn get_trace_detail(&self, trace_id: &str) -> Result<Vec<TraceDetailRow>, OxyError> {
        traces::get_trace_detail(self, trace_id).await
    }

    async fn get_cluster_map_data(
        &self,
        days: u32,
        limit: usize,
        source: Option<&str>,
    ) -> Result<Vec<ClusterMapDataRow>, OxyError> {
        with_query_timeout(
            "get_cluster_map_data",
            traces::get_cluster_map_data(self, days, limit, source),
        )
        .await
    }

    async fn get_cluster_infos(&self) -> Result<Vec<ClusterInfoRow>, OxyError> {
        with_query_timeout("get_cluster_infos", traces::get_cluster_infos(self)).await
    }

    async fn get_trace_enrichments(
        &self,
        trace_ids: &[String],
    ) -> Result<Vec<TraceEnrichmentRow>, OxyError> {
        with_query_timeout(
            "get_trace_enrichments",
            traces::get_trace_enrichments(self, trace_ids),
        )
        .await
    }

    async fn fetch_unprocessed_questions(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>, OxyError> {
        intents::fetch_unprocessed_questions(self, limit).await
    }

    async fn load_embeddings(
        &self,
    ) -> Result<Vec<(String, String, Vec<f32>, String, String)>, OxyError> {
        intents::load_embeddings(self).await
    }

    async fn store_clusters(&self, clusters: &[IntentCluster]) -> Result<(), OxyError> {
        intents::store_clusters(self, clusters).await
    }

    async fn load_clusters(&self) -> Result<Vec<IntentCluster>, OxyError> {
        intents::load_clusters(self).await
    }

    async fn store_classification(
        &self,
        trace_id: &str,
        question: &str,
        cluster_id: u32,
        intent_name: &str,
        confidence: f32,
        embedding: &[f32],
        source_type: &str,
        source: &str,
    ) -> Result<(), OxyError> {
        intents::store_classification(
            self,
            trace_id,
            question,
            cluster_id,
            intent_name,
            confidence,
            embedding,
            source_type,
            source,
        )
        .await
    }

    async fn update_classification(
        &self,
        trace_id: &str,
        question: &str,
        cluster_id: u32,
        intent_name: &str,
        confidence: f32,
        embedding: &[f32],
        source_type: &str,
        source: &str,
    ) -> Result<(), OxyError> {
        // ReplacingMergeTree ordered by (trace_id, question) handles upsert-
        // style semantics; reuse the store path.
        intents::store_classification(
            self,
            trace_id,
            question,
            cluster_id,
            intent_name,
            confidence,
            embedding,
            source_type,
            source,
        )
        .await
    }

    async fn get_intent_analytics(&self, days: u32) -> Result<Vec<IntentAnalyticsRow>, OxyError> {
        with_query_timeout(
            "get_intent_analytics",
            intents::get_intent_analytics(self, days),
        )
        .await
    }

    async fn get_outliers(&self, limit: usize) -> Result<Vec<(String, String)>, OxyError> {
        with_query_timeout("get_outliers", intents::get_outliers(self, limit)).await
    }

    async fn load_unknown_classifications(
        &self,
    ) -> Result<Vec<(String, String, Vec<f32>, String)>, OxyError> {
        intents::load_unknown_classifications(self).await
    }

    async fn get_unknown_count(&self) -> Result<usize, OxyError> {
        intents::get_unknown_count(self).await
    }

    async fn update_cluster_record(&self, cluster: &IntentCluster) -> Result<(), OxyError> {
        intents::update_cluster_record(self, cluster).await
    }

    async fn get_next_cluster_id(&self) -> Result<u32, OxyError> {
        intents::get_next_cluster_id(self).await
    }

    async fn store_metric_usages(&self, metrics: Vec<MetricUsageRecord>) -> Result<(), OxyError> {
        metrics::store_metric_usages(self, metrics).await
    }

    async fn get_metrics_analytics(&self, days: u32) -> Result<MetricAnalyticsData, OxyError> {
        with_query_timeout(
            "get_metrics_analytics",
            metrics::get_metrics_analytics(self, days),
        )
        .await
    }

    async fn get_metrics_list(
        &self,
        days: u32,
        limit: usize,
        offset: usize,
    ) -> Result<MetricsListData, OxyError> {
        with_query_timeout(
            "get_metrics_list",
            metrics::get_metrics_list(self, days, limit, offset),
        )
        .await
    }

    async fn get_metric_detail(
        &self,
        metric_name: &str,
        days: u32,
    ) -> Result<MetricDetailData, OxyError> {
        with_query_timeout(
            "get_metric_detail",
            metrics::get_metric_detail(self, metric_name, days),
        )
        .await
    }

    async fn get_execution_summary(&self, days: u32) -> Result<ExecutionSummaryData, OxyError> {
        with_query_timeout(
            "get_execution_summary",
            execution_analytics::get_execution_summary(self, days),
        )
        .await
    }

    async fn get_execution_time_series(
        &self,
        days: u32,
    ) -> Result<Vec<ExecutionTimeBucketData>, OxyError> {
        with_query_timeout(
            "get_execution_time_series",
            execution_analytics::get_execution_time_series(self, days),
        )
        .await
    }

    async fn get_execution_agent_stats(
        &self,
        days: u32,
        limit: usize,
    ) -> Result<Vec<AgentExecutionStatsData>, OxyError> {
        with_query_timeout(
            "get_execution_agent_stats",
            execution_analytics::get_execution_agent_stats(self, days, limit),
        )
        .await
    }

    async fn get_execution_list(
        &self,
        days: u32,
        limit: usize,
        offset: usize,
        execution_type: Option<&str>,
        is_verified: Option<bool>,
        source_ref: Option<&str>,
        status: Option<&str>,
    ) -> Result<ExecutionListData, OxyError> {
        with_query_timeout(
            "get_execution_list",
            execution_analytics::get_execution_list(
                self,
                days,
                limit,
                offset,
                execution_type,
                is_verified,
                source_ref,
                status,
            ),
        )
        .await
    }

    async fn get_latency_percentiles(&self, days: u32) -> Result<LatencyPercentilesData, OxyError> {
        with_query_timeout(
            "get_latency_percentiles",
            latency_cost::get_latency_percentiles(self, days),
        )
        .await
    }

    async fn get_latency_histogram(&self, days: u32) -> Result<LatencyHistogramData, OxyError> {
        with_query_timeout(
            "get_latency_histogram",
            latency_cost::get_latency_histogram(self, days),
        )
        .await
    }

    async fn get_model_usage(&self, days: u32) -> Result<Vec<ModelUsageData>, OxyError> {
        with_query_timeout("get_model_usage", latency_cost::get_model_usage(self, days)).await
    }

    async fn insert_spans(&self, spans: Vec<SpanRecord>) -> Result<(), OxyError> {
        traces::insert_spans(self, spans).await
    }

    async fn insert_custom_app_events(
        &self,
        events: Vec<CustomAppEventRecord>,
    ) -> Result<(), OxyError> {
        custom_apps::insert_custom_app_events(self, events).await
    }

    async fn insert_custom_app_logs(&self, logs: Vec<CustomAppLogRecord>) -> Result<(), OxyError> {
        custom_apps::insert_custom_app_logs(self, logs).await
    }

    async fn insert_custom_app_client_errors(
        &self,
        errors: Vec<CustomAppClientErrorRecord>,
    ) -> Result<(), OxyError> {
        custom_apps::insert_custom_app_client_errors(self, errors).await
    }

    async fn get_client_errors(
        &self,
        org_id: &str,
        app_id: &str,
        hours: u32,
        limit: u32,
        build_id: &str,
    ) -> Result<Vec<ClientErrorGroup>, OxyError> {
        with_query_timeout(
            "get_client_errors",
            custom_apps::get_client_errors(self, org_id, app_id, hours, limit, build_id),
        )
        .await
    }

    async fn get_function_logs(
        &self,
        org_id: &str,
        app_id: &str,
        hours: u32,
        limit: u32,
        invocation_id: &str,
        request_id: &str,
    ) -> Result<Vec<FunctionLogRow>, OxyError> {
        with_query_timeout(
            "get_function_logs",
            custom_apps::get_function_logs(
                self,
                org_id,
                app_id,
                hours,
                limit,
                invocation_id,
                request_id,
            ),
        )
        .await
    }

    async fn get_app_availability(
        &self,
        org_id: &str,
        app_id: &str,
        windows_minutes: &[u32],
    ) -> Result<Vec<AppAvailabilityWindow>, OxyError> {
        // Bounded like every other serving read: the default clickhouse client
        // sets no timeout, and this one now runs on the workspace-health eval
        // pass, where an unbounded await would pin that task rather than a
        // request task.
        with_query_timeout(
            "get_app_availability",
            custom_apps::get_app_availability(self, org_id, app_id, windows_minutes),
        )
        .await
    }

    async fn get_fleet_availability(
        &self,
        apps: &[(String, String)],
        windows_minutes: &[u32],
    ) -> Result<HashMap<String, Vec<AppAvailabilityWindow>>, OxyError> {
        with_query_timeout(
            "get_fleet_availability",
            custom_apps::get_fleet_availability(self, apps, windows_minutes),
        )
        .await
    }

    async fn get_fleet_heartbeat_baseline(
        &self,
        apps: &[(String, String)],
        window_minutes: u32,
        cycle_days: u32,
    ) -> Result<HashMap<String, u64>, OxyError> {
        with_query_timeout(
            "get_fleet_heartbeat_baseline",
            custom_apps::get_fleet_heartbeat_baseline(self, apps, window_minutes, cycle_days),
        )
        .await
    }

    async fn shutdown(&self) {
        // HTTP client has no long-lived resources.
        tracing::debug!("ClickHouseObservabilityStorage shutdown");
    }
}
