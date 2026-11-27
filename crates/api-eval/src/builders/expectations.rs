//! A case's `expect:` block checked against what its run did
//! ([`RunObserved`]). Any unmet expectation fails the record and is named in
//! `unmet_expectations` and in its `cot`.
//!
//! A run that stopped on a clarifying question is graded only when its case
//! expects one (`asks_user: true`); otherwise it fails as a run error, with the
//! same record as before expectations existed (see [`unexpected_question`]).

use oxy::config::test_config::TestExpect;
use oxy::exec_types::{RunObserved, TargetOutput};

use super::types::{Record, VERDICT_CORRECT, VERDICT_INCORRECT};

const PASS: &str = "PASS";
const FAIL: &str = "FAIL";
const SKIP: &str = "SKIP";

/// Each expectation `expect` sets that `seen` does not meet, named, with what
/// the run did instead. Empty when all are met.
pub(super) fn unmet(expect: &TestExpect, seen: &RunObserved) -> Vec<String> {
    let charts = seen.charts;
    let asked = seen.asked_user.as_deref().unwrap_or_default();
    [
        flag(
            "asks_user",
            expect.asks_user,
            seen.asked_user.is_some(),
            "expected a clarifying question; the run answered".to_string(),
            format!("expected an answer; the run asked: {asked}"),
        ),
        flag(
            "chart",
            expect.chart,
            charts > 0,
            "expected a chart; none was drawn".to_string(),
            format!("expected no chart; {charts} drawn"),
        ),
        flag(
            "preagg",
            expect.preagg,
            seen.preagg,
            "expected a query served from a pre-aggregation; none was".to_string(),
            "expected no pre-aggregated query; one was served from a pre-aggregation".to_string(),
        ),
        query_source(expect, seen),
    ]
    .into_iter()
    .flatten()
    .chain(missing_tools(&expect.tools, &seen.tools))
    .collect()
}

/// A boolean expectation: `missing` when it wanted `true` and got `false`,
/// `extra` the other way round.
fn flag(
    name: &str,
    want: Option<bool>,
    got: bool,
    missing: String,
    extra: String,
) -> Option<String> {
    match want? {
        true if !got => Some(format!("{name}: {missing}")),
        false if got => Some(format!("{name}: {extra}")),
        _ => None,
    }
}

fn query_source(expect: &TestExpect, seen: &RunObserved) -> Option<String> {
    let want = expect.query_source?.as_str();
    match seen.answer_source.as_deref() {
        Some(got) if got == want => None,
        Some(got) => Some(format!(
            "query_source: expected the answer's query from `{want}`; it came from `{got}`"
        )),
        None => Some(format!(
            "query_source: expected the answer's query from `{want}`; the answer rests on no \
             successful query"
        )),
    }
}

fn missing_tools(want: &[String], called: &[String]) -> Vec<String> {
    let called_list = if called.is_empty() {
        "no tools called".to_string()
    } else {
        format!("called: {}", called.join(", "))
    };
    want.iter()
        .filter(|t| !called.contains(t))
        .map(|t| format!("tools: `{t}` was never called ({called_list})"))
        .collect()
}

/// The error a run that asked a question becomes when its case does not
/// expect one: the text a headless run returned before expectations, so the
/// record (and every reader of it) is unchanged. `None` when the run answered
/// or the question was expected.
pub(super) fn unexpected_question(
    actual: &TargetOutput,
    expected: &TargetOutput,
) -> Option<String> {
    let question = actual.observed.as_ref()?.asked_user.as_deref()?;
    if expected
        .expect
        .as_ref()
        .is_some_and(TestExpect::expects_question)
    {
        return None;
    }
    Some(format!(
        "{}{question}",
        agentic_pipeline::CLARIFYING_QUESTION_PREFIX
    ))
}

