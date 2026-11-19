use std::collections::HashMap;

use agentic_connector::{ConnectorError, SqlDialect};
use agentic_core::result::{ColumnSpec, TypedDataType, TypedRowStream, TypedValue};
use async_trait::async_trait;

use super::*;
use crate::integrations::eval::builders::{
    answer_judge::tests::{FakeJudge, Step},
    types::{VERDICT_CORRECT, VERDICT_JUDGE_ERROR},
};

enum Canned {
    Rows(Vec<Vec<TypedValue>>),
    Error(&'static str),
    Hang,
    /// Rows after a delay, holding the connection the way the real connector does.
    Slow(u64, Vec<Vec<TypedValue>>),
}

/// Serves one query at a time, like the Postgres connector's client mutex,
/// and counts the queries it was sent.
struct FakeConn(
    HashMap<&'static str, Canned>,
    tokio::sync::Mutex<()>,
    std::sync::atomic::AtomicUsize,
);

impl FakeConn {
    fn new(map: HashMap<&'static str, Canned>) -> Self {
        Self(map, tokio::sync::Mutex::new(()), Default::default())
    }

    fn queries(&self) -> usize {
        self.2.load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn stream_of(rows: &[Vec<TypedValue>]) -> TypedRowStream {
    TypedRowStream::from_rows(
        vec![ColumnSpec {
            name: "c".into(),
            data_type: TypedDataType::Int64,
        }],
        rows.iter().cloned().map(Ok).collect(),
    )
}

#[async_trait]
impl DatabaseConnector for FakeConn {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }
    async fn execute_query(
        &self,
        _sql: &str,
        _limit: u64,
    ) -> Result<agentic_connector::ExecutionResult, ConnectorError> {
        unimplemented!("grader only streams full results")
    }
    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        let _held = self.1.lock().await;
        self.2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.0.get(sql).expect("unexpected sql") {
            Canned::Rows(rows) => Ok(stream_of(rows)),
            Canned::Error(m) => Err(ConnectorError::Other((*m).into())),
            Canned::Hang => std::future::pending().await,
            Canned::Slow(secs, rows) => {
                tokio::time::sleep(Duration::from_secs(*secs)).await;
                Ok(stream_of(rows))
            }
        }
    }
}

fn ints(v: &[i64]) -> Canned {
    Canned::Rows(v.iter().map(|i| vec![TypedValue::Int64(*i)]).collect())
}

fn pair(actual_sql: Option<&str>, gold_sql: &str) -> (TargetOutput, TargetOutput) {
    let actual = TargetOutput {
        output: "answer".into(),
        final_sql: actual_sql.map(str::to_string),
        llm_rounds: 3,
        timing: Some(oxy::exec_types::RunTiming {
            wall_ms: 42,
            ..Default::default()
        }),
        ..Default::default()
    };
    let expected = TargetOutput {
        output: gold_sql.into(),
        final_sql: Some(gold_sql.into()),
        task_description: Some("q".into()),
        ..Default::default()
    };
    (actual, expected)
}

async fn grade(conn: &FakeConn, actual_sql: Option<&str>, gold: &str) -> Record {
    let (a, e) = pair(actual_sql, gold);
    grade_pair(
        &GradingDb::new(conn),
        &FakeJudge::new("CORRECT\nok"),
        &a,
        &e,
    )
    .await
}

#[tokio::test]
async fn pass_when_rows_match() {
    let conn = FakeConn::new(HashMap::from([("a", ints(&[1, 2])), ("g", ints(&[1, 2]))]));
    let r = grade(&conn, Some("a"), "g").await;
    assert_eq!(
        (r.choice.as_str(), r.score, r.soft_f1),
        ("PASS", 1.0, Some(1.0))
    );
    assert_eq!(r.llm_rounds, 3);
    assert_eq!(r.timing.as_ref().map(|t| t.wall_ms), Some(42));
    assert_eq!(r.final_sql.as_deref(), Some("a"));
}

#[tokio::test]
async fn fail_when_no_final_sql() {
    let conn = FakeConn::new(HashMap::from([("g", ints(&[1]))]));
    let r = grade(&conn, None, "g").await;
    assert_eq!(r.score, 0.0, "EX mismatch");
    assert!(r.cot.contains("no successful query"), "{}", r.cot);
}

#[tokio::test]
async fn skip_when_gold_errors() {
    let conn = FakeConn::new(HashMap::from([
        ("a", ints(&[1])),
        ("g", Canned::Error("boom")),
        ("g2", ints(&[1])),
    ]));
    let skipped = grade(&conn, Some("a"), "g").await;
    assert_eq!(skipped.choice, "SKIP");
    let passed = grade(&conn, Some("a"), "g2").await;
    let m = Execution::from_records(vec![skipped, passed]);
    assert_eq!((m.graded, m.skipped, m.ex), (1, 1, 1.0));
}

#[tokio::test(start_paused = true)]
async fn fail_on_timeout() {
    let conn = FakeConn::new(HashMap::from([("a", Canned::Hang), ("g", ints(&[1]))]));
    let fut = grade(&conn, Some("a"), "g");
    tokio::pin!(fut);
    tokio::select! {
        _ = &mut fut => panic!("finished before the timeout elapsed"),
        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
    }
    tokio::time::advance(Duration::from_secs(61)).await;
    let r = fut.await;
    assert_eq!(r.score, 0.0, "EX mismatch");
    assert!(r.cot.contains("timeout"), "{}", r.cot);
}

#[tokio::test]
async fn fail_on_row_cap() {
    let big = Canned::Rows(vec![vec![TypedValue::Int64(1)]; ROW_CAP + 1]);
    let conn = FakeConn::new(HashMap::from([("a", big), ("g", ints(&[1]))]));
    let r = grade(&conn, Some("a"), "g").await;
    assert_eq!(r.score, 0.0, "EX mismatch");
    assert!(r.cot.contains("row cap"), "{}", r.cot);
}

#[test]
fn run_errors_count_as_fail() {
    let (_, expected) = pair(None, "g");
    let rec = failed_run_record("llm down", &expected);
    let pass = Record {
        choice: "PASS".into(),
        score: 1.0,
        soft_f1: Some(1.0),
        ..failed_run_record("x", &expected)
    };
    let m = Execution::from_records(vec![pass, rec]);
    assert_eq!((m.graded, m.skipped, m.ex), (2, 0, 0.5));
}

#[tokio::test]
async fn gold_failure_skips_without_judge_call() {
    let conn = FakeConn::new(HashMap::from([
        ("a", ints(&[1])),
        ("g", Canned::Error("boom")),
    ]));
    let judge = FakeJudge::new("CORRECT\nok");
    let (a, e) = pair(Some("a"), "g");
    let r = grade_pair(&GradingDb::new(&conn), &judge, &a, &e).await;
    assert_eq!(r.choice, "SKIP");
    assert_eq!(r.judge_verdict, None);
    assert_eq!(judge.calls(), 0);
}

#[tokio::test]
async fn judge_runs_without_agent_sql_and_records_inputs() {
    let conn = FakeConn::new(HashMap::from([("g", ints(&[7]))]));
    let judge = FakeJudge::new("CORRECT\nstates 7");
    let (a, e) = pair(None, "g");
    let r = grade_pair(&GradingDb::new(&conn), &judge, &a, &e).await;
    assert_eq!(r.score, 0.0, "EX still fails without agent SQL");
    assert_eq!(r.choice, "PASS", "the judge verdict decides pass/fail");
    assert_eq!(judge.calls(), 1);
    assert_eq!(r.judge_verdict.as_deref(), Some(VERDICT_CORRECT));
    assert_eq!(r.judge_reason.as_deref(), Some("states 7"));
    assert_eq!(r.answer_text.as_deref(), Some("answer"));
    assert_eq!(r.gold_preview.as_deref(), Some("c\n7"));
}

#[test]
fn answer_acc_aggregates() {
    let (_, expected) = pair(None, "g");
    let judged = |verdict: &str| Record {
        judge_verdict: Some(verdict.to_string()),
        ..failed_run_record("x", &expected)
    };
    let skip = Record {
        choice: "SKIP".into(),
        judge_verdict: None,
        ..failed_run_record("x", &expected)
    };
    let m = Execution::from_records(vec![
        judged(VERDICT_CORRECT),
        judged(VERDICT_CORRECT),
        judged("INCORRECT"),
        skip,
    ]);
    assert_eq!((m.graded, m.skipped), (3, 1));
    assert!((m.answer_acc - 2.0 / 3.0).abs() < 1e-6, "{}", m.answer_acc);
    assert_eq!(m.correct_answers(), 2);
}

#[tokio::test]
async fn judge_error_is_excluded_and_counted() {
    let conn = FakeConn::new(HashMap::from([("a", ints(&[1])), ("g", ints(&[1]))]));
    let judge = FakeJudge::scripted("CORRECT\nok", vec![Step::Fail("503"), Step::Fail("504")]);
    let (a, e) = pair(Some("a"), "g");
    let errored = grade_pair(&GradingDb::new(&conn), &judge, &a, &e).await;
    assert_eq!(judge.calls(), 2, "one call plus one retry");
    assert_eq!(errored.judge_verdict.as_deref(), Some(VERDICT_JUDGE_ERROR));
    assert!(
        errored
            .judge_reason
            .as_deref()
            .is_some_and(|r| r.contains("504"))
    );
    assert_eq!(
        (errored.choice.as_str(), errored.score, errored.soft_f1),
        (VERDICT_JUDGE_ERROR, 1.0, Some(1.0))
    );

    let (_, expected) = pair(None, "g");
    let judged = |verdict: &str| Record {
        judge_verdict: Some(verdict.to_string()),
        ..failed_run_record("x", &expected)
    };
    let m = Execution::from_records(vec![errored, judged(VERDICT_CORRECT), judged("INCORRECT")]);
    assert_eq!((m.graded, m.judge_errors, m.judged()), (3, 1, 2));
    assert!((m.answer_acc - 0.5).abs() < 1e-6, "{}", m.answer_acc);
    assert!((m.ex - 1.0 / 3.0).abs() < 1e-6, "EX unaffected: {}", m.ex);
}

#[tokio::test]
async fn choice_follows_the_judge_not_ex() {
    let conn = FakeConn::new(HashMap::from([("a", ints(&[1])), ("g", ints(&[1]))]));
    let (a, e) = pair(Some("a"), "g");
    let judge = FakeJudge::new("INCORRECT\nwrong number in prose");
    let r = grade_pair(&GradingDb::new(&conn), &judge, &a, &e).await;
    assert_eq!((r.choice.as_str(), r.score), ("FAIL", 1.0));
    let m = Execution::from_records(vec![r]);
    assert_eq!(
        (m.ex_matches(), m.correct_answers(), m.case_tally()),
        (1, 0, (1, 0))
    );
    let report = m.failure_report();
    assert!(
        report.contains("ANSWER FAILURES (judge INCORRECT)"),
        "{report}"
    );
    assert!(
        report.contains("wrong number in prose") && report.contains("EX: match"),
        "{report}"
    );
}

#[test]
fn failure_report_and_tally_skip_ex_only_mismatches() {
    let (_, expected) = pair(None, "g");
    let with = |verdict: &str, choice: &str| Record {
        choice: choice.into(),
        judge_verdict: Some(verdict.to_string()),
        ..failed_run_record("x", &expected)
    };
    // judge CORRECT with an EX mismatch, a JUDGE_ERROR, and a SKIP: no failures.
    let skip = Record {
        judge_verdict: None,
        ..with(VERDICT_CORRECT, "SKIP")
    };
    let m = Execution::from_records(vec![
        with(VERDICT_CORRECT, "PASS"),
        with(VERDICT_JUDGE_ERROR, VERDICT_JUDGE_ERROR),
        skip,
    ]);
    assert_eq!(m.failure_report(), "");
    assert_eq!(
        m.case_tally(),
        (1, 1),
        "same prompt: one judged case, passing"
    );
    assert_eq!(m.ex_matches(), 0);
}

#[tokio::test(start_paused = true)]
async fn gold_timeout_is_labelled_apart_from_gold_error() {
    let conn = FakeConn::new(HashMap::from([
        ("a", ints(&[1])),
        ("hang", Canned::Hang),
        (
            "cancel",
            Canned::Error("query failed: [57014] canceling statement due to statement timeout"),
        ),
        (
            "boom",
            Canned::Error("query failed: [42702] column reference \"round\" is ambiguous"),
        ),
    ]));
    let hang = grade(&conn, Some("a"), "hang").await;
    assert_eq!(hang.choice, "SKIP");
    assert!(
        hang.cot.starts_with("gold timeout: timeout after 60s"),
        "{}",
        hang.cot
    );
    let cancel = grade(&conn, Some("a"), "cancel").await;
    assert!(cancel.cot.starts_with("gold timeout: "), "{}", cancel.cot);
    let boom = grade(&conn, Some("a"), "boom").await;
    assert!(boom.cot.starts_with("gold sql failed: "), "{}", boom.cot);
}

#[tokio::test(start_paused = true)]
async fn timeout_starts_after_the_gate_so_waiting_does_not_skip() {
    // Each gold query holds the connection 40 s. Concurrently, the second waits
    // 40 s for the first, then runs 40 s: 80 s in all, but only 40 s of it is
    // its own, so it must not hit the 60 s timeout.
    let slow = Canned::Slow(40, vec![vec![TypedValue::Int64(1)]]);
    let conn = FakeConn::new(HashMap::from([("a", ints(&[1])), ("slow", slow)]));
    let db = GradingDb::new(&conn);
    let judge = FakeJudge::new("CORRECT\nok");
    let (a, e) = pair(Some("a"), "slow");
    let (r1, r2) = tokio::join!(
        grade_pair(&db, &judge, &a, &e),
        grade_pair(&db, &judge, &a, &e)
    );
    assert_eq!(
        (r1.choice.as_str(), r2.choice.as_str()),
        ("PASS", "PASS"),
        "{}",
        r2.cot
    );
}

#[tokio::test]
async fn errored_run_skips_when_gold_fails_and_fails_when_gold_runs() {
    let conn = FakeConn::new(HashMap::from([
        ("bad", Canned::Error("query failed: [42702] ambiguous")),
        ("good", ints(&[1])),
    ]));
    let (_, bad) = pair(None, "bad");
    let (_, good) = pair(None, "good");
    let errors = vec![
        ("max iterations exceeded".to_string(), bad.clone()),
        ("clarifying question".to_string(), bad),
        ("max iterations exceeded".to_string(), good),
    ];
    let records = grade_errored_runs(&GradingDb::new(&conn), &errors).await;
    assert_eq!(
        conn.queries(),
        2,
        "gold runs once per case, not once per run"
    );

    for skipped in &records[..2] {
        assert_eq!(skipped.choice, "SKIP");
        assert_eq!(skipped.judge_verdict, None);
        assert_eq!(skipped.soft_f1, None);
        assert!(
            skipped.cot.starts_with("gold sql failed: "),
            "{}",
            skipped.cot
        );
    }
    let failed = &records[2];
    assert_eq!(failed.choice, "FAIL");
    assert_eq!(failed.judge_verdict.as_deref(), Some(VERDICT_INCORRECT));

    let m = Execution::from_records(records);
    assert_eq!(
        (m.graded, m.skipped),
        (1, 2),
        "only the gold-OK run is counted"
    );
    assert_eq!((m.correct_answers(), m.answer_acc), (0, 0.0));
}

#[tokio::test(start_paused = true)]
async fn errored_run_on_gold_timeout_is_labelled_timeout() {
    let conn = FakeConn::new(HashMap::from([("hang", Canned::Hang)]));
    let (_, hang) = pair(None, "hang");
    let errors = vec![("max iterations exceeded".to_string(), hang)];
    let records = grade_errored_runs(&GradingDb::new(&conn), &errors).await;
    assert_eq!(records[0].choice, "SKIP");
    assert!(
        records[0].cot.starts_with("gold timeout: "),
        "{}",
        records[0].cot
    );
}
