use super::clarifying::build_triage_user_prompt;
use super::interpreting::build_interpret_user_prompt;
use super::prompts::format_session_turns_section;
use super::*;
use crate::{
    AnalyticsError, AnalyticsIntent, AnalyticsResult, AnalyticsSolution, LlmClient, QuerySpec,
    QuestionType, ResultShape, SemanticCatalog, SolutionPayload, SolutionSource,
};
use agentic_connector::{ConnectorError, ExecutionResult, SqlDialect};
use agentic_core::state::ProblemState;
use agentic_core::{BackTarget, DomainSolver, QueryResult};
use async_trait::async_trait;

// ── StubConnector — always returns an error ────────────────────────────────

struct StubConnector;

#[async_trait]
impl agentic_connector::DatabaseConnector for StubConnector {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::DuckDb
    }

    async fn execute_query(
        &self,
        _sql: &str,
        _limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        Err(ConnectorError::Other("stub".into()))
    }
}

// ── Fixtures ──────────────────────────────────────────────────────────────

fn make_solver() -> AnalyticsSolver {
    AnalyticsSolver::new(
        LlmClient::new("dummy"),
        SemanticCatalog::empty(),
        Box::new(StubConnector),
    )
}

/// Build a SemanticCatalog with orders + customers views for tests
/// that need table names in prompts.
#[allow(dead_code)]
fn make_semantic_catalog_with_tables() -> SemanticCatalog {
    use crate::airlayer_compat;
    let orders_yaml = r#"
name: orders
description: Orders
table: orders
dimensions:
  - name: order_id
type: number
expr: order_id
  - name: revenue
type: number
expr: revenue
  - name: date
type: date
expr: date
"#;
    let customers_yaml = r#"
name: customers
description: Customers
table: customers
dimensions:
  - name: customer_id
type: number
expr: customer_id
  - name: region
type: string
expr: region
"#;
    let views = vec![
        airlayer_compat::parse_view_yaml(orders_yaml).unwrap(),
        airlayer_compat::parse_view_yaml(customers_yaml).unwrap(),
    ];
    let layer = oxy_airlayer_compat::SemanticLayer::new(views, None);
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::with_default(
        oxy_airlayer_compat::Dialect::DuckDB,
    );
    let engine = oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer, dialects).unwrap();
    SemanticCatalog::from_engine(engine)
}

fn make_intent() -> AnalyticsIntent {
    AnalyticsIntent {
        raw_question: "What is total revenue by region?".into(),
        summary: "Total revenue broken down by region".into(),
        question_type: QuestionType::Breakdown,
        metrics: vec!["revenue".into()],
        dimensions: vec!["region".into()],
        filters: vec![],
        history: vec![],
        spec_hint: None,
        selected_automation: None,
        semantic_query: Default::default(),
        semantic_confidence: 0.0,
    }
}

fn make_spec() -> QuerySpec {
    QuerySpec {
        intent: make_intent(),
        resolved_metrics: vec!["orders.revenue".into()],
        resolved_filters: vec![],
        resolved_tables: vec!["orders".into(), "customers".into()],
        join_path: vec![("orders".into(), "customers".into(), "customer_id".into())],
        expected_result_shape: ResultShape::Table {
            columns: vec!["region".into(), "revenue".into()],
        },
        assumptions: vec![],
        solution_source: Default::default(),
        precomputed: None,
        context: None,
        connector_name: "default".to_string(),
        query_request_item: None,
        query_request: None,
        compile_error: None,
    }
}

fn make_solution() -> AnalyticsSolution {
    AnalyticsSolution {
        payload: SolutionPayload::Sql(String::new()),
        solution_source: Default::default(),
        connector_name: "default".to_string(),
        semantic_query: None,
    }
}

fn make_result() -> AnalyticsResult {
    AnalyticsResult::single(
        QueryResult {
            columns: vec![],
            rows: vec![],
            total_row_count: 0,
            truncated: false,
        },
        None,
    )
}

fn make_run_ctx() -> agentic_core::orchestrator::RunContext<AnalyticsDomain> {
    agentic_core::orchestrator::RunContext {
        intent: None,
        spec: None,
        retry_ctx: None,
    }
}

/// RunContext pre-populated with an intent — used by tests that simulate
/// recovery from a `BackTarget::Solve` back-edge, where the intent is no
/// longer embedded in the spec (HasIntent removed) but is available in ctx.
fn make_run_ctx_with_intent() -> agentic_core::orchestrator::RunContext<AnalyticsDomain> {
    agentic_core::orchestrator::RunContext {
        intent: Some(make_intent()),
        spec: None,
        retry_ctx: None,
    }
}

// ── NeedsUserInput → always fatal ─────────────────────────────────────────

#[tokio::test]
async fn diagnose_needs_user_input_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::NeedsUserInput {
        prompt: "Which table?".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Clarify(make_intent(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::NeedsUserInput { .. })
    ));
}

// ── AmbiguousColumn → Clarifying ─────────────────────────────────────────

#[tokio::test]
async fn diagnose_ambiguous_column_with_clarify_back() {
    let mut s = make_solver();
    let err = AnalyticsError::AmbiguousColumn {
        column: "customer_id".into(),
        tables: vec!["orders".into(), "customers".into()],
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Clarify(make_intent(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Clarifying(_))
    ));
}

#[tokio::test]
async fn diagnose_ambiguous_column_with_specify_back() {
    let mut s = make_solver();
    let err = AnalyticsError::AmbiguousColumn {
        column: "customer_id".into(),
        tables: vec!["orders".into(), "customers".into()],
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Specify(make_intent(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Clarifying(_))
    ));
}

#[tokio::test]
async fn diagnose_ambiguous_column_with_solve_back_uses_ctx_intent() {
    let mut s = make_solver();
    let err = AnalyticsError::AmbiguousColumn {
        column: "customer_id".into(),
        tables: vec!["orders".into(), "customers".into()],
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Solve(make_spec(), Default::default()),
            &make_run_ctx_with_intent()
        )
        .await,
        Ok(ProblemState::Clarifying(_))
    ));
}

#[tokio::test]
async fn diagnose_ambiguous_column_with_execute_back_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::AmbiguousColumn {
        column: "x".into(),
        tables: vec!["a".into(), "b".into()],
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Execute(make_solution(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::AmbiguousColumn { .. })
    ));
}

// ── UnresolvedMetric → Specifying ─────────────────────────────────────────

#[tokio::test]
async fn diagnose_unresolved_metric_with_specify_back() {
    let mut s = make_solver();
    let err = AnalyticsError::UnresolvedMetric {
        metric: "revenue".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Specify(make_intent(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Specifying(_))
    ));
}

#[tokio::test]
async fn diagnose_unresolved_metric_with_solve_back_uses_ctx_intent() {
    let mut s = make_solver();
    let err = AnalyticsError::UnresolvedMetric {
        metric: "revenue".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Solve(make_spec(), Default::default()),
            &make_run_ctx_with_intent()
        )
        .await,
        Ok(ProblemState::Specifying(_))
    ));
}

#[tokio::test]
async fn diagnose_unresolved_metric_with_execute_back_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::UnresolvedMetric {
        metric: "revenue".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Execute(make_solution(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::UnresolvedMetric { .. })
    ));
}

// ── SyntaxError ───────────────────────────────────────────────────────────

#[tokio::test]
async fn diagnose_syntax_error_with_solve_back() {
    let mut s = make_solver();
    let err = AnalyticsError::SyntaxError {
        query: "SELECT * FORM orders".into(),
        message: "unexpected token".into(),
    };
    // Solving is absorbed into specifying — BackTarget::Solve routes to Specifying.
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Solve(make_spec(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Specifying(_))
    ));
}

#[tokio::test]
async fn diagnose_syntax_error_with_specify_back() {
    let mut s = make_solver();
    let err = AnalyticsError::SyntaxError {
        query: "bad sql".into(),
        message: "parse error".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Specify(make_intent(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Specifying(_))
    ));
}

#[tokio::test]
async fn diagnose_syntax_error_with_execute_back_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::SyntaxError {
        query: "bad".into(),
        message: "error".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Execute(make_solution(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::SyntaxError { .. })
    ));
}

// ── EmptyResults → Specifying ─────────────────────────────────────────────

