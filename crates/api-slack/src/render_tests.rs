use super::*;
use crate::client::SlackClient;
use oxy::exec_types::event::ArtifactKind;
use oxy::types::{ArtifactValue, ExecuteSQL, SemanticQuery};

/// `finalize()` calls `assistant_threads_set_status` (logged-and-swallowed
/// on error). Pointing the test client at an unreachable address makes
/// the request fail fast locally instead of hitting the live Slack API
/// — which would 401 with our placeholder token, succeed only by being
/// silently swallowed, and pollute test logs / CI traces with spurious
/// outbound traffic. Port 1 is reserved (RFC 1340 / TCPMUX) and the
/// loopback address can't be bound by user code, so the connect
/// returns an immediate `ECONNREFUSED`.
fn make_test_client() -> SlackClient {
    SlackClient::with_base_url("http://127.0.0.1:1")
}

/// No-op renderer for capture/finalize tests. Pass `false` for `upload_charts`
/// so `on_chart` (which we don't call here) would skip the network path; tests
/// don't drive any chart events.
fn make_renderer<'a>(client: &'a SlackClient, upload_charts: bool) -> SlackRenderer<'a> {
    SlackRenderer::new(
        client,
        "token",
        "C123",
        "12345.6789",
        None,
        Uuid::nil(),
        upload_charts,
    )
}

#[tokio::test]
async fn finalize_returns_empty_state_with_no_events() {
    let client = make_test_client();
    let r = make_renderer(&client, false);
    let result = r.finalize().await;
    assert!(result.body.is_empty());
    assert!(result.queued_charts.is_empty());
    assert!(result.chart_local_paths.is_empty());
    assert_eq!(result.failed_chart_count, 0);
    assert!(result.captured_sql_artifacts.is_empty());
}

#[tokio::test]
async fn captures_execute_sql_when_sql_is_on_kind() {
    // The pre-configured-SQL path: tool YAML provides `sql:` directly, so
    // `tool_type.artifact()` populates `ArtifactKind::ExecuteSQL { sql, ... }`
    // with the literal query and we can capture immediately.
    let client = make_test_client();
    let mut r = make_renderer(&client, false);
    r.on_artifact_started(
        "art-1",
        "Top Stores by Total Weekly Sales",
        &ArtifactKind::ExecuteSQL {
            sql: "SELECT * FROM stores".to_string(),
            database: "bigquery-prod".to_string(),
        },
        true,
    )
    .await;
    let result = r.finalize().await;
    assert_eq!(
        result.captured_sql_artifacts,
        vec![CapturedSqlArtifact {
            title: "Top Stores by Total Weekly Sales".to_string(),
            sql: "SELECT * FROM stores".to_string(),
            database: "bigquery-prod".to_string(),
            is_verified: true,
        }]
    );
}

#[tokio::test]
async fn captures_execute_sql_when_sql_arrives_via_value() {
    // The LLM-generated path (the common case): the YAML tool config has
    // no `sql:`, so the kind arrives with `sql=""` and the actual query
    // lands later via `ArtifactValue::ExecuteSQL { sql_query, database }`.
    // Mirrors the SemanticQuery streaming pattern: empty placeholder
    // first, populated value second, optional duplicate after.
    let client = make_test_client();
    let mut r = make_renderer(&client, false);
    r.on_artifact_started(
        "art-1",
        "execute_sql",
        &ArtifactKind::ExecuteSQL {
            sql: String::new(),
            database: "duckdb".to_string(),
        },
        false,
    )
    .await;
    // Empty placeholder — should NOT consume the pending entry.
    r.on_artifact_value(
        "art-1",
        &ArtifactValue::ExecuteSQL(ExecuteSQL {
            database: String::new(),
            sql_query: String::new(),
            result: vec![],
            is_result_truncated: false,
        }),
    )
    .await;
    // Populated value — should capture.
    r.on_artifact_value(
        "art-1",
        &ArtifactValue::ExecuteSQL(ExecuteSQL {
            database: "duckdb".to_string(),
            sql_query: "SELECT * FROM weekly_sales LIMIT 10".to_string(),
            result: vec![],
            is_result_truncated: false,
        }),
    )
    .await;
    // Duplicate populated value — should be a no-op (pending already consumed).
    r.on_artifact_value(
        "art-1",
        &ArtifactValue::ExecuteSQL(ExecuteSQL {
            database: "duckdb".to_string(),
            sql_query: "SELECT * FROM weekly_sales LIMIT 10".to_string(),
            result: vec![vec!["1".into(), "2".into()]],
            is_result_truncated: false,
        }),
    )
    .await;
    let result = r.finalize().await;
    assert_eq!(
        result.captured_sql_artifacts,
        vec![CapturedSqlArtifact {
            title: "execute_sql".to_string(),
            sql: "SELECT * FROM weekly_sales LIMIT 10".to_string(),
            database: "duckdb".to_string(),
            is_verified: false,
        }]
    );
}

