//! Execution grader: re-run the agent's final SQL and the gold SQL against the
//! configured database and compare the result sets (BIRD EX + Soft-F1), then
//! judge the answer text; the judge verdict decides pass/fail.

use std::{collections::HashMap, time::Duration};

use agentic_connector::DatabaseConnector;
use agentic_pipeline::platform::ProjectContext;
use futures::stream::StreamExt;

use oxy::{
    config::{
        constants::{EVAL_METRICS_POSTFIX, EVAL_SOURCE},
        model::ExecutionSolver,
    },
    exec_types::{ProgressType, TargetOutput},
};
use oxy_shared::errors::OxyError;

use super::{
    EvalRun,
    answer_judge::{AnswerJudge, judge_answer, render_gold_table},
    execution_grade::{Cell, canonical, execution_match, soft_f1},
    execution_metric::choice_for_verdict,
    types::{Execution, MetricKind, Record, VERDICT_INCORRECT},
};

const QUERY_TIMEOUT: Duration = Duration::from_secs(60);
const ROW_CAP: usize = 100_000;

/// A query's column names and canonical rows.
struct ResultSet {
    columns: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

/// Why a grading query produced no rows. A timeout is kept apart so a gold
/// timeout reads differently from a gold error in the `SKIP` reason.
#[derive(Debug, PartialEq)]
enum FetchError {
    Timeout(String),
    Failed(String),
}

impl FetchError {
    /// A server-side cancel (`57014`, from `bird_ro`'s `statement_timeout`)
    /// is a timeout too, not a query error.
    fn from_query(e: String) -> Self {
        if e.contains("57014") || e.contains("statement timeout") {
            Self::Timeout(e)
        } else {
            Self::Failed(e)
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(e) | Self::Failed(e) => f.write_str(e),
        }
    }
}

/// The grading connection. Every pair shares one connector, which serves one
/// query at a time, so fetches go through `gate` one at a time and the 60 s
/// timeout starts only once a fetch holds it: a slow query delays the queue
/// but cannot time out the queries waiting behind it. Judge calls do not take
/// the gate and stay concurrent.
pub(super) struct GradingDb<'a> {
    conn: &'a dyn DatabaseConnector,
    gate: tokio::sync::Mutex<()>,
}

impl<'a> GradingDb<'a> {
    pub(super) fn new(conn: &'a dyn DatabaseConnector) -> Self {
        Self {
            conn,
            gate: tokio::sync::Mutex::new(()),
        }
    }

    /// Run `sql` and collect its rows as canonical cells, bounded by a 60 s
    /// timeout (covering execute + drain) and a 100 000-row cap.
    async fn fetch(&self, sql: &str) -> Result<ResultSet, FetchError> {
        let _turn = self.gate.lock().await;
        match tokio::time::timeout(QUERY_TIMEOUT, self.fetch_unbounded(sql)).await {
            Ok(result) => result,
            Err(_) => Err(FetchError::Timeout(format!(
                "timeout after {}s",
                QUERY_TIMEOUT.as_secs()
            ))),
        }
    }

    async fn fetch_unbounded(&self, sql: &str) -> Result<ResultSet, FetchError> {
        let failed = |e: String| FetchError::from_query(e);
        let mut stream = self
            .conn
            .execute_query_full(sql)
            .await
            .map_err(|e| failed(e.to_string()))?;
        let columns = stream.columns.iter().map(|c| c.name.clone()).collect();
        let mut rows = Vec::new();
        while let Some(row) = stream.rows.next().await {
            let row = row.map_err(|e| failed(e.to_string()))?;
            if rows.len() >= ROW_CAP {
                return Err(FetchError::Failed(format!("row cap {ROW_CAP} exceeded")));
            }
            rows.push(row.iter().map(canonical).collect());
        }
        Ok(ResultSet { columns, rows })
    }
}

fn base_record(actual: &TargetOutput, expected: &TargetOutput) -> Record {
    Record {
        cot: String::new(),
        choice: "FAIL".to_string(),
        score: 0.0,
        prompt: expected.task_description.clone(),
        expected: expected.final_sql.clone(),
        actual_output: Some(actual.output.clone()),
        references: actual.references.clone(),
        duration_ms: actual.duration_ms,
        input_tokens: actual.input_tokens,
        output_tokens: actual.output_tokens,
        soft_f1: Some(0.0),
        llm_rounds: actual.llm_rounds,
        final_sql: actual.final_sql.clone(),
        judge_verdict: None,
        judge_reason: None,
        answer_text: Some(actual.output.clone()),
        gold_preview: None,
        timing: actual.timing.clone(),
        runtime: actual.runtime.clone(),
        model: actual.model.clone(),
    }
}

