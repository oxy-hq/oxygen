//! Integration tests: full orchestrator pipeline backed by an in-memory SQLite database.
//!
//! Three scenarios are exercised end-to-end:
//!
//! 1. **Happy path** — a natural-language question flows through all five stages
//!    (clarify → specify → solve → execute → interpret) and the final answer
//!    contains actual numbers read from SQLite.
//!
//! 2. **Follow-up via prior intent** — the `solve` stage signals that its initial
//!    spec is insufficient and fires `BackTarget::Specify(spec.intent().clone())`.
//!    `diagnose` routes back to `Specifying`; the orchestrator re-enters `specify`
//!    with the recovered (prior) intent.  The test asserts that both `specify` and
//!    `solve` were called twice and that the final count matches the real SQLite row.
//!
//! 3. **Ambiguous column triggers a back-edge** — `specify` detects that the
//!    unqualified column `"status"` appears in all three tables, emits
//!    `AmbiguousColumn`, and `diagnose` routes back to `Clarifying`.  On the
//!    second `clarify` call the intent is refined with a fully-qualified reference
//!    (`orders.status`).  The test verifies that `clarify` and `specify` were each
//!    called twice and that the answer reflects real SQLite data.

use std::sync::{Arc, Mutex};

use agentic_analytics::{
    AnalyticsAnswer, AnalyticsDomain, AnalyticsError, AnalyticsIntent, AnalyticsResult,
    AnalyticsSolution, QuerySpec, QuestionType, ResultShape, SolutionPayload, SolutionSource,
};
use agentic_core::{
    BackTarget, CellValue, DomainSolver, Orchestrator, ProblemState, QueryResult, QueryRow,
};
use async_trait::async_trait;
use rusqlite::Connection;

// ─── SQLite fixture ───────────────────────────────────────────────────────────

/// Create and populate an in-memory SQLite database with three tables.
///
/// The `status` column appears in **all three** tables so that test 3 can
/// exercise ambiguous-column detection without any additional setup.
fn setup_db() -> Arc<Mutex<Connection>> {
    let conn = Connection::open_in_memory().expect("in-memory SQLite");

    conn.execute_batch(
        "
        CREATE TABLE regions (
            region_id   INTEGER PRIMARY KEY,
            region_name TEXT NOT NULL,
            status      TEXT NOT NULL DEFAULT 'active'
        );

        CREATE TABLE products (
            product_id   INTEGER PRIMARY KEY,
            product_name TEXT NOT NULL,
            category     TEXT NOT NULL,
            price        REAL NOT NULL,
            status       TEXT NOT NULL DEFAULT 'active'
        );

        CREATE TABLE orders (
            order_id    INTEGER PRIMARY KEY,
            customer_id INTEGER NOT NULL,
            amount      REAL    NOT NULL,
            region_id   INTEGER NOT NULL,
            product_id  INTEGER NOT NULL,
            status      TEXT    NOT NULL DEFAULT 'completed',
            order_date  TEXT    NOT NULL
        );

        INSERT INTO regions VALUES (1, 'North', 'active');
        INSERT INTO regions VALUES (2, 'South', 'active');
        INSERT INTO regions VALUES (3, 'West',  'active');

        INSERT INTO products VALUES (1, 'Widget A', 'hardware', 9.99,  'active');
        INSERT INTO products VALUES (2, 'Widget B', 'software', 19.99, 'active');
        INSERT INTO products VALUES (3, 'Gadget C', 'hardware', 49.99, 'active');

        INSERT INTO orders VALUES (1, 101, 250.00, 1, 1, 'completed', '2024-01-15');
        INSERT INTO orders VALUES (2, 102, 180.00, 2, 2, 'completed', '2024-01-20');
        INSERT INTO orders VALUES (3, 103, 320.00, 1, 3, 'completed', '2024-02-01');
        INSERT INTO orders VALUES (4, 104,  90.00, 3, 1, 'completed', '2024-02-10');
        INSERT INTO orders VALUES (5, 105, 410.00, 2, 2, 'completed', '2024-02-15');
        ",
    )
    .expect("populate SQLite fixture");

    Arc::new(Mutex::new(conn))
}

