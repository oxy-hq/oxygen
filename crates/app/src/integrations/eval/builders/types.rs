use serde::Serialize;

use oxy::{exec_types::ReferenceKind, theme::StyledText};

pub use super::execution_metric::{
    Execution, VERDICT_CORRECT, VERDICT_INCORRECT, VERDICT_JUDGE_ERROR,
};

pub struct EvalInput {
    pub index: Option<usize>,
    pub target_ref: String,
    pub tag: Option<String>,
    /// Grade the outputs recorded in this results file instead of running
    /// the agent (`oxy test --regrade`).
    pub regrade_from: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug)]
pub(super) struct AgenticInput {
    pub config_path: String,
    pub prompt: String,
}

impl std::fmt::Display for AgenticInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.config_path)
    }
}

#[enum_dispatch::enum_dispatch]
pub(super) trait Verbose {
    fn verbose(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
#[enum_dispatch::enum_dispatch(Verbose)]
pub enum MetricKind {
    Similarity(Similarity),
    Recall(Recall),
    Correctness(Correctness),
    Execution(Execution),
}

#[derive(Clone, Debug, Serialize)]
pub struct Similarity {
    pub score: f32,
    pub records: Vec<Record>,
}

impl Similarity {
    pub fn new(score: f32, records: Vec<Record>) -> Self {
        Self { score, records }
    }
}

impl Verbose for Similarity {
    fn verbose(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut is_header_printed = false;
        for record in &self.records {
            if record.score < 1.0 {
                if !is_header_printed {
                    writeln!(f)?;
                    writeln!(f, "{}", "FAILURES:".error())?;
                    writeln!(f, "**********")?;
                    is_header_printed = true;
                }
                write!(f, "{record}")?;
                writeln!(f, "**********")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RecallRecord {
    pub score: f32,
    pub pass: bool,
    pub retrieved_contexts: Vec<String>,
    pub reference_contexts: Vec<String>,
}

impl std::fmt::Display for RecallRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Distance score: {}", self.score)?;
        writeln!(f, "Retrieved Contexts:")?;
        for context in &self.retrieved_contexts {
            writeln!(f, "- {context:?}")?;
        }
        writeln!(f, "Reference Contexts:")?;
        for context in &self.reference_contexts {
            writeln!(f, "- {context:?}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Recall {
    pub score: f32,
    pub records: Vec<RecallRecord>,
}

impl Verbose for Recall {
    fn verbose(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut is_header_printed = false;
        for record in &self.records {
            if !record.pass {
                if !is_header_printed {
                    writeln!(f)?;
                    writeln!(f, "{}", "RECALL FAILURES:".error())?;
                    writeln!(f, "**********")?;
                    is_header_printed = true;
                }
                write!(f, "{record}")?;
                writeln!(f, "**********")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Correctness {
    pub score: f32,
    pub records: Vec<Record>,
}

impl Correctness {
    pub fn from_records(records: Vec<Record>) -> Self {
        let score = if records.is_empty() {
            0.0
        } else {
            records.iter().map(|r| r.score).sum::<f32>() / records.len() as f32
        };
        Self { score, records }
    }
}

impl Verbose for Correctness {
    fn verbose(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut is_header_printed = false;
        for record in &self.records {
            if record.score < 1.0 {
                if !is_header_printed {
                    writeln!(f)?;
                    writeln!(f, "{}", "CORRECTNESS FAILURES:".error())?;
                    writeln!(f, "**********")?;
                    is_header_printed = true;
                }
                write!(f, "{record}")?;
                writeln!(f, "**********")?;
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for MetricKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetricKind::Similarity(Similarity { score, .. }) => {
                writeln!(f, "Accuracy: {:.2}%", score * 100.0)
            }
            MetricKind::Recall(Recall { score, .. }) => {
                writeln!(f, "Recall: {:.2}%", score * 100.0)
            }
            MetricKind::Correctness(Correctness { score, .. }) => {
                writeln!(f, "Correctness: {:.2}%", score * 100.0)
            }
            MetricKind::Execution(e) => write!(f, "{e}"),
        }
    }
}

impl MetricKind {
    pub fn verbose_write(&self, writer: &mut dyn std::io::Write) -> std::io::Result<()> {
        match self {
            MetricKind::Similarity(similarity) => {
                let failing_records: Vec<_> = similarity
                    .records
                    .iter()
                    .filter(|r| r.score < 1.0)
                    .collect();
                if !failing_records.is_empty() {
                    writeln!(writer)?;
                    writeln!(writer, "{}", "FAILURES:".error())?;
                    writeln!(writer, "**********")?;
                    for record in failing_records {
                        write!(writer, "{}", record)?;
                        writeln!(writer, "**********")?;
                    }
                }
            }
            MetricKind::Recall(recall) => {
                let failing_records: Vec<_> = recall.records.iter().filter(|r| !r.pass).collect();
                if !failing_records.is_empty() {
                    writeln!(writer)?;
                    writeln!(writer, "{}", "RECALL FAILURES:".error())?;
                    writeln!(writer, "**********")?;
                    for record in failing_records {
                        write!(writer, "{}", record)?;
                        writeln!(writer, "**********")?;
                    }
                }
            }
            MetricKind::Correctness(correctness) => {
                let failing_records: Vec<_> = correctness
                    .records
                    .iter()
                    .filter(|r| r.score < 1.0)
                    .collect();
                if !failing_records.is_empty() {
                    writeln!(writer)?;
                    writeln!(writer, "{}", "CORRECTNESS FAILURES:".error())?;
                    writeln!(writer, "**********")?;
                    for record in failing_records {
                        write!(writer, "{}", record)?;
                        writeln!(writer, "**********")?;
                    }
                }
            }
            MetricKind::Execution(execution) => {
                write!(writer, "{}", execution.failure_report())?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Record {
    pub cot: String,
    /// `PASS` / `FAIL` (and `SKIP`). Execution grader: follows the judge
    /// verdict, and is `JUDGE_ERROR` when the judge failed.
    pub choice: String,
    /// Execution grader: the EX bit (1.0 = result sets match), not pass/fail.
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_output: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ReferenceKind>,
    pub duration_ms: f64,
    pub input_tokens: i32,
    pub output_tokens: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft_f1: Option<f32>,
    pub llm_rounds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_sql: Option<String>,
    /// Answer-accuracy judge verdict (`CORRECT` / `INCORRECT`); execution grader only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_reason: Option<String>,
    /// The agent's final answer text the judge read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_text: Option<String>,
    /// The rendered gold-rows table the judge read, for offline re-judging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gold_preview: Option<String>,
    /// Per-question wall-time breakdown (agentic target runs only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<oxy::exec_types::RunTiming>,
    /// Which agent runtime answered: `react` or `fsm`. Absent in records written before the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// The model the agent's LLM calls reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl std::fmt::Display for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = self
            .cot
            .replace("---", &format!("{}", "---".error()))
            .replace("+++", &format!("{}", "+++".success()));

        if self.score < 1.0 {
            writeln!(f, "{}", "Inconsistent result detected.".warning())?;
        }
        writeln!(f, "{}", reason)
    }
}

/// Per-eval run statistics (populated by the generator/solver pipeline).
#[derive(Debug, Serialize, Clone, Default)]
pub struct RunStats {
    /// Total runs attempted (including those that errored).
    pub total_attempted: usize,
    /// Runs that completed without an execution error.
    pub answered: usize,
}

#[derive(Serialize, Clone)]
pub struct EvalResult {
    pub errors: Vec<String>,
    pub metrics: Vec<MetricKind>,
    pub stats: RunStats,
    pub test_name: Option<String>,
}

impl EvalResult {
    pub fn new(errors: Vec<String>, metrics: Vec<MetricKind>, stats: RunStats) -> Self {
        Self {
            errors,
            metrics,
            stats,
            test_name: None,
        }
    }
}

impl std::fmt::Debug for EvalResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.errors.is_empty() {
            writeln!(f)?;
            writeln!(
                f,
                "{}",
                format!("Failed to generate {} outputs:", self.errors.len()).warning()
            )?;
            writeln!(f, "**********")?;
            for error in &self.errors {
                writeln!(f, "{error}")?;
                writeln!(f, "**********")?;
            }
            writeln!(f)?;
        }
        for metric in &self.metrics {
            metric.verbose(f)?;
        }
        Ok(())
    }
}

impl std::fmt::Display for EvalResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", "✅Eval finished with metrics:".primary())?;
        for metric in &self.metrics {
            writeln!(f, "{}", format!("{metric}").primary())?;
        }
        Ok(())
    }
}

impl FromIterator<Record> for MetricKind {
    fn from_iter<T: IntoIterator<Item = Record>>(iter: T) -> Self {
        let records = iter.into_iter().collect::<Vec<_>>();
        let score = if records.is_empty() {
            0.0
        } else {
            records.iter().map(|r| r.score).sum::<f32>() / records.len() as f32
        };
        MetricKind::Similarity(Similarity { score, records })
    }
}

impl FromIterator<RecallRecord> for MetricKind {
    fn from_iter<T: IntoIterator<Item = RecallRecord>>(iter: T) -> Self {
        let records = iter.into_iter().collect::<Vec<_>>();
        let score = if records.is_empty() {
            0.0
        } else {
            records.iter().map(|r| r.score).sum::<f32>() / records.len() as f32
        };
        MetricKind::Recall(Recall { score, records })
    }
}
