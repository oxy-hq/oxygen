//! The execution grader's metric: BIRD EX + Soft-F1, plus answer accuracy (an
//! LLM judge reading the agent's answer text against the gold rows).
//!
//! Answer accuracy is the headline: a record passes when the judge says
//! `CORRECT` and fails when it says `INCORRECT`; `JUDGE_ERROR` and `SKIP` are
//! neither. A record's `score` stays the EX bit (1.0 = result sets match), so
//! EX mismatches are reported as a count, not as failures.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde::Serialize;

use oxy::theme::StyledText;

use super::types::{Record, Verbose};

pub const VERDICT_CORRECT: &str = "CORRECT";
pub const VERDICT_INCORRECT: &str = "INCORRECT";
/// The judge call failed (both attempts); excluded from `answer_acc`.
pub const VERDICT_JUDGE_ERROR: &str = "JUDGE_ERROR";

/// Record `choice` for a judge verdict: `PASS` / `FAIL`, or the verdict itself
/// (`JUDGE_ERROR`) when it is neither.
pub(super) fn choice_for_verdict(verdict: &str) -> &'static str {
    match verdict {
        VERDICT_CORRECT => "PASS",
        VERDICT_INCORRECT => "FAIL",
        _ => VERDICT_JUDGE_ERROR,
    }
}

/// `SKIP` records (the gold SQL itself failed) are kept for the report but
/// excluded from every mean.
#[derive(Clone, Debug, Serialize)]
pub struct Execution {
    pub ex: f32,
    pub soft_f1: f32,
    /// `CORRECT / (CORRECT + INCORRECT)` over graded records; `JUDGE_ERROR`
    /// records are excluded and counted in `judge_errors`.
    pub answer_acc: f32,
    /// Graded records whose judge call failed on both attempts.
    pub judge_errors: usize,
    /// Resolved model name of the answer-accuracy judge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_model: Option<String>,
    pub graded: usize,
    pub skipped: usize,
    pub records: Vec<Record>,
}

impl Execution {
    pub fn from_records(records: Vec<Record>) -> Self {
        let graded_records: Vec<&Record> = records.iter().filter(|r| r.choice != "SKIP").collect();
        let graded = graded_records.len();
        let mean = |f: &dyn Fn(&Record) -> f32| {
            if graded == 0 {
                0.0
            } else {
                graded_records.iter().map(|r| f(r)).sum::<f32>() / graded as f32
            }
        };
        let ex = mean(&|r| r.score);
        let soft_f1 = mean(&|r| r.soft_f1.unwrap_or(0.0));
        let judge_errors = count_verdict(&records, VERDICT_JUDGE_ERROR);
        let judged = graded - judge_errors;
        let answer_acc = if judged == 0 {
            0.0
        } else {
            count_verdict(&records, VERDICT_CORRECT) as f32 / judged as f32
        };
        let skipped = records.len() - graded;
        Self {
            ex,
            soft_f1,
            answer_acc,
            judge_errors,
            judge_model: None,
            graded,
            skipped,
            records,
        }
    }

    /// Graded records the judge called `CORRECT` (numerator of `answer_acc`).
    pub fn correct_answers(&self) -> usize {
        count_verdict(&self.records, VERDICT_CORRECT)
    }

    /// Graded records the judge ruled on (denominator of `answer_acc`).
    pub fn judged(&self) -> usize {
        self.graded - self.judge_errors
    }

    /// Graded records whose result set matched gold (numerator of `ex`).
    pub fn ex_matches(&self) -> usize {
        self.records
            .iter()
            .filter(|r| r.choice != "SKIP" && r.score >= 1.0)
            .count()
    }

    pub fn with_judge_model(mut self, model: Option<String>) -> Self {
        self.judge_model = model;
        self
    }

    /// `(judged cases, passing cases)`, grouping runs by prompt. A case counts
    /// once any run was judged; it passes when no judged run is `INCORRECT`.
    pub fn case_tally(&self) -> (usize, usize) {
        let mut cases: HashMap<&str, bool> = HashMap::new();
        for r in &self.records {
            let verdict = r.judge_verdict.as_deref();
            if r.choice == "SKIP" || !matches!(verdict, Some(VERDICT_CORRECT | VERDICT_INCORRECT)) {
                continue;
            }
            let prompt = r.prompt.as_deref().unwrap_or("(unknown)");
            let passing = cases.entry(prompt).or_insert(true);
            *passing &= verdict == Some(VERDICT_CORRECT);
        }
        (cases.len(), cases.values().filter(|p| **p).count())
    }

    /// The judge-`INCORRECT` records, each with the judge's reason and its EX
    /// outcome; empty when there are none.
    pub fn failure_report(&self) -> String {
        let mut out = String::new();
        let failing = self
            .records
            .iter()
            .filter(|r| r.judge_verdict.as_deref() == Some(VERDICT_INCORRECT));
        for (i, r) in failing.enumerate() {
            if i == 0 {
                let _ = writeln!(
                    out,
                    "\n{}\n**********",
                    "ANSWER FAILURES (judge INCORRECT):".error()
                );
            }
            let _ = writeln!(
                out,
                "Question: {}",
                r.prompt.as_deref().unwrap_or("(unknown)")
            );
            let _ = writeln!(out, "Judge: {}", r.judge_reason.as_deref().unwrap_or("-"));
            let ex = if r.score >= 1.0 { "match" } else { "mismatch" };
            let _ = writeln!(out, "EX: {ex} ({})\n**********", r.cot);
        }
        out
    }
}

fn count_verdict(records: &[Record], verdict: &str) -> usize {
    records
        .iter()
        .filter(|r| r.choice != "SKIP" && r.judge_verdict.as_deref() == Some(verdict))
        .count()
}

impl Verbose for Execution {
    fn verbose(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.failure_report())
    }
}

impl std::fmt::Display for Execution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "answer accuracy: {:.2}% ({}/{}) · EX: {:.2}% ({}/{} match) · Soft-F1: {:.2}% · judge errors: {} · graded {} · skipped {}",
            self.answer_acc * 100.0,
            self.correct_answers(),
            self.judged(),
            self.ex * 100.0,
            self.ex_matches(),
            self.graded,
            self.soft_f1 * 100.0,
            self.judge_errors,
            self.graded,
            self.skipped
        )
    }
}