#[tokio::test]
async fn captures_semantic_query_when_value_arrives() {
    // SemanticQuery's `kind` is empty — the compiled SQL only shows up
    // in the subsequent `on_artifact_value`. We must pair them by id.
    let client = make_test_client();
    let mut r = make_renderer(&client, false);
    r.on_artifact_started(
        "art-1",
        "query_retail_analytics",
        &ArtifactKind::SemanticQuery {},
        true,
    )
    .await;
    r.on_artifact_value(
        "art-1",
        &ArtifactValue::SemanticQuery(SemanticQuery {
            database: "duckdb".to_string(),
            sql_query: "SELECT region, SUM(weekly_sales) FROM stores GROUP BY 1".to_string(),
            result: vec![],
            error: None,
            validation_error: None,
            sql_generation_error: None,
            is_result_truncated: false,
            topic: Some("retail_analytics".to_string()),
            dimensions: vec![],
            measures: vec![],
            time_dimensions: vec![],
            filters: vec![],
            orders: vec![],
            limit: None,
            offset: None,
        }),
    )
    .await;
    let result = r.finalize().await;
    assert_eq!(
        result.captured_sql_artifacts,
        vec![CapturedSqlArtifact {
            title: "query_retail_analytics".to_string(),
            sql: "SELECT region, SUM(weekly_sales) FROM stores GROUP BY 1".to_string(),
            database: "duckdb".to_string(),
            is_verified: true,
        }]
    );
}

