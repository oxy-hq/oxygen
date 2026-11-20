use std::io::Write;

use indexmap::IndexMap;

use oxy::exec_types::ReferenceKind;
use oxy::theme::StyledText;
use oxy_app::surface::eval::{Correctness, EvalResult, MetricKind};
use oxy_shared::errors::OxyError;

use super::Reporter;

fn format_number(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

pub struct PrettyReporter {
    pub quiet: bool,
    pub verbose: bool,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub duration_ms: f64,
}

impl PrettyReporter {
    fn write_thin_separator(writer: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            writer,
            " ───────────────────────────────────────────────────────────────────────"
        )
    }

    fn truncate_prompt(prompt: &str, max_len: usize) -> String {
        let chars: Vec<char> = prompt.chars().collect();
        if chars.len() <= max_len {
            prompt.to_string()
        } else {
            let truncated: String = chars[..max_len - 3].iter().collect();
            format!("{truncated}...")
        }
    }

    fn pad_right(s: &str, width: usize) -> String {
        let char_count = s.chars().count();
        if char_count >= width {
            s.to_string()
        } else {
            format!("{s}{}", " ".repeat(width - char_count))
        }
    }

    fn write_references(
        writer: &mut dyn Write,
        references: &[ReferenceKind],
    ) -> std::io::Result<()> {
        for reference in references {
            match reference {
                ReferenceKind::SemanticQuery(sq) => {
                    let topic = sq.topic.as_deref().unwrap_or("(no topic)");
                    if let Some(sql) = &sq.sql_query {
                        let sql = sql.replace('\n', " ");
                        let sql = Self::truncate_prompt(&sql, 120);
                        writeln!(
                            writer,
                            "        semantic_query({}, \"{}\") => {}",
                            sq.database, topic, sql
                        )?;
                    } else {
                        writeln!(
                            writer,
                            "        semantic_query({}, \"{}\")",
                            sq.database, topic
                        )?;
                    }
                }
                ReferenceKind::SqlQuery(q) => {
                    let sql = q.sql_query.replace('\n', " ");
                    let sql = Self::truncate_prompt(&sql, 120);
                    writeln!(writer, "        execute_sql({}, \"{}\")", q.database, sql)?;
                }
                ReferenceKind::Retrieval(_) => {
                    writeln!(writer, "        retrieval(...)")?;
                }
                ReferenceKind::DataApp(da) => {
                    writeln!(writer, "        data_app({})", da.file_path.display())?;
                }
            }
        }
        Ok(())
    }

    fn format_score(label: &str, score: f32) -> String {
        let score_str = format!("{label}: {:.1}%", score * 100.0);
        if score >= 0.8 {
            score_str.success().to_string()
        } else if score >= 0.5 {
            score_str.warning().to_string()
        } else {
            score_str.error().to_string()
        }
    }
}

