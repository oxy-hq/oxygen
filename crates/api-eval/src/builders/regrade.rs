//! Regrade: grade the agent outputs a previous run recorded, without running
//! the agent again. The records of a results file (`oxy dev test --output-json`)
//! carry each run's question, gold SQL, answer text, final SQL, tokens and
//! timing, which is everything a grader reads. Only the grading (gold SQL,
//! EX, judge) is redone, so a change to the grader can be applied to a costly
//! baseline for the price of the judge calls.

use std::path::Path;

use serde::Deserialize;

use oxy::config::test_config::TestCase;
use oxy::exec_types::{RunObserved, RunTiming, TargetOutput};
use oxy_shared::errors::OxyError;

/// The prefix `failed_run_record` puts on the `actual_output` of a run that
/// errored before answering.
const ERROR_PREFIX: &str = "[ERROR] ";

/// The `type` tag of an execution-grader metric in a results file.
const EXECUTION_KIND: &str = "Execution";

/// The record fields a regrade reads back; the rest are the grader's own output.
#[derive(Deserialize)]
struct SavedRecord {
    prompt: Option<String>,
    expected: Option<String>,
    actual_output: Option<String>,
    answer_text: Option<String>,
    #[serde(default)]
    duration_ms: f64,
    #[serde(default)]
    input_tokens: i32,
    #[serde(default)]
    output_tokens: i32,
    #[serde(default)]
    llm_rounds: u32,
    final_sql: Option<String>,
    timing: Option<RunTiming>,
    /// Kept so a regraded react file still reads as react.
    runtime: Option<String>,
    model: Option<String>,
    /// Kept so a regraded record still says what the run did.
    #[serde(default)]
    observed: Option<RunObserved>,
}

#[derive(Deserialize)]
struct SavedMetric {
    /// The grader that wrote the records (`MetricKind`'s tag).
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    records: Vec<SavedRecord>,
}

#[derive(Deserialize)]
struct SavedResult {
    #[serde(default)]
    metrics: Vec<SavedMetric>,
}

/// The answered `(actual, expected)` pairs and the errored `(message, expected)`
/// runs, as the generator would have returned them.
pub(super) type LoadedOutputs = (
    Vec<(TargetOutput, TargetOutput)>,
    Vec<(String, TargetOutput)>,
);

/// Rebuild the generator's outputs from a results document, wherever it is,
/// with each run's `expect:` checks taken from `cases` (the test file).
pub(super) fn load(
    source: &super::types::RegradeSource,
    cases: &[TestCase],
) -> Result<LoadedOutputs, OxyError> {
    let mut loaded = match source {
        super::types::RegradeSource::File(path) => load_outputs(path)?,
        super::types::RegradeSource::Text(text) => load_text(text)?,
    };
    attach_expectations(&mut loaded, cases);
    Ok(route_unexpected_questions(loaded))
}

/// A recorded clarifying question its case (as the test file reads now) does
/// not expect is the run error a fresh run makes of it, as in the generator.
fn route_unexpected_questions((outputs, mut errors): LoadedOutputs) -> LoadedOutputs {
    let mut answered = Vec::with_capacity(outputs.len());
    for (actual, expected) in outputs {
        match super::expectations::unexpected_question(&actual, &expected) {
            Some(msg) => errors.push((msg, expected)),
            None => answered.push((actual, expected)),
        }
    }
    (answered, errors)
}

fn load_text(text: &str) -> Result<LoadedOutputs, OxyError> {
    parse_outputs(text).map_err(|e| OxyError::ConfigurationError(format!("regrade: {e}")))
}

/// Give each loaded run the `expect:` checks of the case whose prompt it
/// answers. A results file does not save them, so they come from the test
/// file, as they do on a fresh run; files written before `expect:` existed
/// regrade the same way. A run no case matches is checked against nothing.
fn attach_expectations(loaded: &mut LoadedOutputs, cases: &[TestCase]) {
    let expect_for = |prompt: Option<&str>| {
        cases
            .iter()
            .find(|c| Some(c.prompt.as_str()) == prompt)
            .and_then(TestCase::expectations)
    };
    let (outputs, errors) = loaded;
    let expected_sides = outputs
        .iter_mut()
        .map(|(_, e)| e)
        .chain(errors.iter_mut().map(|(_, e)| e));
    for expected in expected_sides {
        expected.expect = expect_for(expected.task_description.as_deref());
    }
}

/// Read a results file and rebuild the generator's outputs from its records.
pub(super) fn load_outputs(path: &Path) -> Result<LoadedOutputs, OxyError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        OxyError::ConfigurationError(format!("regrade: cannot read {}: {e}", path.display()))
    })?;
    parse_outputs(&text)
        .map_err(|e| OxyError::ConfigurationError(format!("regrade: {}: {e}", path.display())))
}