#[tokio::test]
async fn semantic_query_skips_empty_value_then_captures_populated() {
    // Real-world streaming order: the semantic-query tool emits an early
    // empty-SQL placeholder before the compiled query lands. The renderer
    // must keep its pending entry through the empty event and capture only
    // when a non-empty SQL arrives.
    let client = make_test_client();
    let mut r = make_renderer(&client, false);
    r.on_artifact_started(
        "art-1",
        "query_retail_analytics",
        &ArtifactKind::SemanticQuery {},
        true,
    )
    .await;
    // First value event: empty SQL — should NOT consume the pending entry.
    r.on_artifact_value(
        "art-1",
        &ArtifactValue::SemanticQuery(SemanticQuery {
            database: String::new(),
            sql_query: String::new(),
            result: vec![],
            error: None,
            validation_error: None,
            sql_generation_error: None,
            is_result_truncated: false,
            topic: None,
            dimensions: vec![],
            measures: vec![],
            time_dimensions: vec![],
            filters: vec![],
            orders: vec![],
            limit: None,
            offset: None,
        }),
    )
    .await;
    // Second value event: populated SQL — should capture.
    r.on_artifact_value(
        "art-1",
        &ArtifactValue::SemanticQuery(SemanticQuery {
            database: "duckdb".to_string(),
            sql_query: "SELECT 1".to_string(),
            result: vec![],
            error: None,
            validation_error: None,
            sql_generation_error: None,
            is_result_truncated: false,
            topic: Some("retail_analytics".to_string()),
            dimensions: vec![],
            measures: vec![],
            time_dimensions: vec![],
            filters: vec![],
            orders: vec![],
            limit: None,
            offset: None,
        }),
    )
    .await;
    // Third value event: another populated SQL (tool re-emits as it streams).
    // Should NOT push a second block — pending entry was consumed above.
    r.on_artifact_value(
        "art-1",
        &ArtifactValue::SemanticQuery(SemanticQuery {
            database: "duckdb".to_string(),
            sql_query: "SELECT 1, 2".to_string(),
            result: vec![],
            error: None,
            validation_error: None,
            sql_generation_error: None,
            is_result_truncated: false,
            topic: Some("retail_analytics".to_string()),
            dimensions: vec![],
            measures: vec![],
            time_dimensions: vec![],
            filters: vec![],
            orders: vec![],
            limit: None,
            offset: None,
        }),
    )
    .await;
    let result = r.finalize().await;
    assert_eq!(
        result.captured_sql_artifacts,
        vec![CapturedSqlArtifact {
            title: "query_retail_analytics".to_string(),
            sql: "SELECT 1".to_string(),
            database: "duckdb".to_string(),
            is_verified: true,
        }]
    );
}

#[tokio::test]
async fn semantic_query_with_only_empty_sql_is_dropped() {
    // sql_generation_error / validation_error paths can leave sql_query
    // empty. Render path should skip it rather than emit a blank block.
    let client = make_test_client();
    let mut r = make_renderer(&client, false);
    r.on_artifact_started("art-1", "broken", &ArtifactKind::SemanticQuery {}, false)
        .await;
    r.on_artifact_value(
        "art-1",
        &ArtifactValue::SemanticQuery(SemanticQuery {
            database: "duckdb".to_string(),
            sql_query: String::new(),
            result: vec![],
            error: None,
            validation_error: Some("missing measure".to_string()),
            sql_generation_error: None,
            is_result_truncated: false,
            topic: None,
            dimensions: vec![],
            measures: vec![],
            time_dimensions: vec![],
            filters: vec![],
            orders: vec![],
            limit: None,
            offset: None,
        }),
    )
    .await;
    let result = r.finalize().await;
    assert!(result.captured_sql_artifacts.is_empty());
}

#[tokio::test]
async fn ignores_non_sql_bearing_kinds() {
    // Kinds where there is no SQL surface to render inline: Workflow,
    // Agent, OmniQuery, LookerQuery, SandboxApp. SemanticQuery and
    // ExecuteSQL have their own dedicated tests above.
    let client = make_test_client();
    let mut r = make_renderer(&client, false);
    r.on_artifact_started(
        "art-1",
        "A workflow",
        &ArtifactKind::Workflow {
            r#ref: "flow.yml".to_string(),
        },
        false,
    )
    .await;
    r.on_artifact_started(
        "art-2",
        "An agent",
        &ArtifactKind::Agent {
            r#ref: "agent.yml".to_string(),
        },
        false,
    )
    .await;
    let result = r.finalize().await;
    assert!(result.captured_sql_artifacts.is_empty());
}

#[tokio::test]
async fn finalize_output_includes_captured_artifacts_in_arrival_order() {
    let client = make_test_client();
    let mut r = make_renderer(&client, false);
    for i in 0..3 {
        r.on_artifact_started(
            &format!("art-{i}"),
            &format!("Query {i}"),
            &ArtifactKind::ExecuteSQL {
                sql: format!("SELECT {i}"),
                database: "duckdb".to_string(),
            },
            false,
        )
        .await;
    }
    let result = r.finalize().await;
    let titles: Vec<&str> = result
        .captured_sql_artifacts
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Query 0", "Query 1", "Query 2"]);
}