impl Reporter for PrettyReporter {
    fn report(&self, results: &[EvalResult], writer: &mut dyn Write) -> Result<(), OxyError> {
        writeln!(writer)?;

        // Aggregate stats across all eval results
        let total_attempted: usize = results.iter().map(|r| r.stats.total_attempted).sum();
        let answered: usize = results.iter().map(|r| r.stats.answered).sum();

        // Execution cases pass on the judge verdict (EX is not pass/fail).
        let (total_cases, passing_cases) = results
            .iter()
            .flat_map(|r| &r.metrics)
            .map(|m| match m {
                MetricKind::Correctness(c) => correctness_tally(c),
                MetricKind::Execution(e) => e.case_tally(),
                _ => (0, 0),
            })
            .fold((0usize, 0usize), |(t, p), (n, k)| (t + n, p + k));

        // Per-test results
        for result in results {
            if !result.errors.is_empty() {
                writeln!(
                    writer,
                    " {} {}",
                    "Errors:".error(),
                    format!("{} output(s) failed to generate", result.errors.len()).error()
                )?;
                if !self.quiet {
                    for error in &result.errors {
                        writeln!(writer, "   {error}")?;
                    }
                }
                writeln!(writer)?;
            }

            for metric in &result.metrics {
                match metric {
                    MetricKind::Correctness(correctness) => {
                        self.write_correctness(writer, correctness, result.test_name.as_deref())?;
                    }
                    MetricKind::Execution(e) => {
                        if let Some(name) = &result.test_name {
                            writeln!(writer, " {} {name}", "●".text())?;
                        }
                        writeln!(writer)?;
                        writeln!(
                            writer,
                            "   {} ({}/{})  ·  {} ({}/{} match)  ·  {}  ·  judge errors: {}  ·  graded {}  ·  skipped {}",
                            Self::format_score("answer accuracy", e.answer_acc),
                            e.correct_answers(),
                            e.judged(),
                            Self::format_score("EX", e.ex),
                            e.ex_matches(),
                            e.graded,
                            Self::format_score("Soft-F1", e.soft_f1),
                            e.judge_errors,
                            e.graded,
                            e.skipped
                        )?;
                        if let Some(model) = &e.judge_model {
                            writeln!(writer, "   judge: {model}")?;
                        }
                        writeln!(writer)?;
                        if self.verbose {
                            for r in &e.records {
                                writeln!(
                                    writer,
                                    "   {} ex={} judge={} soft-f1={} rounds={} sql={}",
                                    r.choice,
                                    if r.score >= 1.0 { "match" } else { "mismatch" },
                                    r.judge_verdict.as_deref().unwrap_or("-"),
                                    r.soft_f1.map_or("-".to_string(), |v| format!("{v:.2}")),
                                    r.llm_rounds,
                                    r.final_sql.as_deref().unwrap_or("-")
                                )?;
                            }
                            writeln!(writer)?;
                        }
                    }
                    MetricKind::Similarity(similarity) => {
                        if let Some(name) = &result.test_name {
                            writeln!(writer, " {} {name}", "●".text())?;
                        }
                        writeln!(writer)?;

                        let passing = similarity.records.iter().filter(|r| r.score >= 1.0).count();
                        let total = similarity.records.len();

                        writeln!(
                            writer,
                            "   {}  ·  {passing}/{total} passing",
                            Self::format_score("Accuracy", similarity.score)
                        )?;
                        writeln!(writer)?;

                        if !self.quiet {
                            metric.verbose_write(writer)?;
                        }
                    }
                    MetricKind::Recall(recall) => {
                        if let Some(name) = &result.test_name {
                            writeln!(writer, " {} {name}", "●".text())?;
                        }
                        writeln!(writer)?;

                        writeln!(writer, "   {}", Self::format_score("Recall", recall.score))?;
                        writeln!(writer)?;

                        if !self.quiet {
                            metric.verbose_write(writer)?;
                        }
                    }
                }
            }
        }

        // Summary at the bottom
        Self::write_thin_separator(writer)?;
        writeln!(writer)?;

        if total_cases > 0 {
            let pct = passing_cases as f32 / total_cases as f32 * 100.0;
            let label = format!(
                " Tests:        {}/{} passed ({:.1}%)",
                passing_cases, total_cases, pct
            );
            if passing_cases == total_cases {
                writeln!(writer, "{}", label.success())?;
            } else if passing_cases as f32 / total_cases as f32 >= 0.5 {
                writeln!(writer, "{}", label.warning())?;
            } else {
                writeln!(writer, "{}", label.error())?;
            }
        }

        if total_attempted > 0 {
            let answer_pct = answered as f32 / total_attempted as f32 * 100.0;
            writeln!(
                writer,
                " Answer Rate:  {}/{} ({:.1}%)",
                answered, total_attempted, answer_pct
            )?;
        }

        if self.duration_ms > 0.0 {
            let total_secs = self.duration_ms / 1000.0;
            let total_str = if total_secs >= 60.0 {
                let m = (total_secs / 60.0) as u64;
                let s = total_secs % 60.0;
                format!("{m}m {s:.1}s")
            } else {
                format!("{total_secs:.1}s")
            };
            writeln!(writer, " Total Time:   {total_str}")?;

            if total_attempted > 0 {
                let avg_secs = self.duration_ms / 1000.0 / total_attempted as f64;
                writeln!(writer, " Avg Time:     {:.1}s per run", avg_secs)?;
            }
        }

        let total_tokens = self.total_input_tokens + self.total_output_tokens;
        if total_tokens > 0 {
            writeln!(
                writer,
                " Tokens:       {} ({} in / {} out)",
                format_number(total_tokens),
                format_number(self.total_input_tokens),
                format_number(self.total_output_tokens),
            )?;
        }

        writeln!(writer)?;
        Ok(())
    }
}

