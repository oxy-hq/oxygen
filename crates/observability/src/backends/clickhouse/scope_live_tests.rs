//! Tenant scoping against a real ClickHouse.
//!
//! The unit tests around this prove the SQL *names* the workspace and the
//! layer *stamps* it. These prove the two meet: rows written by
//! `insert_spans` for two workspaces, read back through the store, with the
//! server deciding which rows each scope sees.
//!
//! `#[ignore]`d because they need a server. Run them with
//!
//! ```text
//! OXY_TEST_CLICKHOUSE_URL=http://localhost:8123 \
//!   cargo nextest run -p oxy-observability --lib --run-ignored only -E 'test(live_)'
//! ```
//!
//! Each test works in a database of its own and drops it.

use uuid::Uuid;

use super::ClickHouseObservabilityStorage;
use crate::scope::WorkspaceScope;
use crate::store::ObservabilityStore;
use crate::types::SpanRecord;

const ACME: &str = "70787bb2-e11b-5488-b2c3-02e60d5fc7d3";
const GLOBEX: &str = "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d";

fn scope(workspace: &str) -> WorkspaceScope {
    WorkspaceScope::of(Uuid::parse_str(workspace).unwrap())
}

/// A storage on `database`, as one process opens it. Each call is a client of
/// its own — which matters, because the ClickHouse client remembers a table's
/// columns from its first insert, exactly as a running instance does.
fn connect(database: &str) -> ClickHouseObservabilityStorage {
    let url = std::env::var("OXY_TEST_CLICKHOUSE_URL")
        .expect("OXY_TEST_CLICKHOUSE_URL names the ClickHouse to test against");
    let user = std::env::var("OXY_TEST_CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_string());
    let password = std::env::var("OXY_TEST_CLICKHOUSE_PASSWORD").unwrap_or_default();
    ClickHouseObservabilityStorage::new(&url, &user, &password, database).unwrap()
}

/// A storage on a fresh database, with nothing created in it yet.
fn storage_on_fresh_database() -> (ClickHouseObservabilityStorage, String) {
    let database = format!("oxy_scope_test_{}", Uuid::new_v4().simple());
    (connect(&database), database)
}

async fn drop_database(storage: &ClickHouseObservabilityStorage, database: &str) {
    storage
        .client()
        .clone()
        .with_database("default")
        .query(&format!("DROP DATABASE IF EXISTS `{database}`"))
        .execute()
        .await
        .expect("drop the test database");
}

/// A run as the layer stores it: an `analytics.run` root and one `llm.call`
/// child, every row carrying `workspace` (or nothing). The child names a
/// model after the trace, so a read that leaks shows whose it was.
fn run(trace: &str, workspace: &str) -> Vec<SpanRecord> {
    let span =
        |span_id: &str, parent: &str, name: &str, attributes: String, events: String| SpanRecord {
            trace_id: trace.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent.to_string(),
            span_name: name.to_string(),
            service_name: "oxy".to_string(),
            span_attributes: attributes,
            duration_ns: 1_000_000,
            status_code: "OK".to_string(),
            status_message: String::new(),
            event_data: events,
            timestamp: chrono::Utc::now().to_rfc3339(),
            workspace_id: workspace.to_string(),
        };
    vec![
        span(
            &format!("{trace}-root"),
            "",
            "analytics.run",
            format!(r#"{{"agent.prompt":"what did {trace} ask"}}"#),
            "[]".to_string(),
        ),
        span(
            &format!("{trace}-llm"),
            &format!("{trace}-root"),
            "llm.call",
            format!(r#"{{"oxy.span_type":"llm","gen_ai.request.model":"model-of-{trace}"}}"#),
            r#"[{"name":"llm.usage","attributes":{"prompt_tokens":"10","completion_tokens":"5","total_tokens":"15"}}]"#
                .to_string(),
        ),
    ]
}

/// One rollup row for `trace`, as the materialized view would have flattened
/// a tool call — written directly so the test does not depend on the view.
async fn executed(storage: &ClickHouseObservabilityStorage, trace: &str, agent: &str) {
    storage
        .client()
        .query(&format!(
            "INSERT INTO observability_executions \
             (trace_id, span_id, timestamp, agent_ref, user_question, execution_type, \
              is_verified, is_success, duration_ns, generated_sql) VALUES \
             ('{trace}', '{trace}-tool', now64(9), '{agent}', 'what did {trace} ask', \
              'sql_generated', 0, 1, 2000000, 'SELECT secret_of_{agent}')"
        ))
        .execute()
        .await
        .expect("insert a rollup row");
}

fn usage(trace: &str, metric: &str) -> crate::types::MetricUsageRecord {
    crate::types::MetricUsageRecord {
        metric_name: metric.to_string(),
        source_type: "agent".to_string(),
        source_ref: "agents/a".to_string(),
        context: format!("what did {trace} ask"),
        context_types: r#"["Question"]"#.to_string(),
        trace_id: trace.to_string(),
    }
}

async fn listed(storage: &ClickHouseObservabilityStorage, scope: &WorkspaceScope) -> Vec<String> {
    let (rows, total) = storage
        .search_traces(scope, 50, 0, None, None, None, None, None, None)
        .await
        .expect("list traces");
    assert_eq!(
        total as usize,
        rows.len(),
        "the count and the page disagree"
    );
    let mut ids: Vec<String> = rows.into_iter().map(|r| r.trace_id).collect();
    ids.sort();
    ids
}

#[tokio::test]
#[ignore = "needs a live ClickHouse: set OXY_TEST_CLICKHOUSE_URL"]
async fn live_a_workspace_reads_its_own_traces_and_nobody_elses() {
    let (storage, database) = storage_on_fresh_database();
    storage.ensure_schema().await.expect("schema");
    assert!(storage.spans_are_scoped());

    let mut spans = run("acme-1", ACME);
    spans.extend(run("acme-2", ACME));
    spans.extend(run("globex-1", GLOBEX));
    // A run whose root never said whose it was.
    spans.extend(run("unclaimed-1", ""));
    storage.insert_spans(spans).await.expect("insert");
    for (trace, question) in [
        ("acme-1", "acme question"),
        ("globex-1", "globex question"),
        ("unclaimed-1", "unclaimed question"),
    ] {
        storage
            .store_classification(
                trace,
                question,
                1,
                "revenue",
                0.9,
                &[0.1, 0.2],
                "agent",
                "a",
            )
            .await
            .expect("classification");
    }

    let acme = scope(ACME);
    let globex = scope(GLOBEX);
    let stranger = WorkspaceScope::of(Uuid::new_v4());

    // The list.
    assert_eq!(listed(&storage, &acme).await, ["acme-1", "acme-2"]);
    assert_eq!(listed(&storage, &globex).await, ["globex-1"]);
    assert!(listed(&storage, &stranger).await.is_empty());

    // One trace, by id: its own in full, anyone else's not at all.
    let detail = |scope: WorkspaceScope, trace: &'static str| {
        let storage = &storage;
        async move { storage.get_trace_detail(&scope, trace).await.unwrap().len() }
    };
    assert_eq!(detail(acme.clone(), "acme-1").await, 2);
    assert_eq!(detail(acme.clone(), "globex-1").await, 0);
    assert_eq!(detail(globex.clone(), "acme-1").await, 0);
    // Unclaimed is nobody's — not even the nil workspace's.
    assert_eq!(detail(acme.clone(), "unclaimed-1").await, 0);
    assert_eq!(
        detail(WorkspaceScope::of(Uuid::nil()), "unclaimed-1").await,
        0
    );

    // Enrichments drop the ids that are not the caller's.
    let asked: Vec<String> = ["acme-1", "globex-1", "unclaimed-1"]
        .map(String::from)
        .to_vec();
    let enriched: Vec<String> = storage
        .get_trace_enrichments(&acme, &asked)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.trace_id)
        .collect();
    assert_eq!(enriched, ["acme-1"]);

    // The cluster map shows a workspace its own questions only.
    let questions = |scope: WorkspaceScope| {
        let storage = &storage;
        async move {
            let mut questions: Vec<String> = storage
                .get_cluster_map_data(&scope, 30, 100, None)
                .await
                .unwrap()
                .into_iter()
                .map(|p| p.question)
                .collect();
            questions.sort();
            questions
        }
    };
    assert_eq!(questions(acme).await, ["acme question"]);
    assert_eq!(questions(globex).await, ["globex question"]);
    assert!(questions(stranger).await.is_empty());

    drop_database(&storage, &database).await;
}

/// A deployment whose spans table predates the tenant column. The column
/// arriving must hide the history, not expose it, and must never stop capture
/// — not on the instance that adds it, and not on one still running from
/// before.
#[tokio::test]
#[ignore = "needs a live ClickHouse: set OXY_TEST_CLICKHOUSE_URL"]
async fn live_history_from_before_the_column_is_hidden_not_exposed() {
    let (not_upgraded, database) = storage_on_fresh_database();
    let raw = not_upgraded.client().clone().with_database("default");
    raw.query(&format!("CREATE DATABASE `{database}`"))
        .execute()
        .await
        .unwrap();
    // The table as every deployment had it before this change.
    not_upgraded
        .client()
        .query(
            "CREATE TABLE observability_spans (
                trace_id String, span_id String, parent_span_id String DEFAULT '',
                span_name LowCardinality(String), service_name LowCardinality(String) DEFAULT 'oxy',
                span_attributes String DEFAULT '{}', duration_ns Int64 DEFAULT 0,
                status_code LowCardinality(String) DEFAULT 'UNSET', status_message String DEFAULT '',
                event_data String DEFAULT '[]', timestamp DateTime64(9) DEFAULT now64(9)
            ) ENGINE = MergeTree() ORDER BY (trace_id, span_id, timestamp)",
        )
        .execute()
        .await
        .unwrap();

    // An instance that does not know the column exists — the schema step has
    // not run, or its ALTER was refused. Spans are written in the shape the
    // table has, and a trace read is refused rather than answered unscoped.
    assert!(!not_upgraded.spans_are_scoped());
    not_upgraded
        .insert_spans(run("history-1", ACME))
        .await
        .expect("capture survives a table without the column");
    assert!(
        not_upgraded
            .search_traces(&scope(ACME), 50, 0, None, None, None, None, None, None)
            .await
            .is_err(),
        "a read that cannot be scoped must be refused"
    );

    // The next instance to boot adds the column in place and writes with it.
    let upgraded = connect(&database);
    upgraded.ensure_schema().await.expect("schema");
    assert!(upgraded.spans_are_scoped());
    upgraded
        .insert_spans(run("fresh-1", ACME))
        .await
        .expect("insert");

    // The first instance is still running, as in a rolling deploy. The column
    // appearing under it must not break its writes.
    not_upgraded
        .insert_spans(run("straggler-1", ACME))
        .await
        .expect("an instance from before the column keeps capturing");

    // Only what was written with a workspace is anyone's. `history-1` and
    // `straggler-1` named ACME in the record, but the rows were stored without
    // it — so they are hidden from ACME and, the point, from everyone else.
    assert_eq!(listed(&upgraded, &scope(ACME)).await, ["fresh-1"]);
    assert!(listed(&upgraded, &scope(GLOBEX)).await.is_empty());
    for hidden in ["history-1", "straggler-1"] {
        for workspace in [ACME, GLOBEX] {
            assert!(
                upgraded
                    .get_trace_detail(&scope(workspace), hidden)
                    .await
                    .unwrap()
                    .is_empty(),
                "{hidden} is readable by {workspace}"
            );
        }
    }

    drop_database(&upgraded, &database).await;
}