/// Whether the answer text is graded. Not when the run asked (an expected
/// question has no answer to grade), nor when the case has expectations and
/// no reference to grade against (`has_reference`: an `expected` for the
/// judge, an `expected_sql` for the execution grader). A case with neither is
/// graded as before, and fails there.
pub(super) fn grades_answer(
    actual: &TargetOutput,
    expected: &TargetOutput,
    has_reference: bool,
) -> bool {
    let asked = actual
        .observed
        .as_ref()
        .is_some_and(|o| o.asked_user.is_some());
    !asked && (has_reference || expected.expect.is_none())
}

fn unmet_for(actual: &TargetOutput, expected: &TargetOutput) -> Vec<String> {
    match &expected.expect {
        Some(expect) => unmet(expect, &actual.observed.clone().unwrap_or_default()),
        None => vec![],
    }
}

/// Fold `unmet` into a graded record: it fails (a `SKIP` stays one) and the
/// reasons are appended to `cot`.
fn fail_on(record: &mut Record, unmet: Vec<String>) -> bool {
    if unmet.is_empty() {
        return false;
    }
    if record.choice != SKIP {
        record.choice = FAIL.to_string();
    }
    let reasons = format!("Unmet expectations:\n- {}", unmet.join("\n- "));
    record.cot = if record.cot.is_empty() {
        reasons
    } else {
        format!("{}\n\n{reasons}", record.cot)
    };
    record.unmet_expectations = unmet;
    true
}

/// Judge grader: an unmet expectation fails the record (`score` 0).
pub(super) fn check_judged(record: &mut Record, actual: &TargetOutput, expected: &TargetOutput) {
    record.observed = actual.observed.clone();
    if fail_on(record, unmet_for(actual, expected)) {
        record.score = 0.0;
    }
}

/// Execution grader: an unmet expectation fails the record and makes its
/// verdict `INCORRECT`, so `answer_acc` (what the gate reads) counts it. The
/// EX bit (`score`) is left alone.
pub(super) fn check_executed(record: &mut Record, actual: &TargetOutput, expected: &TargetOutput) {
    record.observed = actual.observed.clone();
    if fail_on(record, unmet_for(actual, expected)) && record.choice != SKIP {
        let reason = record.unmet_expectations.join("; ");
        record.judge_reason = Some(match record.judge_reason.take() {
            Some(judge) => format!("unmet expectations: {reason}; judge: {judge}"),
            None => format!("unmet expectations: {reason}"),
        });
        record.judge_verdict = Some(VERDICT_INCORRECT.to_string());
    }
}

/// A record whose answer text is not graded passes on its expectations
/// alone. `record` is the grader's base record for the run.
fn ungraded(mut record: Record, actual: &TargetOutput) -> Record {
    record.choice = PASS.to_string();
    record.score = 1.0;
    record.cot = match actual
        .observed
        .as_ref()
        .and_then(|o| o.asked_user.as_deref())
    {
        Some(_) => "the run asked a clarifying question; answer text not graded".to_string(),
        None => "no reference answer; answer text not graded".to_string(),
    };
    record
}

/// Judge grader, answer not graded: PASS unless an expectation is unmet.
pub(super) fn ungraded_judged(
    base: Record,
    actual: &TargetOutput,
    expected: &TargetOutput,
) -> Record {
    let mut record = ungraded(base, actual);
    check_judged(&mut record, actual, expected);
    record
}

/// Execution grader, answer not graded: no EX ran, so `score` is 0 and
/// `soft_f1` absent; the verdict follows the expectations.
pub(super) fn ungraded_executed(
    base: Record,
    actual: &TargetOutput,
    expected: &TargetOutput,
) -> Record {
    let mut record = ungraded(base, actual);
    record.score = 0.0;
    record.soft_f1 = None;
    record.judge_verdict = Some(VERDICT_CORRECT.to_string());
    record.judge_reason = Some(record.cot.clone());
    check_executed(&mut record, actual, expected);
    record
}

#[cfg(test)]
#[path = "expectations_tests.rs"]
mod tests;