#[tokio::test]
async fn diagnose_empty_results_with_solve_back() {
    let mut s = make_solver();
    let err = AnalyticsError::EmptyResults {
        query: "SELECT …".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Solve(make_spec(), Default::default()),
            &make_run_ctx_with_intent()
        )
        .await,
        Ok(ProblemState::Specifying(_))
    ));
}

#[tokio::test]
async fn diagnose_empty_results_with_clarify_back() {
    let mut s = make_solver();
    let err = AnalyticsError::EmptyResults {
        query: "SELECT …".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Clarify(make_intent(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Specifying(_))
    ));
}

#[tokio::test]
async fn diagnose_empty_results_with_execute_back_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::EmptyResults {
        query: "SELECT …".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Execute(make_solution(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::EmptyResults { .. })
    ));
}

// ── ShapeMismatch ─────────────────────────────────────────────────────────

#[tokio::test]
async fn diagnose_shape_mismatch_with_solve_back() {
    let mut s = make_solver();
    let err = AnalyticsError::ShapeMismatch {
        expected: ResultShape::Scalar,
        actual: ResultShape::Series,
    };
    // Solving is absorbed into specifying — BackTarget::Solve routes to Specifying.
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Solve(make_spec(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Specifying(_))
    ));
}

#[tokio::test]
async fn diagnose_shape_mismatch_with_specify_back() {
    let mut s = make_solver();
    let err = AnalyticsError::ShapeMismatch {
        expected: ResultShape::Scalar,
        actual: ResultShape::Series,
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Specify(make_intent(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Specifying(_))
    ));
}

#[tokio::test]
async fn diagnose_shape_mismatch_with_execute_back_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::ShapeMismatch {
        expected: ResultShape::Scalar,
        actual: ResultShape::Series,
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Execute(make_solution(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::ShapeMismatch { .. })
    ));
}

// ── ValueAnomaly → Interpreting ───────────────────────────────────────────

#[tokio::test]
async fn diagnose_value_anomaly_with_interpret_back() {
    let mut s = make_solver();
    let err = AnalyticsError::ValueAnomaly {
        column: "revenue".into(),
        value: "999999".into(),
        reason: "outlier".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Interpret(make_result(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Ok(ProblemState::Interpreting(_))
    ));
}

#[tokio::test]
async fn diagnose_value_anomaly_with_execute_back_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::ValueAnomaly {
        column: "revenue".into(),
        value: "NaN".into(),
        reason: "nan detected".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Execute(make_solution(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::ValueAnomaly { .. })
    ));
}

#[tokio::test]
async fn diagnose_value_anomaly_with_solve_back_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::ValueAnomaly {
        column: "revenue".into(),
        value: "Inf".into(),
        reason: "infinite".into(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Solve(make_spec(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::ValueAnomaly { .. })
    ));
}

// should_skip tests removed — solving is absorbed into specifying.

// ── Automation path ──────────────────────────────────────────────────────

#[tokio::test]
async fn specify_impl_short_circuits_when_selected_automation_is_set() {
    let mut s = make_solver();
    let file_path = std::path::PathBuf::from("workflows/monthly_sales.procedure.yml");
    let intent = AnalyticsIntent {
        selected_automation: Some(file_path.clone()),
        ..make_intent()
    };

    let result = s.specify_impl(intent, None).await;

    let specs = result
        .map_err(|(e, _)| e)
        .expect("specify_impl must succeed when selected_automation is set");
    assert_eq!(specs.len(), 1, "must return exactly one spec");
    assert_eq!(
        specs[0].solution_source,
        SolutionSource::Automation {
            file_path: file_path.clone()
        },
        "spec must carry SolutionSource::Automation with the selected path",
    );
    assert!(
        specs[0].resolved_metrics.is_empty(),
        "short-circuit spec must have no resolved metrics (LLM was not called)",
    );
}