fn parse_outputs(text: &str) -> Result<LoadedOutputs, String> {
    let results: Vec<SavedResult> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    // A test file holds one metric per grader; regrading takes the records of
    // the first one, so a run is not loaded twice.
    let metrics: Vec<SavedMetric> = results
        .into_iter()
        .filter_map(|r| r.metrics.into_iter().next())
        .collect();
    // Only an execution record keeps the gold SQL in `expected`; another
    // grader keeps the expected answer text there, which this rebuild would
    // put in the wrong field and grade against an empty answer.
    if let Some(m) = metrics.iter().find(|m| m.kind != EXECUTION_KIND) {
        return Err(format!(
            "only execution-graded results can be regraded; this file holds `{}` records",
            m.kind
        ));
    }
    let records: Vec<SavedRecord> = metrics.into_iter().flat_map(|m| m.records).collect();
    if records.is_empty() {
        return Err("no records to regrade".to_string());
    }
    let mut outputs = Vec::new();
    let mut errors = Vec::new();
    for record in records {
        let expected = TargetOutput {
            task_description: record.prompt.clone(),
            final_sql: record.expected.clone(),
            ..Default::default()
        };
        match errored_message(&record) {
            Some(msg) => errors.push((msg, expected)),
            None => outputs.push((actual_from(record), expected)),
        }
    }
    Ok((outputs, errors))
}

/// A run that errored has no answer text and an `[ERROR] …` output.
fn errored_message(record: &SavedRecord) -> Option<String> {
    if record.answer_text.is_some() {
        return None;
    }
    let output = record.actual_output.as_deref()?;
    output.strip_prefix(ERROR_PREFIX).map(str::to_string)
}