/// Grade one (actual, expected) pair. The gold SQL runs first: if it fails the
/// case is `SKIP` (not the agent's fault), whatever the agent did, and the
/// judge is not called. Otherwise EX / Soft-F1 run, then the answer judge reads
/// the agent's answer text against the gold rows (even without agent SQL). The
/// verdict decides `choice`; EX only sets `score`.
async fn grade_pair(
    db: &GradingDb<'_>,
    judge: &dyn AnswerJudge,
    actual: &TargetOutput,
    expected: &TargetOutput,
) -> Record {
    let mut record = base_record(actual, expected);
    let gold = match fetch_gold(db, expected).await {
        Ok(set) => set,
        Err(e) => {
            record.choice = "SKIP".to_string();
            record.soft_f1 = None;
            record.cot = gold_skip_reason(e);
            return record;
        }
    };
    grade_ex(db, actual, &gold.rows, &mut record).await;
    let preview = render_gold_table(&gold.columns, &gold.rows);
    let question = expected.task_description.as_deref().unwrap_or("");
    let judgement = judge_answer(judge, question, &actual.output, &preview).await;
    record.choice = choice_for_verdict(judgement.verdict).to_string();
    record.judge_verdict = Some(judgement.verdict.to_string());
    record.judge_reason = Some(judgement.reason);
    record.gold_preview = Some(preview);
    record
}

async fn fetch_gold(db: &GradingDb<'_>, expected: &TargetOutput) -> Result<ResultSet, FetchError> {
    match expected.final_sql.as_deref() {
        Some(sql) => db.fetch(sql).await,
        None => Err(FetchError::Failed("no gold sql".to_string())),
    }
}

/// The `cot` of a `SKIP` record: a gold timeout reads apart from a gold error.
fn gold_skip_reason(e: FetchError) -> String {
    match e {
        FetchError::Timeout(e) => format!("gold timeout: {e}"),
        FetchError::Failed(e) => format!("gold sql failed: {e}"),
    }
}

/// EX + Soft-F1: run the agent's final SQL and compare with the gold rows.
/// Sets `score` (the EX bit), `soft_f1` and `cot`; never `choice`.
async fn grade_ex(
    db: &GradingDb<'_>,
    actual: &TargetOutput,
    gold: &[Vec<Cell>],
    record: &mut Record,
) {
    let Some(sql) = actual.final_sql.as_deref() else {
        record.cot = "agent produced no successful query".to_string();
        return;
    };
    match db.fetch(sql).await {
        Err(e) => record.cot = format!("agent sql failed: {e}"),
        Ok(ResultSet { rows, .. }) => {
            let pass = execution_match(&rows, gold);
            record.soft_f1 = Some(soft_f1(&rows, gold) as f32);
            record.score = if pass { 1.0 } else { 0.0 };
            record.cot = if pass {
                "result sets match".to_string()
            } else {
                format!(
                    "result sets differ (agent {} rows, gold {} rows)",
                    rows.len(),
                    gold.len()
                )
            };
        }
    }
}

async fn resolve_conn(
    project_ctx: &oxy_workspace_runtime::project_ctx::OxyProjectContext,
    database: &str,
) -> Result<Box<dyn DatabaseConnector>, OxyError> {
    let cfg = project_ctx
        .resolve_connector(database)
        .await
        .ok_or_else(|| {
            OxyError::ConfigurationError(format!(
                "execution grader: database '{database}' not found or has no connector"
            ))
        })?;
    agentic_connector::build_connector_async(cfg)
        .await
        .map_err(|e| {
            OxyError::DBError(format!(
                "execution grader: failed to connect to '{database}': {e}"
            ))
        })
}

fn failed_run_record(error_msg: &str, expected: &TargetOutput) -> Record {
    Record {
        cot: format!("Run failed with error: {error_msg}"),
        choice: "FAIL".to_string(),
        score: 0.0,
        prompt: expected.task_description.clone(),
        expected: expected.final_sql.clone(),
        actual_output: Some(format!("[ERROR] {error_msg}")),
        references: vec![],
        duration_ms: 0.0,
        input_tokens: 0,
        output_tokens: 0,
        soft_f1: Some(0.0),
        llm_rounds: 0,
        final_sql: None,
        judge_verdict: Some(VERDICT_INCORRECT.to_string()),
        judge_reason: Some("run failed; no answer to judge".to_string()),
        answer_text: None,
        gold_preview: None,
        timing: None,
        runtime: None,
        model: None,
    }
}