/// Regression: `build_specifying_handler` unconditionally overwrites
/// `solution_source` to `LlmWithSemanticContext` after `specify_impl` returns,
/// even when `specify_impl` short-circuited with `SolutionSource::Automation`.
///
/// This means that even when `intent.selected_automation` is set, the spec
/// forwarded to the Solving stage has the wrong `solution_source`, causing
/// `should_skip` to miss the automation path and `SpecResolved` to emit `"Llm"`.
#[tokio::test]
async fn specifying_handler_preserves_automation_solution_source_when_selected_automation_is_set() {
    let file_path = std::path::PathBuf::from("workflows/monthly_sales.procedure.yml");
    let intent = AnalyticsIntent {
        selected_automation: Some(file_path.clone()),
        ..make_intent()
    };

    let mut solver = make_solver();
    let handlers = build_analytics_handlers();
    let execute_fn = {
        let h = handlers
            .get("specifying")
            .expect("specifying handler must exist");
        std::sync::Arc::clone(&h.execute)
    };
    let run_ctx = make_run_ctx();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Specifying(intent),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    // Specifying handler now transitions directly to Executing for automations.
    match result.state_data {
        ProblemState::Executing(solution) => {
            assert_eq!(
                solution.solution_source,
                SolutionSource::Automation {
                    file_path: file_path.clone()
                },
                "specifying handler must preserve SolutionSource::Automation when \
                 selected_automation is set — got {:?} instead",
                solution.solution_source,
            );
        }
        other => panic!(
            "expected ProblemState::Executing, got: {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

// ── SQL file (verified-query) path through the Specifying handler ─────────

/// Write `content` to a fresh `.sql` file under a unique sub-directory of the
/// system temp dir, returning its absolute path. The test owns cleanup.
fn write_temp_sql(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxy_sql_test_{}_{}",
        std::process::id(),
        // Cheap per-call uniqueness so concurrent tests don't collide.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// Build a multi-connector solver with a named connector in addition to the
/// default. Used to verify the SQL file `database:` annotation routes to a
/// specific connector.
fn make_solver_with_named_connector(name: &str) -> AnalyticsSolver {
    use std::collections::HashMap;
    use std::sync::Arc;
    let mut connectors: HashMap<String, Arc<dyn agentic_connector::DatabaseConnector>> =
        HashMap::new();
    connectors.insert("default".to_string(), Arc::new(StubConnector));
    connectors.insert(name.to_string(), Arc::new(StubConnector));
    AnalyticsSolver::new_multi(
        LlmClient::new("dummy"),
        SemanticCatalog::empty(),
        connectors,
        "default".to_string(),
    )
}

#[tokio::test]
async fn specifying_handler_short_circuits_for_sql_file_with_oxy_block() {
    let sql_content = "/*\n  oxy:\n    description: \"Daily revenue\"\n*/\nSELECT 1;";
    let file_path = write_temp_sql("daily_revenue.sql", sql_content);

    let intent = AnalyticsIntent {
        selected_automation: Some(file_path.clone()),
        ..make_intent()
    };

    let mut solver = make_solver();
    let handlers = build_analytics_handlers();
    let execute_fn = std::sync::Arc::clone(
        &handlers
            .get("specifying")
            .expect("specifying handler must exist")
            .execute,
    );
    let run_ctx = make_run_ctx();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Specifying(intent),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    match result.state_data {
        ProblemState::Executing(solution) => {
            assert_eq!(
                solution.solution_source,
                SolutionSource::SqlFile {
                    file_path: file_path.clone()
                },
                "must carry SolutionSource::SqlFile",
            );
            assert!(
                matches!(&solution.payload, SolutionPayload::Sql(s) if s == sql_content),
                "must carry the file contents as precomputed SQL",
            );
            assert_eq!(
                solution.connector_name, "default",
                "must fall back to the default connector when no `database:` annotation is set",
            );
        }
        other => panic!(
            "expected ProblemState::Executing, got: {:?}",
            std::mem::discriminant(&other)
        ),
    }

    let _ = std::fs::remove_dir_all(file_path.parent().unwrap());
}

#[tokio::test]
async fn specifying_handler_returns_file_read_error_for_missing_sql_file() {
    let file_path = std::path::PathBuf::from("/nonexistent/path/missing_query.sql");
    let intent = AnalyticsIntent {
        selected_automation: Some(file_path.clone()),
        ..make_intent()
    };

    let mut solver = make_solver();
    let handlers = build_analytics_handlers();
    let execute_fn = std::sync::Arc::clone(
        &handlers
            .get("specifying")
            .expect("specifying handler must exist")
            .execute,
    );
    let run_ctx = make_run_ctx();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Specifying(intent),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    match result.state_data {
        ProblemState::Diagnosing { error, back } => {
            assert!(
                matches!(&error, AnalyticsError::FileReadError { file_path: fp, .. }
                    if fp.contains("missing_query.sql")),
                "expected FileReadError for missing SQL file, got: {error:?}",
            );
            assert!(
                matches!(back, BackTarget::Clarify(_, _)),
                "must hand the back-edge to Clarify so the LLM can recover",
            );
        }
        other => panic!(
            "expected ProblemState::Diagnosing, got: {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

#[tokio::test]
async fn specifying_handler_routes_sql_file_to_database_from_oxy_annotation() {
    let sql_content = "/*\n  oxy:\n    database: analytics_db\n*/\nSELECT 1;";
    let file_path = write_temp_sql("by_db.sql", sql_content);

    let intent = AnalyticsIntent {
        selected_automation: Some(file_path.clone()),
        ..make_intent()
    };

    let mut solver = make_solver_with_named_connector("analytics_db");
    let handlers = build_analytics_handlers();
    let execute_fn = std::sync::Arc::clone(
        &handlers
            .get("specifying")
            .expect("specifying handler must exist")
            .execute,
    );
    let run_ctx = make_run_ctx();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Specifying(intent),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    match result.state_data {
        ProblemState::Executing(solution) => {
            assert_eq!(
                solution.connector_name, "analytics_db",
                "must honor `database:` annotation when the named connector exists",
            );
        }
        other => panic!(
            "expected ProblemState::Executing, got: {:?}",
            std::mem::discriminant(&other)
        ),
    }

    let _ = std::fs::remove_dir_all(file_path.parent().unwrap());
}

#[tokio::test]
async fn a_verified_querys_unregistered_database_survives_to_the_routing_guard() {
    let sql_content = "/*\n  oxy:\n    database: not_registered\n*/\nSELECT 1;";
    let file_path = write_temp_sql("unknown_db.sql", sql_content);

    let intent = AnalyticsIntent {
        selected_automation: Some(file_path.clone()),
        ..make_intent()
    };

    let mut solver = make_solver();
    let handlers = build_analytics_handlers();
    let execute_fn = std::sync::Arc::clone(
        &handlers
            .get("specifying")
            .expect("specifying handler must exist")
            .execute,
    );
    let run_ctx = make_run_ctx();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Specifying(intent),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    match result.state_data {
        ProblemState::Executing(solution) => {
            // NOT "default". Substituting it here would launder the misroute
            // past `resolve_solution_connector`, which only sees what this
            // stage puts in `connector_name` -- and a Verified Query is
            // hand-written SQL bound to one dialect, so running it elsewhere
            // is the failure this whole change exists to stop.
            assert_eq!(
                solution.connector_name, "not_registered",
                "the annotated database must reach execute so the routing guard can reject it",
            );
        }
        other => panic!(
            "expected ProblemState::Executing, got: {:?}",
            std::mem::discriminant(&other)
        ),
    }

    let _ = std::fs::remove_dir_all(file_path.parent().unwrap());
}

// ── workspace-relative SQL file path resolution ───────────────────────────

/// A relative `selected_automation` path must be joined against `workspace_path`
/// and read successfully, matching the behaviour of `search_automations` which
/// returns workspace-relative paths.
#[tokio::test]
async fn specifying_handler_resolves_relative_sql_path_against_workspace() {
    let sql_content =
        "/*\n  oxy:\n    description: \"Store count\"\n*/\nSELECT COUNT(*) FROM stores;";
    // Write to a temp workspace dir, keeping a relative name.
    let workspace = std::env::temp_dir().join(format!(
        "oxy_ws_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(workspace.join("example_sql")).unwrap();
    std::fs::write(
        workspace.join("example_sql/total_number_of_store.sql"),
        sql_content,
    )
    .unwrap();

    // Supply only the relative path — as search_automations would return it.
    let rel_path = std::path::PathBuf::from("example_sql/total_number_of_store.sql");
    let intent = AnalyticsIntent {
        selected_automation: Some(rel_path.clone()),
        ..make_intent()
    };

    let mut solver = make_solver().with_workspace_path(workspace.clone());
    let handlers = build_analytics_handlers();
    let execute_fn = std::sync::Arc::clone(
        &handlers
            .get("specifying")
            .expect("specifying handler must exist")
            .execute,
    );
    let run_ctx = make_run_ctx();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Specifying(intent),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    match result.state_data {
        ProblemState::Executing(solution) => {
            assert!(
                matches!(
                    &solution.solution_source,
                    SolutionSource::SqlFile { file_path: fp } if *fp == rel_path
                ),
                "must carry the original relative path in SolutionSource",
            );
            assert!(
                matches!(&solution.payload, SolutionPayload::Sql(sql) if !sql.is_empty()),
                "SQL payload must be non-empty",
            );
        }
        other => panic!(
            "expected ProblemState::Executing, got: {:?}",
            std::mem::discriminant(&other)
        ),
    }

    let _ = std::fs::remove_dir_all(&workspace);
}

/// Without `workspace_path` configured, a bare relative path fails to read
/// (resolves against CWD, which won't have the file), confirming the fix is
/// load-bearing.
#[tokio::test]
async fn specifying_handler_relative_sql_path_fails_without_workspace_path() {
    let rel_path = std::path::PathBuf::from("example_sql/this_file_does_not_exist_in_cwd.sql");
    let intent = AnalyticsIntent {
        selected_automation: Some(rel_path.clone()),
        ..make_intent()
    };

    // No with_workspace_path — solver has workspace_path: None.
    let mut solver = make_solver();
    let handlers = build_analytics_handlers();
    let execute_fn = std::sync::Arc::clone(
        &handlers
            .get("specifying")
            .expect("specifying handler must exist")
            .execute,
    );
    let run_ctx = make_run_ctx();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Specifying(intent),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    assert!(
        matches!(
            result.state_data,
            ProblemState::Diagnosing {
                error: AnalyticsError::FileReadError { .. },
                ..
            }
        ),
        "relative path without workspace_path must produce FileReadError",
    );
}

/// A path containing `..` that would escape the workspace root must be
/// rejected with a FileReadError, not silently read.
#[tokio::test]
async fn specifying_handler_rejects_traversal_path() {
    let workspace = std::env::temp_dir().join(format!(
        "oxy_traversal_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&workspace).unwrap();

    // A traversal path that escapes the workspace. The `.sql`
    // extension is load-bearing: the containment guard lives inside
    // the SQL-file short-circuit in `build_specifying_handler`'s
    // step-0 branch, which only fires for `.sql` paths.
    let traversal = std::path::PathBuf::from("../../etc/passwd.sql");
    let intent = AnalyticsIntent {
        selected_automation: Some(traversal),
        ..make_intent()
    };

    let mut solver = make_solver().with_workspace_path(workspace.clone());
    let handlers = build_analytics_handlers();
    let execute_fn = std::sync::Arc::clone(
        &handlers
            .get("specifying")
            .expect("specifying handler must exist")
            .execute,
    );
    let run_ctx = make_run_ctx();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Specifying(intent),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    assert!(
        matches!(
            result.state_data,
            ProblemState::Diagnosing {
                error: AnalyticsError::FileReadError { ref message, .. },
                ..
            } if message.contains("escapes workspace root")
        ),
        "traversal path must be rejected with a containment error",
    );

    let _ = std::fs::remove_dir_all(&workspace);
}

// ── SQL file failure routing through the Executing handler ────────────────

#[tokio::test]
async fn execute_handler_routes_sql_file_failure_to_clarify_not_specify() {
    use agentic_core::{RunContext, SessionMemory};

    let mut solver = make_solver();
    let file_path = std::path::PathBuf::from("queries/broken.sql");
    let solution = AnalyticsSolution {
        payload: SolutionPayload::Sql("SELECT * FROM nope".to_string()),
        solution_source: SolutionSource::SqlFile {
            file_path: file_path.clone(),
        },
        connector_name: "default".to_string(),
        semantic_query: None,
    };

    let handlers = build_analytics_handlers();
    let execute_fn = std::sync::Arc::clone(
        &handlers
            .get("executing")
            .expect("executing handler must exist")
            .execute,
    );
    let run_ctx = RunContext {
        intent: Some(make_intent()),
        spec: Some(make_spec()),
        retry_ctx: None,
    };
    let memory = SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Executing(solution),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    match result.state_data {
        ProblemState::Diagnosing { back, .. } => {
            assert!(
                matches!(back, BackTarget::Clarify(_, _)),
                "SqlFile failures must route to Clarify (not Specify) so the LLM \
                 can pick a different path instead of looping on the same file",
            );
        }
        other => panic!(
            "expected ProblemState::Diagnosing, got: {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

// ── FileReadError surfaces as fatal in diagnose ───────────────────────────

#[tokio::test]
async fn diagnose_file_read_error_is_fatal() {
    let mut s = make_solver();
    let err = AnalyticsError::FileReadError {
        file_path: "queries/missing.sql".to_string(),
        message: "No such file or directory".to_string(),
    };
    assert!(matches!(
        s.diagnose(
            err,
            BackTarget::Clarify(make_intent(), Default::default()),
            &make_run_ctx()
        )
        .await,
        Err(AnalyticsError::FileReadError { .. })
    ));
}

// ── execution_type_for tags SqlFile as verified ───────────────────────────

#[test]
fn execution_type_for_sql_file_is_verified() {
    use crate::solver::executing::execution_type_for;
    let (kind, is_verified) = execution_type_for(&SolutionSource::SqlFile {
        file_path: std::path::PathBuf::from("queries/q.sql"),
    });
    assert_eq!(kind, "verified_sql");
    assert!(is_verified);
}

// ── Prompt builder: session turns injection ───────────────────────────────

fn make_completed_turn(
    question: &str,
    answer: &str,
) -> agentic_core::CompletedTurn<AnalyticsDomain> {
    agentic_core::CompletedTurn {
        intent: AnalyticsIntent {
            raw_question: question.into(),
            summary: String::new(),
            question_type: QuestionType::Breakdown,
            metrics: vec![],
            dimensions: vec![],
            filters: vec![],
            history: vec![],
            spec_hint: None,
            selected_automation: None,
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        },
        spec: None,
        answer: crate::AnalyticsAnswer {
            text: answer.into(),
            display_blocks: vec![],
            spec_hint: None,
        },
        trace_id: "t-test".into(),
    }
}

#[allow(dead_code)]
fn make_hypothesis() -> crate::types::DomainHypothesis {
    crate::types::DomainHypothesis {
        summary: "The user wants revenue broken down by region.".into(),
        question_type: QuestionType::Breakdown,
        time_scope: None,
        confidence: 0.9,
        ambiguities: vec![],
        ambiguity_questions: vec![],
        selected_automation_path: None,
        semantic_query: None,
        semantic_confidence: 0.0,
        missing_members: vec![],
    }
}

#[test]
fn triage_prompt_includes_question() {
    let intent = make_intent();
    let prompt = build_triage_user_prompt(&intent, &[], "");
    assert!(
        prompt.contains("total revenue"),
        "should contain the raw question"
    );
    assert!(
        prompt.contains("search_automations"),
        "should instruct to call search_automations"
    );
    assert!(
        prompt.contains("search_catalog"),
        "should instruct to use search_catalog for discovery"
    );
}

#[test]
fn triage_prompt_instructs_missing_members() {
    let intent = make_intent();
    let prompt = build_triage_user_prompt(&intent, &[], "");
    assert!(
        prompt.contains("missing_members"),
        "should instruct about missing_members"
    );
}

#[test]
fn triage_prompt_instructs_propose_semantic_query() {
    let intent = make_intent();
    let prompt = build_triage_user_prompt(&intent, &[], "");
    assert!(
        prompt.contains("propose_semantic_query"),
        "should instruct to call propose_semantic_query tool"
    );
}

#[test]
fn triage_tools_include_propose_semantic_query() {
    let tools = crate::tools::triage_tools(false);
    assert!(
        tools.iter().any(|t| t.name == "propose_semantic_query"),
        "triage_tools must include propose_semantic_query"
    );
}

#[test]
fn triage_response_schema_does_not_contain_semantic_query() {
    let schema = crate::schemas::triage_response_schema();
    let props = schema.schema["properties"].as_object().unwrap();
    assert!(
        !props.contains_key("semantic_query"),
        "semantic_query should be a tool, not in the response schema"
    );
    assert!(
        !props.contains_key("semantic_confidence"),
        "semantic_confidence moved to propose_semantic_query tool"
    );
}

// ── Interpretation prompt ────────────────────────────────────────────────

#[test]
fn interpret_prompt_includes_last_turn_for_comparison() {
    let prior = vec![
        make_completed_turn("First question", "First answer"),
        make_completed_turn("What is revenue by region?", "West leads."),
    ];
    let result = make_result();
    let prompt = build_interpret_user_prompt(
        "How about by product category?",
        &[],
        &result,
        None,
        &prior,
        None,
    );
    assert!(
        prompt.contains("Previous question:"),
        "should include prior question label"
    );
    assert!(
        prompt.contains("What is revenue by region?"),
        "should include last prior question"
    );
    assert!(
        prompt.contains("West leads."),
        "should include last prior answer"
    );
    assert!(
        prompt.contains("comparatively"),
        "should mention comparative framing"
    );
}

#[test]
fn interpret_prompt_without_session_history_has_no_prior_question() {
    let result = make_result();
    let prompt = build_interpret_user_prompt("What is revenue?", &[], &result, None, &[], None);
    assert!(
        !prompt.contains("Previous question:"),
        "no prior question section when no prior turns",
    );
}

#[test]
fn session_turns_section_formats_multiple_turns() {
    let turns = vec![
        make_completed_turn("Q1", "A1"),
        make_completed_turn("Q2", "A2"),
    ];
    let section = format_session_turns_section(&turns);
    assert!(section.contains("Turn 1:"));
    assert!(section.contains("Turn 2:"));
    assert!(section.contains("Q1"));
    assert!(section.contains("A2"));
}

#[test]
fn session_turns_section_empty_for_no_turns() {
    assert_eq!(format_session_turns_section(&[]), "");
}

// ── strip_json_fences ─────────────────────────────────────────────────────

#[test]
fn strip_fences_removes_json_code_fence() {
    let raw = "```json\n{\"key\": \"value\"}\n```";
    assert_eq!(strip_json_fences(raw), "{\"key\": \"value\"}");
}

#[test]
fn strip_fences_removes_plain_code_fence() {
    let raw = "```\n{\"key\": \"value\"}\n```";
    assert_eq!(strip_json_fences(raw), "{\"key\": \"value\"}");
}

#[test]
fn strip_fences_bare_json_is_unchanged() {
    let raw = "{\"question_type\": \"Trend\"}";
    assert_eq!(strip_json_fences(raw), raw);
}

#[test]
fn strip_fences_trims_surrounding_whitespace() {
    let raw = "  \n  {\"a\": 1}  \n  ";
    assert_eq!(strip_json_fences(raw), "{\"a\": 1}");
}

#[test]
fn strip_fences_json_fence_with_extra_whitespace() {
    let raw = "  ```json\n  SELECT 1\n  ```  ";
    assert_eq!(strip_json_fences(raw), "SELECT 1");
}

#[test]
fn strip_fences_empty_string() {
    assert_eq!(strip_json_fences(""), "");
}

#[test]
fn strip_fences_partial_fence_not_stripped() {
    let raw = "```json\n{\"a\":1}";
    let result = strip_json_fences(raw);
    assert!(
        result.contains("{\"a\":1}"),
        "JSON content must be preserved: '{result}'"
    );
}

// ── Budget-exhaustion suspension / resume tests ───────────────────────────

/// Minimal Anthropic-style mock provider used in budget-exhaustion tests.
///
/// Returns one pre-scripted `Vec<Chunk>` per `stream()` call and records
/// the `messages` array sent on each call so tests can assert the resume
/// conversation structure.
struct ScriptedProvider {
    rounds: std::sync::Mutex<std::collections::VecDeque<Vec<crate::llm::Chunk>>>,
    captured: std::sync::Arc<std::sync::Mutex<Vec<Vec<serde_json::Value>>>>,
}

impl ScriptedProvider {
    fn new(
        rounds: Vec<Vec<crate::llm::Chunk>>,
        captured: std::sync::Arc<std::sync::Mutex<Vec<Vec<serde_json::Value>>>>,
    ) -> Self {
        Self {
            rounds: std::sync::Mutex::new(rounds.into()),
            captured,
        }
    }
}

#[async_trait]
impl crate::llm::LlmProvider for ScriptedProvider {
    async fn stream(
        &self,
        _system: &str,
        _system_date_suffix: &str,
        messages: &[serde_json::Value],
        _tools: &[agentic_core::tools::ToolDef],
        _thinking: &crate::llm::ThinkingConfig,
        _response_schema: Option<&crate::llm::ResponseSchema>,
        _max_tokens_override: Option<u32>,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures_core::Stream<Item = Result<crate::llm::Chunk, crate::llm::LlmError>>
                    + Send,
            >,
        >,
        crate::llm::LlmError,
    > {
        self.captured.lock().unwrap().push(messages.to_vec());
        let chunks = self.rounds.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::pin(tokio_stream::iter(
            chunks.into_iter().map(Ok::<_, crate::llm::LlmError>),
        )))
    }

    fn assistant_message(&self, blocks: &[crate::llm::ContentBlock]) -> serde_json::Value {
        let content: Vec<serde_json::Value> = blocks
            .iter()
            .map(|b| match b {
                crate::llm::ContentBlock::Text { text } => {
                    serde_json::json!({"type": "text", "text": text})
                }
                crate::llm::ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    serde_json::json!({"type":"tool_use","id":id,"name":name,"input":input})
                }
                other => serde_json::to_value(format!("{other:?}")).unwrap(),
            })
            .collect();
        serde_json::json!({"role": "assistant", "content": content})
    }

    fn tool_result_messages(&self, results: &[(String, String, bool)]) -> Vec<serde_json::Value> {
        let blocks: Vec<serde_json::Value> = results
            .iter()
            .map(|(id, content, is_error)| {
                serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": content,
                    "is_error": is_error
                })
            })
            .collect();
        vec![serde_json::json!({"role": "user", "content": blocks})]
    }

    fn model_name(&self) -> &str {
        "scripted"
    }
}

fn make_solver_with_provider(provider: impl crate::llm::LlmProvider + 'static) -> AnalyticsSolver {
    AnalyticsSolver::new(
        LlmClient::with_provider(provider),
        SemanticCatalog::empty(),
        Box::new(StubConnector),
    )
}

/// Helper: a `Chunk` sequence that emits some text then hits MaxTokens.
fn chunks_text_then_max_tokens(text: &str) -> Vec<crate::llm::Chunk> {
    vec![
        crate::llm::Chunk::Text(text.to_string()),
        crate::llm::Chunk::Done(crate::llm::Usage {
            input_tokens: 10,
            output_tokens: 4096,
            stop_reason: crate::llm::StopReason::MaxTokens,
            ..Default::default()
        }),
    ]
}

/// Helper: a `Chunk` sequence that emits a single tool call (triggers rounds).
fn chunks_tool_call(id: &str, name: &str) -> Vec<crate::llm::Chunk> {
    vec![
        crate::llm::Chunk::ToolCall(crate::llm::ToolCallChunk {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::json!({}),
            provider_data: None,
        }),
        crate::llm::Chunk::Done(crate::llm::Usage {
            ..Default::default()
        }),
    ]
}

// -- Suspension: interpret_impl MaxToolRounds -----------------------------

/// `interpret_impl` must suspend with `suspension_type = "max_tool_rounds"`
/// when all tool rounds are consumed, instead of propagating a generic error.
///
/// Bug: before the fix, the `.map_err` in `interpret_impl` swallowed
/// `MaxToolRoundsReached` and returned a plain error; the user was never
/// asked whether they want to continue with more rounds.
#[tokio::test]
async fn interpret_impl_max_tool_rounds_stores_suspension_data() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    // max_rounds defaults to 5 for interpreting; provide 6 rounds of
    // tool calls so the limit is hit on round index 5.
    let rounds: Vec<Vec<crate::llm::Chunk>> = (0..=5)
        .map(|i| chunks_tool_call(&format!("tc{i}"), "render_chart"))
        .collect();
    let provider = ScriptedProvider::new(rounds, captured);
    let mut solver = make_solver_with_provider(provider);

    let result = make_result();
    let outcome = solver
        .interpret_impl("What is revenue?", &[], result, &[], None)
        .await;

    assert!(
        matches!(
            outcome,
            Err((
                AnalyticsError::NeedsUserInput { .. },
                BackTarget::Suspend { .. }
            ))
        ),
        "interpret_impl must suspend on MaxToolRoundsReached"
    );

    let sd = solver
        .suspension_data
        .take()
        .expect("suspension_data must be set");
    assert_eq!(sd.from_state, "interpreting");
    assert_eq!(
        sd.stage_data["suspension_type"].as_str(),
        Some("max_tool_rounds"),
        "stage_data must tag suspension_type"
    );
    assert!(
        sd.stage_data["extra_rounds"].as_u64().is_some(),
        "extra_rounds must be stored"
    );
    assert!(
        sd.stage_data["conversation_history"].is_array(),
        "conversation_history must be stored for resume"
    );
}

// -- Suspension: specify_impl MaxTokens -----------------------------------

/// `specify_impl` must suspend with `suspension_type = "max_tokens"` and
/// store the intent so that resume can re-enter `ProblemState::Specifying`.
#[tokio::test]
async fn specify_impl_max_tokens_stores_suspension_data() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let provider =
        ScriptedProvider::new(vec![chunks_text_then_max_tokens("partial spec")], captured);
    let mut solver = make_solver_with_provider(provider);

    let intent = make_intent();
    let result = solver.specify_impl(intent, None).await;

    assert!(
        matches!(
            result,
            Err((
                AnalyticsError::NeedsUserInput { .. },
                BackTarget::Suspend { .. }
            ))
        ),
        "specify_impl must suspend on MaxTokensReached"
    );

    let sd = solver.suspension_data.take().expect("must have suspension");
    assert_eq!(sd.from_state, "specifying");
    assert_eq!(
        sd.stage_data["suspension_type"].as_str(),
        Some("max_tokens")
    );
    assert!(
        sd.stage_data["intent"].is_object(),
        "intent must be stored for resume routing"
    );
    let doubled = sd.stage_data["max_tokens_override"].as_u64().unwrap_or(0);
    assert_eq!(doubled, 8192, "must double DEFAULT_MAX_TOKENS (4096→8192)");
}

// -- Suspension: solve_impl MaxTokens ------------------------------------

/// `solve_impl` must suspend with `suspension_type = "max_tokens"` and
/// store the QuerySpec so that resume can re-enter `ProblemState::Solving`.
#[tokio::test]
async fn solve_impl_max_tokens_stores_suspension_data() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let provider =
        ScriptedProvider::new(vec![chunks_text_then_max_tokens("partial sql")], captured);
    let mut solver = make_solver_with_provider(provider);

    let spec = make_spec();
    let result = solver.solve_impl(spec, None).await;

    assert!(
        matches!(
            result,
            Err((
                AnalyticsError::NeedsUserInput { .. },
                BackTarget::Suspend { .. }
            ))
        ),
        "solve_impl must suspend on MaxTokensReached"
    );

    let sd = solver.suspension_data.take().expect("must have suspension");
    assert_eq!(sd.from_state, "solving");
    assert_eq!(
        sd.stage_data["suspension_type"].as_str(),
        Some("max_tokens")
    );
    assert!(
        sd.stage_data["spec"].is_object(),
        "QuerySpec must be stored for resume routing"
    );
}

// -- Resume routing: problem_state_from_resume ----------------------------

/// Resuming from a "solving" suspension now routes to `ProblemState::Specifying`
/// (solving is absorbed into specifying).
#[test]
fn problem_state_from_resume_solving_reconstructs_as_specifying() {
    use crate::solver::resuming::problem_state_from_resume;
    use agentic_core::human_input::SuspendedRunData;

    let spec = make_spec();
    let spec_value = serde_json::to_value(&spec).expect("QuerySpec must serialize");
    let data = SuspendedRunData {
        from_state: "solving".to_string(),
        original_input: "test question".to_string(),
        trace_id: String::new(),
        stage_data: serde_json::json!({
            "spec": spec_value,
            "conversation_history": [],
            "suspension_type": "max_tokens",
        }),
        question: "Continue?".to_string(),
        suggestions: vec![],
    };

    let state = problem_state_from_resume(&data, None);
    match state {
        ProblemState::Specifying(recovered_intent) => {
            assert_eq!(recovered_intent.raw_question, spec.intent.raw_question);
        }
        _other => panic!("expected ProblemState::Specifying, got a different variant"),
    }
}

#[test]
fn problem_state_from_resume_solving_bad_spec_falls_back_to_clarifying() {
    use crate::solver::resuming::problem_state_from_resume;
    use agentic_core::human_input::SuspendedRunData;

    let data = SuspendedRunData {
        from_state: "solving".to_string(),
        original_input: "original question".to_string(),
        trace_id: String::new(),
        stage_data: serde_json::json!({ "spec": "not-an-object" }),
        question: String::new(),
        suggestions: vec![],
    };

    let state = problem_state_from_resume(&data, None);
    assert!(
        matches!(state, ProblemState::Clarifying(_)),
        "corrupt spec must fall back to Clarifying"
    );
}

// ── Resume after builder delegation ────────────────────────────────────────

/// After builder delegation completes (success or failure), the pipeline
/// resumes into Clarifying with a fresh intent so triage re-runs against
/// the (potentially updated) catalog.
#[test]
fn resume_from_builder_delegation_re_enters_clarifying() {
    use crate::solver::resuming::problem_state_from_resume;
    use agentic_core::human_input::SuspendedRunData;

    let data = SuspendedRunData {
        from_state: "clarifying".to_string(),
        original_input: "what is revenue per customer?".to_string(),
        trace_id: "trace-123".to_string(),
        stage_data: serde_json::json!({}),
        question: "Create missing measure revenue_per_customer".to_string(),
        suggestions: vec![],
    };

    let state = problem_state_from_resume(&data, Some("done"));
    match state {
        ProblemState::Clarifying(intent) => {
            assert_eq!(intent.raw_question, "what is revenue per customer?");
            // Must have zero confidence to force full re-triage
            assert_eq!(intent.semantic_confidence, 0.0);
        }
        _ => panic!("expected ProblemState::Clarifying after builder delegation resume"),
    }
}

// ── DuckDB binder-error back-edge regression ──────────────────────────────

#[cfg(feature = "duckdb-test")]
mod duckdb_integration {
    use super::*;
    use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, ResultSummary};
    use agentic_core::back_target::BackTarget;
    use agentic_core::state::ProblemState;
    use agentic_core::{RunContext, SessionMemory};
    use duckdb::Connection;

    struct RawDuckDbConnector(std::sync::Mutex<Connection>);

    #[async_trait]
    impl DatabaseConnector for RawDuckDbConnector {
        async fn execute_query(
            &self,
            sql: &str,
            limit: u64,
        ) -> Result<ExecutionResult, ConnectorError> {
            let conn = self.0.lock().unwrap();
            conn.execute_batch(&format!(
                "CREATE OR REPLACE TEMP TABLE _agentic_tmp AS ({sql})"
            ))
            .map_err(|e| ConnectorError::query_failed(sql.to_string(), e.to_string()))?;

            let total: u64 = conn
                .query_row("SELECT COUNT(*) FROM _agentic_tmp", [], |r| r.get(0))
                .map_err(|e| ConnectorError::Other(e.to_string()))?;

            let mut stmt = conn
                .prepare(&format!("SELECT * FROM _agentic_tmp LIMIT {limit}"))
                .map_err(|e| ConnectorError::Other(e.to_string()))?;

            let col_names: Vec<String> = stmt.column_names();
            let n_cols = col_names.len();
            let rows: Vec<agentic_core::result::QueryRow> = stmt
                .query_map([], |row: &duckdb::Row<'_>| {
                    let cells = (0..n_cols)
                        .map(
                            |i| -> Result<agentic_core::result::CellValue, duckdb::Error> {
                                let v: duckdb::types::Value = row.get(i)?;
                                Ok(agentic_core::result::CellValue::Text(format!("{v:?}")))
                            },
                        )
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(agentic_core::result::QueryRow(cells))
                })
                .map_err(|e| ConnectorError::Other(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| ConnectorError::Other(e.to_string()))?;

            let _ = conn.execute_batch("DROP TABLE IF EXISTS _agentic_tmp");

            Ok(ExecutionResult {
                result: agentic_core::result::QueryResult {
                    columns: col_names,
                    rows,
                    total_row_count: total,
                    truncated: total > limit,
                },
                summary: ResultSummary {
                    row_count: total,
                    columns: vec![],
                },
            })
        }
    }

    fn make_duckdb_connector() -> RawDuckDbConnector {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE strength (
                "Date"       TIMESTAMP,
                "Start Time" VARCHAR,
                "Exercise"   VARCHAR
            );
            INSERT INTO strength VALUES
                (TIMESTAMP '2025-01-01 00:00:00', '07:30',  'Squat'),
                (TIMESTAMP '2025-01-02 00:00:00', NULL,     'Press'),
                (TIMESTAMP '2025-01-03 00:00:00', '08:00',  'Row');
            "#,
        )
        .unwrap();
        RawDuckDbConnector(std::sync::Mutex::new(conn))
    }

    const BINDER_ERROR_SQL: &str = r#"
WITH strength_sessions AS (
  SELECT
"Date" AS activity_date,
COUNT(DISTINCT COALESCE("Start Time", CAST("Date" AS TIMESTAMP))) AS session_count
  FROM strength
  WHERE "Date" >= CURRENT_DATE - INTERVAL '8 weeks'
  GROUP BY "Date"
)
SELECT activity_date, 'strength_session_count' AS metric, session_count
FROM strength_sessions
ORDER BY activity_date ASC
"#;

    #[tokio::test]
    async fn execute_method_returns_syntax_error_with_execute_back_target() {
        let mut solver = AnalyticsSolver::new(
            LlmClient::new("dummy"),
            SemanticCatalog::empty(),
            Box::new(make_duckdb_connector()),
        );
        let solution = AnalyticsSolution {
            payload: SolutionPayload::Sql(BINDER_ERROR_SQL.to_string()),
            solution_source: SolutionSource::LlmWithSemanticContext,
            connector_name: "default".to_string(),
            semantic_query: None,
        };
        let result = solver.execute(solution).await;
        assert!(
            matches!(
                result,
                Err((
                    AnalyticsError::SyntaxError { .. },
                    BackTarget::Execute(_, _)
                ))
            ),
            "execute() must return SyntaxError with BackTarget::Execute for a DuckDB binder error",
        );
    }

    #[tokio::test]
    async fn execute_handler_routes_binder_error_to_solving_not_fatal() {
        let mut solver = AnalyticsSolver::new(
            LlmClient::new("dummy"),
            SemanticCatalog::empty(),
            Box::new(make_duckdb_connector()),
        );
        let spec = make_spec();
        let solution = AnalyticsSolution {
            payload: SolutionPayload::Sql(BINDER_ERROR_SQL.to_string()),
            solution_source: SolutionSource::LlmWithSemanticContext,
            connector_name: "default".to_string(),
            semantic_query: None,
        };
        let handlers = build_analytics_handlers();
        let execute_fn = {
            let h = handlers
                .get("executing")
                .expect("executing handler must exist");
            Arc::clone(&h.execute)
        };
        let run_ctx = RunContext {
            intent: Some(make_intent()),
            spec: Some(spec),
            retry_ctx: None,
        };
        let memory = SessionMemory::new(0);
        let result = execute_fn(
            &mut solver,
            ProblemState::Executing(solution),
            &None,
            &run_ctx,
            &memory,
        )
        .await;
        assert!(
            matches!(result.errors, Some(ref e) if e.is_empty()),
            "execute handler must use the empty-sentinel Diagnosing path on DB error",
        );
        match result.state_data {
            ProblemState::Diagnosing { error, back } => {
                assert!(
                    matches!(error, AnalyticsError::SyntaxError { .. }),
                    "error must be SyntaxError, got: {error:?}",
                );
                assert!(
                    matches!(back, BackTarget::Solve(_, _)),
                    "back must be BackTarget::Solve so diagnose can route to Solving",
                );
            }
            _other => panic!("execute handler must produce Diagnosing on DB error"),
        }
    }
}

// ── VendorEngine routing tests ────────────────────────────────────────────

/// Minimal mock engine: `translate()` always succeeds; `execute()` returns
/// a single row with column `"n"` = 42.
struct OkEngine;

#[async_trait]
impl crate::engine::SemanticEngine for OkEngine {
    fn vendor_name(&self) -> &str {
        "test_vendor"
    }

    fn translate(
        &self,
        _ctx: &crate::engine::TranslationContext,
        _intent: &AnalyticsIntent,
    ) -> Result<crate::engine::VendorQuery, crate::engine::EngineError> {
        Ok(crate::engine::VendorQuery {
            payload: serde_json::json!({ "measures": ["orders.revenue"] }),
        })
    }

    async fn ping(&self) -> Result<(), crate::engine::EngineError> {
        Ok(())
    }

    async fn execute(
        &self,
        _query: &crate::engine::VendorQuery,
    ) -> Result<agentic_core::result::QueryResult, crate::engine::EngineError> {
        use agentic_core::result::{CellValue, QueryRow};
        Ok(QueryResult {
            columns: vec!["n".to_string()],
            rows: vec![QueryRow(vec![CellValue::Number(42.0)])],
            total_row_count: 1,
            truncated: false,
        })
    }
}

/// Engine whose `translate()` always returns `TranslationFailed`.
#[allow(dead_code)]
struct FailEngine;

#[async_trait]
impl crate::engine::SemanticEngine for FailEngine {
    fn vendor_name(&self) -> &str {
        "fail_vendor"
    }

    fn translate(
        &self,
        _ctx: &crate::engine::TranslationContext,
        _intent: &AnalyticsIntent,
    ) -> Result<crate::engine::VendorQuery, crate::engine::EngineError> {
        Err(crate::engine::EngineError::TranslationFailed(
            "unsupported query".into(),
        ))
    }

    async fn ping(&self) -> Result<(), crate::engine::EngineError> {
        Ok(())
    }

    async fn execute(
        &self,
        _query: &crate::engine::VendorQuery,
    ) -> Result<agentic_core::result::QueryResult, crate::engine::EngineError> {
        unreachable!("FailEngine::execute should never be called")
    }
}

// should_skip tests for vendor engine removed — solving is absorbed into specifying.

#[test]
fn solver_with_no_engine_has_none_engine_field() {
    let s = make_solver();
    assert!(
        s.engine.is_none(),
        "default solver must not have an engine set"
    );
}

// Test 9: execute_solution dispatches Vendor payload to the engine, not the SQL connector.
#[tokio::test]
async fn execute_solution_vendor_payload_dispatched_to_engine() {
    let mut s = AnalyticsSolver::new(
        LlmClient::new("dummy"),
        SemanticCatalog::empty(),
        Box::new(StubConnector),
    )
    .with_engine(std::sync::Arc::new(OkEngine));

    let vq = crate::engine::VendorQuery {
        payload: serde_json::json!({ "measures": ["orders.revenue"] }),
    };
    let solution = AnalyticsSolution {
        payload: SolutionPayload::Vendor(vq),
        solution_source: SolutionSource::VendorEngine("test_vendor".to_string()),
        connector_name: "default".to_string(),
        semantic_query: None,
    };

    let result = s
        .execute_solution(solution)
        .await
        .map_err(|(e, _)| e)
        .expect("execute should succeed");
    let primary = result.primary();
    assert_eq!(primary.data.columns, vec!["n"]);
    assert_eq!(primary.data.total_row_count, 1);
}

// Test that a Vendor payload error maps to AnalyticsError::VendorError.
#[tokio::test]
async fn execute_solution_vendor_api_error_maps_to_vendor_error() {
    struct ErrorEngine;
    #[async_trait]
    impl crate::engine::SemanticEngine for ErrorEngine {
        fn vendor_name(&self) -> &str {
            "error_vendor"
        }
        fn translate(
            &self,
            _: &crate::engine::TranslationContext,
            _: &AnalyticsIntent,
        ) -> Result<crate::engine::VendorQuery, crate::engine::EngineError> {
            unreachable!()
        }
        async fn ping(&self) -> Result<(), crate::engine::EngineError> {
            Ok(())
        }
        async fn execute(
            &self,
            _: &crate::engine::VendorQuery,
        ) -> Result<agentic_core::result::QueryResult, crate::engine::EngineError> {
            Err(crate::engine::EngineError::ApiError {
                status: 400,
                body: "bad request".into(),
            })
        }
    }

    let mut s = AnalyticsSolver::new(
        LlmClient::new("dummy"),
        SemanticCatalog::empty(),
        Box::new(StubConnector),
    )
    .with_engine(std::sync::Arc::new(ErrorEngine));

    let vq = crate::engine::VendorQuery {
        payload: serde_json::json!({}),
    };
    let solution = AnalyticsSolution {
        payload: SolutionPayload::Vendor(vq),
        solution_source: SolutionSource::VendorEngine("error_vendor".to_string()),
        connector_name: "default".to_string(),
        semantic_query: None,
    };

    let err = s.execute_solution(solution).await.unwrap_err().0;
    assert!(
        matches!(&err, AnalyticsError::VendorError { vendor_name, .. } if vendor_name == "error_vendor"),
        "expected VendorError, got: {err:?}"
    );
}

// ── SQL-generation mode (output: mode: sql) ──────────────────────────────────

/// Stub connector that succeeds on `execute_query` — used to confirm
/// the LIMIT-0 smoke check passes for syntactically valid SQL.
struct OkConnector;

#[async_trait]
impl agentic_connector::DatabaseConnector for OkConnector {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::DuckDb
    }

    async fn execute_query(
        &self,
        _sql: &str,
        _limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        Ok(ExecutionResult {
            result: agentic_core::QueryResult {
                columns: vec![],
                rows: vec![],
                total_row_count: 0,
                truncated: false,
            },
            summary: agentic_connector::ResultSummary {
                row_count: 0,
                columns: vec![],
            },
        })
    }
}

fn sql_mode_solver_with_ok_connector() -> AnalyticsSolver {
    AnalyticsSolver::new(
        LlmClient::new("dummy"),
        SemanticCatalog::empty(),
        Box::new(OkConnector),
    )
    .with_sql_generation_mode(true)
}

fn make_sql_solution(source: SolutionSource, sql: &str) -> AnalyticsSolution {
    AnalyticsSolution {
        payload: SolutionPayload::Sql(sql.to_string()),
        solution_source: source,
        connector_name: "default".to_string(),
        semantic_query: None,
    }
}

/// Regression test for a live panic observed on the split-fleet `ide` node.
///
/// The Mode 2 LLM SQL-generation fast path (`spec_to_executing` calling
/// `solve_impl` inline right after a failed semantic compile) reaches
/// `ProblemState::Executing` with `solution_source: LlmWithSemanticContext`
/// WITHOUT ever passing through `ProblemState::Solving` — the only place the
/// orchestrator loop populates `run_ctx.spec` (`run_loop.rs`: "Fields are
/// populated by the orchestrator loop after each successful state
/// transition: ... `spec` after Specifying" really means "after Solving").
/// So on the very first execution attempt down this path, `run_ctx.spec` is
/// still `None` — only a later retry through `BackTarget::Solve` populates
/// it. The executing handler used to do
/// `run_ctx.spec.clone().expect("run_ctx.spec must be set for
/// LlmWithSemanticContext path")` on a first-attempt execution failure,
/// which panics for exactly this shape of `RunContext`. It must instead
/// degrade gracefully — the same way the SemanticLayer/Automation/
/// VendorEngine arm already does when its own spec is unavailable — by
/// falling back to `BackTarget::Clarify` via `run_ctx.intent`.
#[tokio::test]
async fn executing_llm_with_semantic_context_failure_does_not_panic_when_spec_unset() {
    let mut solver = make_solver(); // StubConnector always errors on execute_query
    let handlers = build_analytics_handlers();
    let execute_fn = {
        let h = handlers
            .get("executing")
            .expect("executing handler must exist");
        std::sync::Arc::clone(&h.execute)
    };
    let solution = make_sql_solution(SolutionSource::LlmWithSemanticContext, "SELECT 1");
    // spec: None, intent: Some — exactly the RunContext shape the Mode 2 fast
    // path produces on its first attempt down this source.
    let run_ctx = make_run_ctx_with_intent();
    let memory = agentic_core::orchestrator::SessionMemory::new(0);

    let result = execute_fn(
        &mut solver,
        ProblemState::Executing(solution),
        &None,
        &run_ctx,
        &memory,
    )
    .await;

    match result.state_data {
        ProblemState::Diagnosing { back, .. } => {
            assert!(
                matches!(back, BackTarget::Clarify(_, _)),
                "expected BackTarget::Clarify (spec unavailable, fall back via \
                 run_ctx.intent) — got back-target discriminant {:?}",
                std::mem::discriminant(&back)
            );
        }
        other => panic!(
            "expected ProblemState::Diagnosing (graceful fallback, not a panic), got: {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

/// SQL-gen mode disabled — the helper short-circuits to `Disabled` so
/// the executing handler falls through to the normal execute path.
#[tokio::test]
async fn sql_gen_mode_disabled_is_passthrough() {
    let solver = make_solver();
    let solution = make_sql_solution(SolutionSource::SemanticLayer, "SELECT 1");
    let outcome = solver.terminate_with_sql_if_enabled(&solution).await;
    assert!(matches!(outcome, super::executing::SqlGenOutcome::Disabled));
}

/// Semantic-model solutions are pre-validated — SQL-gen mode terminates
/// without running execute or the LIMIT-0 smoke check.
#[tokio::test]
async fn sql_gen_mode_semantic_layer_terminates_directly() {
    let solver = sql_mode_solver_with_ok_connector();
    let solution = make_sql_solution(
        SolutionSource::SemanticLayer,
        "SELECT region, SUM(revenue) FROM orders GROUP BY region",
    );
    let outcome = solver.terminate_with_sql_if_enabled(&solution).await;
    let super::executing::SqlGenOutcome::Terminate(ans) = outcome else {
        panic!("expected Terminate");
    };
    assert!(ans.text.contains("SUM(revenue)"));
    assert!(ans.display_blocks.is_empty());
}

/// Verified `.sql` file matches are pre-validated — terminate without
/// smoke-checking.
#[tokio::test]
async fn sql_gen_mode_sql_file_terminates_directly() {
    let solver = sql_mode_solver_with_ok_connector();
    let solution = make_sql_solution(
        SolutionSource::SqlFile {
            file_path: std::path::PathBuf::from("queries/revenue.sql"),
        },
        "SELECT 1",
    );
    let outcome = solver.terminate_with_sql_if_enabled(&solution).await;
    assert!(matches!(
        outcome,
        super::executing::SqlGenOutcome::Terminate(_)
    ));
}

/// LLM-generated SQL goes through the LIMIT-0 smoke check. With an
/// always-ok connector, the smoke passes and the mode terminates.
#[tokio::test]
async fn sql_gen_mode_llm_path_runs_smoke_check_and_terminates() {
    let solver = sql_mode_solver_with_ok_connector();
    let solution = make_sql_solution(SolutionSource::LlmWithSemanticContext, "SELECT 1");
    let outcome = solver.terminate_with_sql_if_enabled(&solution).await;
    assert!(matches!(
        outcome,
        super::executing::SqlGenOutcome::Terminate(_)
    ));
}

/// LLM-generated SQL whose smoke check fails (stub connector errors)
/// surfaces `SmokeCheckFailed` so the handler can route to Diagnosing.
#[tokio::test]
async fn sql_gen_mode_llm_path_failed_smoke_returns_failure_variant() {
    let solver = make_solver().with_sql_generation_mode(true);
    let solution = make_sql_solution(SolutionSource::LlmWithSemanticContext, "SELECT bogus");
    let outcome = solver.terminate_with_sql_if_enabled(&solution).await;
    let super::executing::SqlGenOutcome::SmokeCheckFailed { message, .. } = outcome else {
        panic!("expected SmokeCheckFailed");
    };
    assert!(!message.is_empty());
}

/// Vendor engine paths produce an opaque JSON payload — no portable SQL
/// to write to a cache file. SQL-gen mode rejects them.
#[tokio::test]
async fn sql_gen_mode_vendor_path_is_rejected() {
    let solver = sql_mode_solver_with_ok_connector();
    let solution = AnalyticsSolution {
        payload: SolutionPayload::Sql("ignored".to_string()),
        solution_source: SolutionSource::VendorEngine("omni".to_string()),
        connector_name: "default".to_string(),
        semantic_query: None,
    };
    let outcome = solver.terminate_with_sql_if_enabled(&solution).await;
    let super::executing::SqlGenOutcome::IncompatiblePath { reason } = outcome else {
        panic!("expected IncompatiblePath");
    };
    assert!(reason.contains("vendor"));
}

/// Automation delegation produces no SQL until the child automation runs.
/// SQL-gen mode rejects this combination at the executing entry.
#[tokio::test]
async fn sql_gen_mode_automation_path_is_rejected() {
    let solver = sql_mode_solver_with_ok_connector();
    let solution = AnalyticsSolution {
        payload: SolutionPayload::Sql(String::new()),
        solution_source: SolutionSource::Automation {
            file_path: std::path::PathBuf::from("procedures/foo.procedure.yml"),
        },
        connector_name: "default".to_string(),
        semantic_query: None,
    };
    let outcome = solver.terminate_with_sql_if_enabled(&solution).await;
    let super::executing::SqlGenOutcome::IncompatiblePath { reason } = outcome else {
        panic!("expected IncompatiblePath");
    };
    assert!(reason.contains("automation"));
}

// ── Connector routing policy ─────────────────────────────────────────────────
//
// `resolve_solution_connector` decides whether a query runs where its view says
// it should. Getting it wrong does not fail loudly — it answers from a different
// warehouse, in a different SQL dialect, and reports the number as fact. These
// pin the three branches and the invariant the dialect fix rests on.

fn routing_connectors(
    names: &[&str],
) -> std::collections::HashMap<String, std::sync::Arc<dyn agentic_connector::DatabaseConnector>> {
    names
        .iter()
        .map(|n| {
            (
                n.to_string(),
                std::sync::Arc::new(StubConnector)
                    as std::sync::Arc<dyn agentic_connector::DatabaseConnector>,
            )
        })
        .collect()
}

#[test]
fn a_named_registered_connector_is_the_one_used() {
    let connectors = routing_connectors(&["warehouse", "lake"]);
    let picked = resolve_solution_connector(&connectors, "lake", "warehouse")
        .expect("a registered name resolves");
    // Identity by pointer: the map holds distinct Arcs per name.
    assert!(std::sync::Arc::ptr_eq(&picked, &connectors["lake"]));
}

#[test]
fn a_named_but_unregistered_connector_errors_and_lists_what_is_available() {
    let connectors = routing_connectors(&["warehouse", "lake"]);
    // `expect_err` would need Debug on the Ok type, and `dyn DatabaseConnector`
    // has none.
    let err = match resolve_solution_connector(&connectors, "july_airhouse", "warehouse") {
        Ok(_) => panic!("a query must not be rerouted off the database its view names"),
        Err(e) => e,
    };
    assert!(
        err.contains("july_airhouse"),
        "names the missing one: {err}"
    );
    // Sorted, so the message is stable across runs and diffable in a log.
    assert!(err.contains("[lake, warehouse]"), "lists available: {err}");
}

#[test]
fn only_an_empty_name_falls_back_to_the_default() {
    let connectors = routing_connectors(&["warehouse", "lake"]);
    let picked =
        resolve_solution_connector(&connectors, "", "warehouse").expect("empty name means default");
    assert!(std::sync::Arc::ptr_eq(&picked, &connectors["warehouse"]));
}

#[test]
fn a_datasource_differing_only_in_case_resolves() {
    // The execute-path twin of `a_datasource_differing_only_in_case_still_resolves`
    // in tools/tests.rs. airlayer matches a view's `datasource:` with
    // `eq_ignore_ascii_case`, so a name that resolves for airlayer must resolve
    // here -- otherwise the query is refused for a database that exists.
    let connectors = routing_connectors(&["july_airhouse", "july_warehouse"]);
    let picked = resolve_solution_connector(&connectors, "July_Airhouse", "july_warehouse")
        .expect("a case-differing name is the same database, not a missing one");
    assert!(std::sync::Arc::ptr_eq(
        &picked,
        &connectors["july_airhouse"]
    ));
}

#[test]
fn an_unregistered_default_errors_rather_than_picking_arbitrarily() {
    // The old tail was `.or_else(|| values().next())`, and HashMap order is
    // nondeterministic — so this case used to land in a different warehouse
    // depending on the process.
    let connectors = routing_connectors(&["warehouse", "lake"]);
    let err = match resolve_solution_connector(&connectors, "", "not_registered") {
        Ok(_) => panic!("an unregistered default is a misconfiguration, not a choice"),
        Err(e) => e,
    };
    assert!(err.contains("not_registered"), "{err}");
}
