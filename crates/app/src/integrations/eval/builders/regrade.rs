//! Regrade: grade the agent outputs a previous run recorded, without running
//! the agent again. The records of a results file (`oxy test --output-json`)
//! carry each run's question, gold SQL, answer text, final SQL, tokens and
//! timing, which is everything a grader reads. Only the grading (gold SQL,
//! EX, judge) is redone, so a change to the grader can be applied to a costly
//! baseline for the price of the judge calls.

use std::path::Path;

use serde::Deserialize;

use oxy::exec_types::{RunTiming, TargetOutput};
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
                "final_sql": "SELECT 3", "judge_verdict": "INCORRECT"}"#,
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

    #[test]
    fn a_file_with_no_records_is_refused() {
        assert!(parse_outputs("[]").is_err());
        assert!(parse_outputs(&file("")).is_err());
        assert!(parse_outputs("not json").is_err());
    }
}