/// Execute `sql` against the shared connection and return a `QueryResult`.
fn run_query(db: &Arc<Mutex<Connection>>, sql: &str) -> Result<QueryResult, String> {
    use rusqlite::types::Value;

    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let col_count = stmt.column_count();
    let column_names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();

    let rows = stmt
        .query_map([], |row| {
            let cells = (0..col_count)
                .map(|i| -> rusqlite::Result<CellValue> {
                    Ok(match row.get::<_, Value>(i)? {
                        Value::Text(s) => CellValue::Text(s),
                        Value::Real(f) => CellValue::Number(f),
                        Value::Integer(n) => CellValue::Number(n as f64),
                        _ => CellValue::Null,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(QueryRow(cells))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, rusqlite::Error>>()
        .map_err(|e| e.to_string())?;

    let total_row_count = rows.len() as u64;
    Ok(QueryResult {
        columns: column_names,
        rows,
        total_row_count,
        truncated: false,
    })
}

// 1. Happy path — full pipeline with real SQLite data

struct FullPipelineSolver {
    db: Arc<Mutex<Connection>>,
}

#[async_trait]
impl DomainSolver<AnalyticsDomain> for FullPipelineSolver {
    async fn clarify(
        &mut self,
        intent: AnalyticsIntent,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsIntent, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        Ok(AnalyticsIntent {
            raw_question: intent.raw_question,
            summary: String::new(),
            question_type: QuestionType::Breakdown,
            metrics: vec!["amount".into()],
            dimensions: vec!["region_name".into()],
            filters: vec![],
            history: intent.history,
            spec_hint: None,
            selected_automation: None,
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        })
    }

    async fn specify_single(
        &mut self,
        intent: AnalyticsIntent,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<QuerySpec, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        Ok(QuerySpec {
            intent,
            resolved_metrics: vec!["orders.amount".into()],
            resolved_tables: vec!["orders".into(), "regions".into()],
            join_path: vec![("orders".into(), "regions".into(), "region_id".into())],
            expected_result_shape: ResultShape::Table {
                columns: vec!["region_name".into(), "total_revenue".into()],
            },
            resolved_filters: vec![],
            assumptions: vec![],
            solution_source: Default::default(),
            precomputed: None,
            context: None,
            connector_name: String::new(),
            query_request_item: None,
            query_request: None,
            compile_error: None,
        })
    }

    async fn solve(
        &mut self,
        _spec: QuerySpec,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsSolution, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        Ok(AnalyticsSolution {
            payload: SolutionPayload::Sql(
                "SELECT r.region_name, SUM(o.amount) AS total_revenue \
                  FROM orders o \
                  JOIN regions r ON o.region_id = r.region_id \
                  GROUP BY r.region_name \
                  ORDER BY total_revenue DESC"
                    .into(),
            ),
            solution_source: Default::default(),
            connector_name: String::new(),
            semantic_query: None,
        })
    }

    async fn execute(
        &mut self,
        solution: AnalyticsSolution,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsResult, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let sql = solution
            .payload
            .sql()
            .expect("test solver only handles Sql payloads")
            .to_string();
        match run_query(&self.db, &sql) {
            Ok(data) => Ok(AnalyticsResult::single(data, None)),
            Err(e) => Err((
                AnalyticsError::SyntaxError {
                    query: sql,
                    message: e,
                },
                BackTarget::Execute(solution, Default::default()),
            )),
        }
    }

    async fn interpret(
        &mut self,
        result: AnalyticsResult,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsAnswer, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let mut lines = vec!["Revenue by region:".to_string()];
        for row in &result.primary().data.rows {
            let region = match row.0.first() {
                Some(CellValue::Text(s)) => s.clone(),
                _ => "?".into(),
            };
            let revenue = match row.0.get(1) {
                Some(CellValue::Number(n)) => *n,
                _ => 0.0,
            };
            lines.push(format!("  {region}: ${revenue:.2}"));
        }
        Ok(AnalyticsAnswer {
            display_blocks: vec![],
            text: lines.join("\n"),
            spec_hint: None,
        })
    }

    async fn diagnose(
        &mut self,
        error: AnalyticsError,
        _back: BackTarget<AnalyticsDomain>,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
    ) -> Result<ProblemState<AnalyticsDomain>, AnalyticsError> {
        Err(error)
    }
}

/// Full pipeline with a real SQLite backend.
///
/// Data: North = 250 + 320 = $570, South = 180 + 410 = $590, West = $90.
#[tokio::test]
async fn full_pipeline_with_sqlite_returns_real_data() {
    let db = setup_db();
    let mut orch = Orchestrator::<AnalyticsDomain, _>::new(FullPipelineSolver { db });

    let answer = orch
        .run(AnalyticsIntent {
            raw_question: "What is total revenue by region?".into(),
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
        })
        .await
        .expect("pipeline must complete");

    // South has the highest revenue ($590).
    assert!(
        answer.text.contains("South"),
        "answer must name the South region: {}",
        answer.text,
    );
    assert!(
        answer.text.contains("590"),
        "answer must include South's total $590: {}",
        answer.text,
    );
    // North is second ($570).
    assert!(
        answer.text.contains("North"),
        "answer must name the North region: {}",
        answer.text,
    );
    assert!(
        answer.text.contains("570"),
        "answer must include North's total $570: {}",
        answer.text,
    );
    // West appears with $90.
    assert!(
        answer.text.contains("West"),
        "answer must name the West region: {}",
        answer.text,
    );
}

// 2. Follow-up via prior intent — HasIntent recovery across a back-edge

struct PriorIntentSolver {
    db: Arc<Mutex<Connection>>,
    /// Tracks how many times each stage was reached.
    specify_calls: u32,
    solve_calls: u32,
    /// The raw question recovered from the spec's intent on the second specify call.
    recovered_question: Option<String>,
}

#[async_trait]
impl DomainSolver<AnalyticsDomain> for PriorIntentSolver {
    async fn clarify(
        &mut self,
        intent: AnalyticsIntent,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsIntent, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        Ok(AnalyticsIntent {
            raw_question: intent.raw_question,
            summary: String::new(),
            question_type: QuestionType::SingleValue,
            metrics: vec!["order_count".into()],
            dimensions: vec![],
            filters: vec![],
            history: intent.history,
            spec_hint: None,
            selected_automation: None,
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        })
    }

    async fn specify_single(
        &mut self,
        intent: AnalyticsIntent,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<QuerySpec, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        self.specify_calls += 1;
        // Capture the question received on this call so the test can verify
        // the prior intent was correctly threaded through the back-edge.
        self.recovered_question = Some(intent.raw_question.clone());

        // First call: use a deliberately wrong metric so `solve` can reject it.
        // Second call (after the back-edge): use the correct metric.
        let metric = if self.specify_calls == 1 {
            "nonexistent_metric"
        } else {
            "COUNT(*)"
        };

        Ok(QuerySpec {
            intent,
            resolved_metrics: vec![metric.into()],
            resolved_tables: vec!["orders".into()],
            join_path: vec![],
            expected_result_shape: ResultShape::Scalar,
            resolved_filters: vec![],
            assumptions: vec![],
            solution_source: Default::default(),
            precomputed: None,
            context: None,
            connector_name: String::new(),
            query_request_item: None,
            query_request: None,
            compile_error: None,
        })
    }

    async fn solve(
        &mut self,
        spec: QuerySpec,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsSolution, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        self.solve_calls += 1;
        if self.solve_calls == 1 {
            // The spec is insufficient.  Fire a back-edge to Specify so the spec can be rebuilt.
            let prior_intent = spec.intent.clone();
            return Err((
                AnalyticsError::UnresolvedMetric {
                    metric: "nonexistent_metric".into(),
                },
                BackTarget::Specify(prior_intent, Default::default()),
            ));
        }
        // Second attempt: generate correct SQL from the refined spec.
        Ok(AnalyticsSolution {
            payload: SolutionPayload::Sql("SELECT COUNT(*) AS order_count FROM orders".into()),
            solution_source: Default::default(),
            connector_name: String::new(),
            semantic_query: None,
        })
    }

    async fn execute(
        &mut self,
        solution: AnalyticsSolution,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsResult, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let sql = solution
            .payload
            .sql()
            .expect("test solver only handles Sql payloads")
            .to_string();
        match run_query(&self.db, &sql) {
            Ok(data) => Ok(AnalyticsResult::single(data, None)),
            Err(e) => Err((
                AnalyticsError::SyntaxError {
                    query: sql,
                    message: e,
                },
                BackTarget::Execute(solution, Default::default()),
            )),
        }
    }

    async fn interpret(
        &mut self,
        result: AnalyticsResult,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsAnswer, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let count = result
            .primary()
            .data
            .rows
            .first()
            .and_then(|r| r.0.first())
            .map(|c| match c {
                CellValue::Number(n) => *n as u64,
                _ => 0,
            })
            .unwrap_or(0);
        Ok(AnalyticsAnswer {
            display_blocks: vec![],
            text: format!("There are {count} orders in total."),
            spec_hint: None,
        })
    }

    async fn diagnose(
        &mut self,
        error: AnalyticsError,
        back: BackTarget<AnalyticsDomain>,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
    ) -> Result<ProblemState<AnalyticsDomain>, AnalyticsError> {
        match error {
            // Route UnresolvedMetric back to Specifying with the recovered intent.
            AnalyticsError::UnresolvedMetric { .. } => match back {
                BackTarget::Specify(intent, _) => Ok(ProblemState::Specifying(intent)),
                _ => Err(error),
            },
            _ => Err(error),
        }
    }
}

/// A failed `solve` fires `BackTarget::Specify(spec.intent().clone())`.
/// `diagnose` routes back to `Specifying` with the recovered (prior) intent.
/// The test asserts that both `specify` and `solve` ran twice and that the
/// final answer contains the real row count from SQLite (5 orders).
#[tokio::test]
async fn follow_up_reuses_prior_intent_via_has_intent() {
    let db = setup_db();
    let mut orch = Orchestrator::<AnalyticsDomain, _>::new(PriorIntentSolver {
        db,
        specify_calls: 0,
        solve_calls: 0,
        recovered_question: None,
    });

    let answer = orch
        .run(AnalyticsIntent {
            raw_question: "How many orders were placed?".into(),
            summary: String::new(),
            question_type: QuestionType::SingleValue,
            metrics: vec![],
            dimensions: vec![],
            filters: vec![],
            history: vec![],
            spec_hint: None,
            selected_automation: None,
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        })
        .await
        .expect("pipeline must succeed after the back-edge retry");

    let solver = orch.into_solver();

    // specify must run twice: once producing the bad spec, once producing the good one.
    assert_eq!(
        solver.specify_calls, 2,
        "specify must be called twice (initial + after back-edge)",
    );
    // solve must run twice: once failing, once succeeding.
    assert_eq!(solver.solve_calls, 2, "solve must be called twice");

    // The raw_question must survive the back-edge intact — this is the "prior intent".
    assert_eq!(
        solver.recovered_question.as_deref(),
        Some("How many orders were placed?"),
        "prior intent question must be preserved across the back-edge",
    );

    // The answer must contain the actual count read from SQLite (5 rows).
    assert!(
        answer.text.contains('5'),
        "answer must include the real order count (5): {}",
        answer.text,
    );
}

// 3. Ambiguous column — back-edge to Clarifying for disambiguation

struct AmbiguousColumnSolver {
    db: Arc<Mutex<Connection>>,
    clarify_calls: u32,
    specify_calls: u32,
}

#[async_trait]
impl DomainSolver<AnalyticsDomain> for AmbiguousColumnSolver {
    async fn clarify(
        &mut self,
        intent: AnalyticsIntent,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsIntent, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        self.clarify_calls += 1;
        if self.clarify_calls == 1 {
            // First pass: return intent with an unqualified "status" filter.
            Ok(AnalyticsIntent {
                raw_question: intent.raw_question,
                summary: String::new(),
                question_type: QuestionType::SingleValue,
                metrics: vec!["order_count".into()],
                dimensions: vec![],
                // "status" exists in orders, regions, and products — deliberately ambiguous.
                filters: vec!["status = 'completed'".into()],
                history: intent.history.clone(),
                spec_hint: None,
                selected_automation: None,
                semantic_query: Default::default(),
                semantic_confidence: 0.0,
            })
        } else {
            // Second pass (after the AmbiguousColumn back-edge): qualify the column.
            Ok(AnalyticsIntent {
                raw_question: intent.raw_question,
                summary: String::new(),
                question_type: QuestionType::SingleValue,
                metrics: vec!["order_count".into()],
                dimensions: vec![],
                filters: vec!["orders.status = 'completed'".into()],
                history: intent.history,
                spec_hint: None,
                selected_automation: None,
                semantic_query: Default::default(),
                semantic_confidence: 0.0,
            })
        }
    }

    async fn specify_single(
        &mut self,
        intent: AnalyticsIntent,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<QuerySpec, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        self.specify_calls += 1;

        // Detect unqualified "status" filter — ambiguous across all three tables.
        for filter in &intent.filters {
            if filter.starts_with("status ") || filter.starts_with("status=") {
                return Err((
                    AnalyticsError::AmbiguousColumn {
                        column: "status".into(),
                        tables: vec!["orders".into(), "regions".into(), "products".into()],
                    },
                    // Carry the intent back so diagnose can route to Clarifying.
                    BackTarget::Specify(intent, Default::default()),
                ));
            }
        }

        // Qualified filter — no ambiguity; build the spec.
        Ok(QuerySpec {
            intent,
            resolved_metrics: vec!["orders.order_id".into()],
            resolved_tables: vec!["orders".into()],
            join_path: vec![],
            expected_result_shape: ResultShape::Scalar,
            resolved_filters: vec![],
            assumptions: vec![],
            solution_source: Default::default(),
            precomputed: None,
            context: None,
            connector_name: String::new(),
            query_request_item: None,
            query_request: None,
            compile_error: None,
        })
    }

    async fn solve(
        &mut self,
        _spec: QuerySpec,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsSolution, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        Ok(AnalyticsSolution {
            payload: SolutionPayload::Sql(
                "SELECT COUNT(*) AS cnt FROM orders WHERE status = 'completed'".into(),
            ),
            solution_source: Default::default(),
            connector_name: String::new(),
            semantic_query: None,
        })
    }

    async fn execute(
        &mut self,
        solution: AnalyticsSolution,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsResult, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let sql = solution
            .payload
            .sql()
            .expect("test solver only handles Sql payloads")
            .to_string();
        match run_query(&self.db, &sql) {
            Ok(data) => Ok(AnalyticsResult::single(data, None)),
            Err(e) => Err((
                AnalyticsError::SyntaxError {
                    query: sql,
                    message: e,
                },
                BackTarget::Execute(solution, Default::default()),
            )),
        }
    }

    async fn interpret(
        &mut self,
        result: AnalyticsResult,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsAnswer, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        let count = result
            .primary()
            .data
            .rows
            .first()
            .and_then(|r| r.0.first())
            .map(|c| match c {
                CellValue::Number(n) => *n as u64,
                _ => 0,
            })
            .unwrap_or(0);
        Ok(AnalyticsAnswer {
            display_blocks: vec![],
            text: format!("{count} completed orders found."),
            spec_hint: None,
        })
    }

    async fn diagnose(
        &mut self,
        error: AnalyticsError,
        back: BackTarget<AnalyticsDomain>,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
    ) -> Result<ProblemState<AnalyticsDomain>, AnalyticsError> {
        match &error {
            // AmbiguousColumn → route back to Clarifying with the original intent.
            AnalyticsError::AmbiguousColumn { .. } => match back {
                BackTarget::Clarify(i, _) | BackTarget::Specify(i, _) => {
                    Ok(ProblemState::Clarifying(i))
                }
                _ => Err(error),
            },
            _ => Err(error),
        }
    }
}

/// `specify` detects that the column `"status"` is present in `orders`,
/// `regions`, and `products`, returns `AmbiguousColumn`, and `diagnose` routes
/// back to `Clarifying`.  On the second `clarify` call the intent is refined
/// with a table-qualified filter; `specify` then succeeds and the pipeline
/// completes with the real SQLite count (all 5 sample orders are 'completed').
#[tokio::test]
async fn ambiguous_column_triggers_back_edge_to_clarifying() {
    let db = setup_db();
    let mut orch = Orchestrator::<AnalyticsDomain, _>::new(AmbiguousColumnSolver {
        db,
        clarify_calls: 0,
        specify_calls: 0,
    });

    let answer = orch
        .run(AnalyticsIntent {
            raw_question: "How many completed orders are there?".into(),
            summary: String::new(),
            question_type: QuestionType::SingleValue,
            metrics: vec![],
            dimensions: vec![],
            filters: vec!["status = 'completed'".into()],
            history: vec![],
            spec_hint: None,
            selected_automation: None,
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        })
        .await
        .expect("pipeline must succeed after column disambiguation");

    let solver = orch.into_solver();

    // clarify must run twice: once producing the ambiguous filter,
    // once (after the back-edge) producing the qualified filter.
    assert_eq!(
        solver.clarify_calls, 2,
        "clarify must be called twice: first with ambiguous column, then with qualified column",
    );
    // specify must run twice: first detecting the ambiguity, second succeeding.
    assert_eq!(
        solver.specify_calls, 2,
        "specify must be called twice: first AmbiguousColumn, then qualified filter",
    );
    // The answer must include the real count from SQLite (all 5 orders are 'completed').
    assert!(
        answer.text.contains('5'),
        "answer must contain the actual SQLite count (5): {}",
        answer.text,
    );
}

// 4. Selected automation — automation path bypasses LLM spec resolution

struct AutomationSolver {
    specify_calls: u32,
    solve_calls: u32,
    /// Set to true when execute receives an Automation-sourced solution.
    automation_executed: bool,
}

#[async_trait]
impl DomainSolver<AnalyticsDomain> for AutomationSolver {
    async fn clarify(
        &mut self,
        intent: AnalyticsIntent,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsIntent, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        // Simulate the Ground sub-phase selecting an automation.
        Ok(AnalyticsIntent {
            raw_question: intent.raw_question,
            summary: String::new(),
            question_type: QuestionType::SingleValue,
            metrics: vec![],
            dimensions: vec![],
            filters: vec![],
            history: intent.history,
            spec_hint: None,
            selected_automation: Some(std::path::PathBuf::from(
                "workflows/monthly_sales.procedure.yml",
            )),
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        })
    }

    async fn specify_single(
        &mut self,
        intent: AnalyticsIntent,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<QuerySpec, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        self.specify_calls += 1;
        // Mirrors the AnalyticsSolver::specify_impl short-circuit: when
        // selected_automation is set, skip LLM resolution and return a
        // minimal spec with SolutionSource::Automation.
        let file_path = intent
            .selected_automation
            .clone()
            .expect("selected_automation must be set when automation path is taken");
        Ok(QuerySpec {
            solution_source: SolutionSource::Automation { file_path },
            resolved_metrics: vec![],
            resolved_filters: vec![],
            resolved_tables: vec![],
            join_path: vec![],
            expected_result_shape: ResultShape::Table { columns: vec![] },
            assumptions: vec![],
            precomputed: None,
            context: None,
            connector_name: String::new(),
            query_request_item: None,
            query_request: None,
            compile_error: None,
            intent,
        })
    }

    async fn solve(
        &mut self,
        spec: QuerySpec,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<agentic_analytics::AnalyticsSolution, (AnalyticsError, BackTarget<AnalyticsDomain>)>
    {
        self.solve_calls += 1;
        // Note: AnalyticsSolver.should_skip() bypasses this stage for
        // SolutionSource::Automation, but stubs always call solve since
        // they don't override should_skip. Propagate the automation source.
        let file_path = match &spec.solution_source {
            SolutionSource::Automation { file_path } => file_path.clone(),
            _ => panic!("solve must receive an Automation-sourced spec"),
        };
        Ok(agentic_analytics::AnalyticsSolution {
            payload: SolutionPayload::Sql(String::new()),
            solution_source: SolutionSource::Automation { file_path },
            connector_name: String::new(),
            semantic_query: None,
        })
    }

    async fn execute(
        &mut self,
        solution: agentic_analytics::AnalyticsSolution,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<AnalyticsResult, (AnalyticsError, BackTarget<AnalyticsDomain>)> {
        // Verify the executor receives an automation-sourced solution.
        assert!(
            matches!(solution.solution_source, SolutionSource::Automation { .. }),
            "execute must receive SolutionSource::Automation, got {:?}",
            solution.solution_source,
        );
        self.automation_executed = true;

        // Simulate automation output: a JSON result from the runner.
        Ok(AnalyticsResult::single(
            QueryResult {
                columns: vec!["result".to_string()],
                rows: vec![QueryRow(vec![CellValue::Text(
                    r#"{"total_sales": 1250.00, "month": "2024-01"}"#.to_string(),
                )])],
                total_row_count: 1,
                truncated: false,
            },
            None,
        ))
    }

    async fn interpret(
        &mut self,
        result: AnalyticsResult,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
        _memory: &agentic_core::orchestrator::SessionMemory<AnalyticsDomain>,
    ) -> Result<agentic_analytics::AnalyticsAnswer, (AnalyticsError, BackTarget<AnalyticsDomain>)>
    {
        let text = result
            .primary()
            .data
            .rows
            .first()
            .and_then(|r| r.0.first())
            .map(|c| match c {
                CellValue::Text(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();
        Ok(agentic_analytics::AnalyticsAnswer {
            display_blocks: vec![],
            text: format!("Automation result: {text}"),
            spec_hint: None,
        })
    }

    async fn diagnose(
        &mut self,
        error: AnalyticsError,
        _back: BackTarget<AnalyticsDomain>,
        _ctx: &agentic_core::orchestrator::RunContext<AnalyticsDomain>,
    ) -> Result<ProblemState<AnalyticsDomain>, AnalyticsError> {
        Err(error)
    }
}

/// When `clarify` sets `selected_automation`, the pipeline must:
/// 1. Call `specify_single` exactly once — it short-circuits LLM resolution
///    and returns a `SolutionSource::Automation` spec.
/// 2. Route `execute` with a `SolutionSource::Automation` solution so the
///    automation runner would be invoked.
/// 3. Return the automation output in the final answer.
#[tokio::test]
async fn selected_automation_routes_through_automation_execution_path() {
    let mut orch = Orchestrator::<AnalyticsDomain, _>::new(AutomationSolver {
        specify_calls: 0,
        solve_calls: 0,
        automation_executed: false,
    });

    let answer = orch
        .run(AnalyticsIntent {
            raw_question: "Show me the monthly sales summary.".into(),
            summary: String::new(),
            question_type: QuestionType::SingleValue,
            metrics: vec![],
            dimensions: vec![],
            filters: vec![],
            history: vec![],
            spec_hint: None,
            selected_automation: None,
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        })
        .await
        .expect("pipeline must complete for automation path");

    let solver = orch.into_solver();

    // specify_single must be called — it handles the short-circuit itself.
    assert_eq!(
        solver.specify_calls, 1,
        "specify_single must be called once for the automation path",
    );
    // execute must have received an Automation-sourced solution.
    assert!(
        solver.automation_executed,
        "execute must be called with SolutionSource::Automation",
    );
    // The answer must include the automation output JSON.
    assert!(
        answer.text.contains("1250"),
        "answer must contain the automation result (1250): {}",
        answer.text,
    );
}