/// The rollup, latency, cost and metric reads. Several of these swallow a
/// failed statement into a default, so each is checked for a value only a
/// working, correctly scoped statement can produce — and for the other
/// workspace's value being absent, not merely for "no error".
#[tokio::test]
#[ignore = "needs a live ClickHouse: set OXY_TEST_CLICKHOUSE_URL"]
async fn live_analytics_and_metrics_are_one_workspaces_own() {
    let (storage, database) = storage_on_fresh_database();
    storage.ensure_schema().await.expect("schema");

    let mut spans = run("acme-1", ACME);
    spans.extend(run("acme-2", ACME));
    spans.extend(run("globex-1", GLOBEX));
    spans.extend(run("unclaimed-1", ""));
    storage.insert_spans(spans).await.expect("insert");

    executed(&storage, "acme-1", "acme-agent").await;
    executed(&storage, "acme-2", "acme-agent").await;
    executed(&storage, "globex-1", "globex-agent").await;
    executed(&storage, "unclaimed-1", "nobodys-agent").await;
    storage
        .store_metric_usages(vec![
            usage("acme-1", "acme.revenue"),
            usage("acme-1", "acme.orders"),
            usage("acme-2", "acme.revenue"),
            usage("globex-1", "globex.margin"),
            usage("unclaimed-1", "nobodys.metric"),
            // Recorded outside any run: no trace to belong to anyone through.
            usage("", "traceless.metric"),
        ])
        .await
        .expect("metric usage");

    let acme = scope(ACME);
    let globex = scope(GLOBEX);

    // Executions.
    assert_eq!(
        storage
            .get_execution_summary(&acme, 30)
            .await
            .unwrap()
            .total_executions,
        2
    );
    assert_eq!(
        storage
            .get_execution_summary(&globex, 30)
            .await
            .unwrap()
            .total_executions,
        1
    );
    let series = storage.get_execution_time_series(&acme, 30).await.unwrap();
    assert_eq!(series.iter().map(|d| d.generated_count).sum::<u64>(), 2);
    let agents: Vec<String> = storage
        .get_execution_agent_stats(&acme, 30, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.agent_ref)
        .collect();
    assert_eq!(agents, ["acme-agent"]);
    let list = storage
        .get_execution_list(&acme, 30, 50, 0, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(list.total, 2);
    assert!(list.executions.iter().all(|e| e.agent_ref == "acme-agent"));
    assert!(
        list.executions
            .iter()
            .all(|e| e.generated_sql == "SELECT secret_of_acme-agent")
    );
    // A filter narrows the workspace's rows; it does not reach past them.
    let filtered = storage
        .get_execution_list(
            &globex,
            30,
            50,
            0,
            Some("sql_generated"),
            Some(false),
            None,
            Some("success"),
        )
        .await
        .unwrap();
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.executions[0].agent_ref, "globex-agent");

    // Latency: the histogram counts this workspace's executions.
    let histogram = storage.get_latency_histogram(&acme, 30).await.unwrap();
    assert_eq!(histogram.buckets.iter().map(|b| b.count).sum::<u64>(), 2);
    assert!(histogram.percentiles.p50_ms > 0.0);
    let percentiles = storage.get_latency_percentiles(&globex, 30).await.unwrap();
    assert_eq!(percentiles.series.len(), 1);
    assert!(percentiles.overall.p50_ms > 0.0);

    // Cost: models this workspace's runs called.
    let mut models: Vec<String> = storage
        .get_model_usage(&acme, 30)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.model)
        .collect();
    models.sort();
    assert_eq!(models, ["model-of-acme-1", "model-of-acme-2"]);

    // Metrics.
    let analytics = storage.get_metrics_analytics(&acme, 30).await.unwrap();
    assert_eq!(analytics.total_queries, 3);
    assert_eq!(analytics.unique_metrics, 2);
    assert_eq!(analytics.most_popular.as_deref(), Some("acme.revenue"));
    assert_eq!(analytics.by_source_type.agent, 3);
    assert_eq!(analytics.by_context_type.question, 3);
    let mut listed_metrics: Vec<String> = storage
        .get_metrics_list(&acme, 30, 50, 0)
        .await
        .unwrap()
        .metrics
        .into_iter()
        .map(|m| m.name)
        .collect();
    listed_metrics.sort();
    assert_eq!(listed_metrics, ["acme.orders", "acme.revenue"]);
    assert_eq!(
        storage
            .get_metrics_list(&globex, 30, 50, 0)
            .await
            .unwrap()
            .total,
        1
    );

    let detail = storage
        .get_metric_detail(&acme, "acme.revenue", 30)
        .await
        .unwrap();
    assert_eq!(detail.total_queries, 2);
    assert_eq!(detail.via_agent, 2);
    assert_eq!(detail.usage_trend.iter().map(|d| d.count).sum::<u64>(), 2);
    assert_eq!(
        detail
            .related_metrics
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["acme.orders"]
    );
    assert_eq!(detail.recent_usage.len(), 2);
    assert!(
        detail
            .recent_usage
            .iter()
            .all(|u| u.trace_id.starts_with("acme-"))
    );

    // Another workspace's metric, asked for by name, is not there to be found.
    let foreign = storage
        .get_metric_detail(&acme, "globex.margin", 30)
        .await
        .unwrap();
    assert_eq!(foreign.total_queries, 0);
    assert!(foreign.recent_usage.is_empty());
    for nobodys in ["nobodys.metric", "traceless.metric"] {
        for workspace in [&acme, &globex] {
            let detail = storage
                .get_metric_detail(workspace, nobodys, 30)
                .await
                .unwrap();
            assert_eq!(detail.total_queries, 0, "{nobodys}");
        }
    }

    drop_database(&storage, &database).await;
}