/// `(cases, passing cases)`: runs grouped by prompt; a case passes when every run does.
fn correctness_tally(c: &Correctness) -> (usize, usize) {
    let mut prompts: std::collections::HashMap<&str, (usize, usize)> =
        std::collections::HashMap::new();
    for record in &c.records {
        let entry = prompts
            .entry(record.prompt.as_deref().unwrap_or("(unknown)"))
            .or_default();
        entry.0 += 1;
        if record.score >= 1.0 {
            entry.1 += 1;
        }
    }
    let passing = prompts.values().filter(|(t, pass)| pass == t).count();
    (prompts.len(), passing)
}

impl PrettyReporter {
    fn write_correctness(
        &self,
        writer: &mut dyn Write,
        correctness: &Correctness,
        test_name: Option<&str>,
    ) -> std::io::Result<()> {
        // Test file header
        if let Some(name) = test_name {
            writeln!(writer, " {} {name}", "●".text())?;
        }
        writeln!(writer)?;

        // Group records by prompt to show per-case results (IndexMap preserves insertion order)
        let mut case_map: IndexMap<String, CaseResult> = IndexMap::new();
        for record in &correctness.records {
            let prompt = record
                .prompt
                .as_deref()
                .unwrap_or("(unknown prompt)")
                .to_string();
            let case = case_map
                .entry(prompt.clone())
                .or_insert_with(|| CaseResult {
                    prompt,
                    total: 0,
                    passed: 0,
                    failing_runs: vec![],
                });
            case.total += 1;
            if record.score >= 1.0 {
                case.passed += 1;
            } else {
                case.failing_runs.push(FailingRun {
                    reasoning: record.cot.clone(),
                    actual_output: record.actual_output.clone(),
                    references: record.references.clone(),
                });
            }
        }
        let cases: Vec<CaseResult> = case_map.into_values().collect();

        let total_cases = cases.len();
        let passing_cases = cases.iter().filter(|c| c.passed == c.total).count();

        for case in &cases {
            let display_prompt = Self::truncate_prompt(&case.prompt, 55);
            let padded_prompt = Self::pad_right(&display_prompt, 58);
            let runs_label = format!("({}/{})", case.passed, case.total);

            if case.passed == case.total {
                writeln!(
                    writer,
                    "   {} {padded_prompt} {}",
                    "PASS".success(),
                    runs_label.success()
                )?;
            } else if case.passed > 0 {
                writeln!(
                    writer,
                    "   {} {padded_prompt} {}",
                    "FLKY".warning(),
                    runs_label.warning()
                )?;
            } else {
                writeln!(
                    writer,
                    "   {} {padded_prompt} {}",
                    "FAIL".error(),
                    runs_label.error()
                )?;
            }
        }

        writeln!(writer)?;
        writeln!(
            writer,
            "   {}  ·  {passing_cases}/{total_cases} cases passing",
            Self::format_score("Correctness", correctness.score)
        )?;
        writeln!(writer)?;

        if !self.quiet {
            let failing_cases: Vec<&CaseResult> =
                cases.iter().filter(|c| c.passed < c.total).collect();
            if !failing_cases.is_empty() {
                writeln!(writer, "   {}", "Failures:".error())?;
                writeln!(writer)?;

                for case in failing_cases {
                    writeln!(
                        writer,
                        "   {} \"{}\"  ({}/{})",
                        "×".error(),
                        case.prompt,
                        case.passed,
                        case.total,
                    )?;
                    for (i, run) in case.failing_runs.iter().enumerate() {
                        if case.failing_runs.len() > 1 {
                            writeln!(writer, "     --- Run {} ---", i + 1)?;
                        }
                        if self.verbose {
                            if !run.references.is_empty() {
                                writeln!(writer, "     {}", "Steps:".secondary())?;
                                Self::write_references(writer, &run.references)?;
                            }
                            if let Some(actual) = &run.actual_output {
                                writeln!(writer, "     {}", "Agent output:".secondary())?;
                                for line in actual.lines() {
                                    writeln!(writer, "       {line}")?;
                                }
                            }
                        }
                        writeln!(writer, "     {}", "Reasoning:".secondary())?;
                        for line in run.reasoning.lines() {
                            writeln!(writer, "       {}", line.secondary())?;
                        }
                        writeln!(writer)?;
                    }
                }
            }
        }

        Ok(())
    }
}

struct CaseResult {
    prompt: String,
    total: usize,
    passed: usize,
    failing_runs: Vec<FailingRun>,
}

struct FailingRun {
    reasoning: String,
    actual_output: Option<String>,
    references: Vec<ReferenceKind>,
}