/// Records for runs that errored before answering. Like `grade_pair`, the
/// gold SQL decides first: a case whose gold fails is `SKIP` (outside the
/// denominator) for every run of it; otherwise the errored run is a `FAIL`
/// judged `INCORRECT`. The gold runs once per case, however many runs errored.
async fn grade_errored_runs(
    db: &GradingDb<'_>,
    errors_with_expected: &[(String, TargetOutput)],
) -> Vec<Record> {
    let mut gold_failures: HashMap<Option<&str>, Option<String>> = HashMap::new();
    let mut records = Vec::with_capacity(errors_with_expected.len());
    for (msg, expected) in errors_with_expected {
        let key = expected.final_sql.as_deref();
        let failure = match gold_failures.get(&key) {
            Some(failure) => failure.clone(),
            None => {
                let failure = fetch_gold(db, expected).await.err().map(gold_skip_reason);
                gold_failures.insert(key, failure.clone());
                failure
            }
        };
        let mut record = failed_run_record(msg, expected);
        if let Some(reason) = failure {
            record.choice = "SKIP".to_string();
            record.soft_f1 = None;
            record.judge_verdict = None;
            record.judge_reason = None;
            record.cot = format!("{reason} (run failed with error: {msg})");
        }
        records.push(record);
    }
    records
}

pub(super) async fn run_execution_solver(
    ctx: &EvalRun,
    solver: ExecutionSolver,
    outputs: Vec<(TargetOutput, TargetOutput)>,
    errors_with_expected: Vec<(String, TargetOutput)>,
    concurrency: usize,
) -> Result<MetricKind, OxyError> {
    let project_ctx = ctx.project_ctx();
    let conn = resolve_conn(&project_ctx, &solver.database).await?;
    // Same judge-model resolution as the correctness solver: `judge_model`,
    // else the project default; resolved once, fails fast before grading.
    let judge = agentic_pipeline::prepare_one_shot(
        &*project_ctx,
        solver.model_ref.as_deref(),
        "eval-answer-judge",
    )
    .await
    .map_err(OxyError::RuntimeError)?;
    let judge_model = judge.model().to_string();
    let metric_ctx = ctx.with_child_source(
        format!("{}-{}", ctx.source.id, EVAL_METRICS_POSTFIX),
        EVAL_SOURCE.to_string(),
    );
    let db = GradingDb::new(conn.as_ref());
    let mut records = grade_all(&db, &judge, outputs, concurrency, &metric_ctx).await;
    records.extend(grade_errored_runs(&db, &errors_with_expected).await);
    Ok(MetricKind::Execution(
        Execution::from_records(records).with_judge_model(Some(judge_model)),
    ))
}

/// Grade every pair, `concurrency` at a time (database fetches still go one
/// at a time through `db`), reporting progress on `metric_ctx`.
async fn grade_all(
    db: &GradingDb<'_>,
    judge: &dyn AnswerJudge,
    outputs: Vec<(TargetOutput, TargetOutput)>,
    concurrency: usize,
    metric_ctx: &EvalRun,
) -> Vec<Record> {
    if let Err(e) = metric_ctx
        .write_progress(ProgressType::Started(Some(outputs.len())))
        .await
    {
        tracing::warn!("eval: failed to emit grading progress Started: {e}");
    }
    let mut stream = futures::stream::iter(outputs)
        .map(|(actual, expected)| async move { grade_pair(db, judge, &actual, &expected).await })
        .buffered(concurrency.max(1));
    let mut records = Vec::new();
    while let Some(record) = stream.next().await {
        if let Err(e) = metric_ctx.write_progress(ProgressType::Updated(1)).await {
            tracing::warn!("eval: failed to emit grading progress Updated: {e}");
        }
        records.push(record);
    }
    if let Err(e) = metric_ctx.write_progress(ProgressType::Finished).await {
        tracing::warn!("eval: failed to emit grading progress Finished: {e}");
    }
    records
}

#[cfg(test)]
#[path = "execution_solver_tests.rs"]
mod tests;