fn actual_from(record: SavedRecord) -> TargetOutput {
    TargetOutput {
        output: record
            .answer_text
            .or(record.actual_output)
            .unwrap_or_default(),
        task_description: record.prompt,
        duration_ms: record.duration_ms,
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        final_sql: record.final_sql,
        llm_rounds: record.llm_rounds,
        timing: record.timing,
        runtime: record.runtime,
        model: record.model,
        observed: record.observed,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(records: &str) -> String {
        format!(
            r#"[{{"errors": [], "metrics": [{{"type": "Execution", "records": [{records}]}}]}}]"#
        )
    }

    #[test]
    fn an_answered_record_becomes_an_output_pair_with_its_gold() {
        let text = file(
            r#"{"cot": "x", "choice": "FAIL", "score": 0.0, "prompt": "How many?",
                "expected": "SELECT 1", "actual_output": "Three.", "answer_text": "Three.",
                "duration_ms": 12.5, "input_tokens": 7, "output_tokens": 3, "llm_rounds": 2,
                "final_sql": "SELECT 3", "judge_verdict": "INCORRECT", "runtime": "react", "model": "m-1"}"#,
        );
        let (outputs, errors) = parse_outputs(&text).unwrap();
        assert!(errors.is_empty());
        let (actual, expected) = &outputs[0];
        assert_eq!(actual.output, "Three.");
        assert_eq!(actual.final_sql.as_deref(), Some("SELECT 3"));
        assert_eq!(
            (actual.input_tokens, actual.output_tokens, actual.llm_rounds),
            (7, 3, 2)
        );
        assert_eq!(actual.duration_ms, 12.5);
        assert_eq!(actual.runtime.as_deref(), Some("react"));
        assert_eq!(actual.model.as_deref(), Some("m-1"));
        assert_eq!(expected.task_description.as_deref(), Some("How many?"));
        assert_eq!(expected.final_sql.as_deref(), Some("SELECT 1"));
    }

    #[test]
    fn an_errored_record_becomes_an_error_with_its_message() {
        let text = file(
            r#"{"cot": "Run failed", "choice": "FAIL", "score": 0.0, "prompt": "Q",
                "expected": "SELECT 1", "actual_output": "[ERROR] llm 400",
                "duration_ms": 0.0, "input_tokens": 0, "output_tokens": 0, "llm_rounds": 0}"#,
        );
        let (outputs, errors) = parse_outputs(&text).unwrap();
        assert!(outputs.is_empty());
        assert_eq!(errors[0].0, "llm 400");
        assert_eq!(errors[0].1.final_sql.as_deref(), Some("SELECT 1"));
    }

    #[test]
    fn an_answer_that_starts_like_an_error_is_still_an_answer() {
        let text = file(
            r#"{"cot": "", "choice": "FAIL", "score": 0.0, "prompt": "Q", "expected": "S",
                "actual_output": "[ERROR] in the data", "answer_text": "[ERROR] in the data",
                "duration_ms": 1.0, "input_tokens": 1, "output_tokens": 1, "llm_rounds": 1}"#,
        );
        let (outputs, errors) = parse_outputs(&text).unwrap();
        assert_eq!((outputs.len(), errors.len()), (1, 0));
    }

    #[test]
    fn a_correctness_results_file_is_refused() {
        let text = r#"[{"errors": [], "metrics": [{"type": "Correctness", "score": 1.0,
            "records": [{"cot": "", "choice": "A", "score": 1.0, "prompt": "Q",
            "expected": "the expected answer", "actual_output": "an answer",
            "duration_ms": 1.0, "input_tokens": 1, "output_tokens": 1, "llm_rounds": 1}]}]}]"#;
        let err = parse_outputs(text).expect_err("a correctness file must not be regraded");
        assert!(err.contains("only execution-graded"), "{err}");
    }

    fn case(yaml: &str) -> TestCase {
        serde_yaml::from_str(yaml).unwrap()
    }

    /// A regraded run is held to its case's `expect:` checks: one that drew no
    /// chart where the case wants one stays `INCORRECT`, though the judge
    /// called its answer correct.
    #[test]
    fn a_regrade_keeps_the_cases_expectations() {
        let text = file(
            r#"{"cot": "x", "choice": "PASS", "score": 1.0, "prompt": "Chart it",
                "expected": "SELECT 1", "actual_output": "Here.", "answer_text": "Here.",
                "duration_ms": 1.0, "input_tokens": 1, "output_tokens": 1, "llm_rounds": 1,
                "observed": {"tools": ["query"], "query_sources": ["llm"],
                             "answer_source": "llm", "preagg": false, "charts": 0}}"#,
        );
        let cases = [
            case("prompt: Chart it\nexpected_sql: SELECT 1\nexpect:\n  chart: true\n"),
            case("prompt: Other\nexpected_sql: SELECT 2\ntool: search\n"),
        ];
        let source = super::super::types::RegradeSource::Text(text);
        let (outputs, _) = load(&source, &cases).unwrap();
        let (actual, expected) = &outputs[0];
        assert_eq!(expected.expect.as_ref().and_then(|e| e.chart), Some(true));
        assert_eq!(actual.observed.as_ref().map(|o| o.charts), Some(0));

        let mut record = super::super::types::Record {
            choice: "PASS".into(),
            score: 1.0,
            judge_verdict: Some(super::super::types::VERDICT_CORRECT.into()),
            ..Default::default()
        };
        super::super::expectations::check_executed(&mut record, actual, expected);
        assert_eq!(record.choice, "FAIL");
        assert_eq!(
            record.judge_verdict.as_deref(),
            Some(super::super::types::VERDICT_INCORRECT)
        );
        assert!(record.unmet_expectations[0].starts_with("chart:"));
    }

    /// A recorded question whose case no longer sets `asks_user: true` is a
    /// run error, as it would be on a fresh run, not an ungraded pass.
    #[test]
    fn a_recorded_question_its_case_does_not_expect_is_a_run_error() {
        let text = file(
            r#"{"cot": "x", "choice": "PASS", "score": 0.0, "prompt": "Sales?",
                "expected": "SELECT 1", "actual_output": "Which region?",
                "answer_text": "Which region?", "duration_ms": 1.0, "input_tokens": 1,
                "output_tokens": 1, "llm_rounds": 1,
                "observed": {"asked_user": "Which region?"}}"#,
        );
        let source = super::super::types::RegradeSource::Text(text);
        let cases = [case("prompt: Sales?\nexpected_sql: SELECT 1\n")];
        let (outputs, errors) = load(&source, &cases).unwrap();
        assert!(outputs.is_empty());
        assert_eq!(
            errors[0].0,
            "agentic pipeline asked a clarifying question: Which region?"
        );

        let cases = [case(
            "prompt: Sales?\nexpected_sql: SELECT 1\nexpect:\n  asks_user: true\n",
        )];
        let (outputs, errors) = load(&source, &cases).unwrap();
        assert_eq!((outputs.len(), errors.len()), (1, 0));
    }

    #[test]
    fn a_regrade_with_no_matching_case_checks_nothing() {
        let text = file(
            r#"{"cot": "Run failed", "choice": "FAIL", "score": 0.0, "prompt": "Q",
                "expected": "SELECT 1", "actual_output": "[ERROR] llm 400",
                "duration_ms": 0.0, "input_tokens": 0, "output_tokens": 0, "llm_rounds": 0}"#,
        );
        let cases = [case(
            "prompt: Other\nexpected_sql: SELECT 2\ntool: search\n",
        )];
        let source = super::super::types::RegradeSource::Text(text);
        let (_, errors) = load(&source, &cases).unwrap();
        assert_eq!(errors[0].1.expect, None);
    }

    #[test]
    fn a_file_with_no_records_is_refused() {
        assert!(parse_outputs("[]").is_err());
        assert!(parse_outputs(&file("")).is_err());
        assert!(parse_outputs("not json").is_err());
    }
}
