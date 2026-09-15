//! The `oxy test` subcommand — runs LLM-as-judge evaluations defined in
//! `.test.yml` files.
//!
//! Scope: `.test.yml` files only. The inline `workflow.tests` / `agent.tests`
//! entry points were removed with classic-agent retirement, and the eval engine
//! (`integrations::eval::builders::eval::EvalMapper`) only accepts `.agentic.yml`
//! targets. This command rejects anything else up front so the failure surfaces
//! at the CLI boundary instead of deep inside the executable pipeline.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use tokio::sync::Mutex;
use uuid::Uuid;

use ::oxy::adapters::workspace::builder::WorkspaceBuilder;
use ::oxy::adapters::workspace::manager::WorkspaceManager;
use ::oxy::config::resolve_local_workspace_path;
use oxy_shared::errors::OxyError;

use super::OutputFormat;
use crate::integrations::eval::{EvalResult, JsonReporter, MetricKind, PrettyReporter, Reporter};
use crate::server::service::eval::{
    EvalEventsHandler, SharedTokenStats, TokenStats, run_eval_with_tag,
};
use oxy::config::WorkingCopy;

const TEST_FILE_SUFFIX: &str = ".test.yml";

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum ThresholdMode {
    /// Average of all test accuracies must meet threshold
    Average,
    /// All individual test accuracies must meet threshold
    All,
}

#[derive(Parser, Debug)]
pub struct TestArgs {
    /// Path to a .test.yml file. If omitted, discovers all *.test.yml files.
    pub file: Option<String>,
    /// Filter test cases by tag
    #[clap(long)]
    tag: Option<String>,
    /// Suppress detailed output and show only results summary
    #[clap(long, short = 'q', default_value_t = false)]
    quiet: bool,
    /// Show full detail including agent steps, actual output, and judge reasoning
    #[clap(long, short = 'v', default_value_t = false)]
    verbose: bool,
    /// Output format (pretty or json)
    #[clap(long, value_enum, default_value = "pretty")]
    format: OutputFormat,
    /// Minimum accuracy threshold (0.0-1.0). Exit with code 1 if accuracy is below this value
    #[clap(long, value_name = "THRESHOLD")]
    min_accuracy: Option<f32>,
    /// Threshold mode: 'average' checks average of all tests, 'all' checks each test individually
    #[clap(long, value_enum, default_value = "average")]
    threshold_mode: ThresholdMode,
    /// Write full JSON results to a file (derived from test file name, e.g. sales.agentic.test.results.json)
    #[clap(long)]
    output_json: bool,
    /// Run only a specific test case by 0-based index, name, or prompt string. Requires a file to be
    /// specified. If --tag is also set, both filters apply: the case must match both the
    /// index/name/prompt and the tag.
    #[clap(long, value_name = "CASE")]
    case: Option<String>,
}

pub async fn handle_test_command(test_args: TestArgs) -> Result<(), OxyError> {
    validate_args(&test_args)?;

    let workspace_path = resolve_local_workspace_path()?;
    let workspace_manager = WorkspaceBuilder::new(Uuid::nil())
        .with_working_copy(&workspace_path, None, oxy::config::OnMissing::Fail)
        .await?
        .build()
        .await
        .map_err(|e| OxyError::from(anyhow::anyhow!("Failed to create project: {e}")))?;

    let file_paths = resolve_test_files(&workspace_manager, test_args.file.as_deref()).await?;

    let case_index = match test_args.case.as_deref() {
        // `validate_args` guarantees a file was supplied alongside --case, so
        // `file_paths` holds exactly that one entry.
        Some(case) => resolve_case_index(&workspace_manager, case, &file_paths[0]).await?,
        None => None,
    };

    let token_stats: SharedTokenStats = Arc::new(Mutex::new(TokenStats::default()));
    let start_time = std::time::Instant::now();

    let mut all_results = Vec::new();
    for file_path in &file_paths {
        let results = run_one_file(
            &workspace_manager,
            file_path,
            &test_args,
            case_index,
            &token_stats,
        )
        .await?;
        all_results.extend(results);
    }

    let duration_ms = start_time.elapsed().as_millis() as f64;
    let tokens = token_stats.lock().await.clone();

    emit_reports(&all_results, &test_args, &tokens, duration_ms)?;

    if let Some(min_accuracy) = test_args.min_accuracy {
        enforce_threshold(&all_results, min_accuracy, &test_args.threshold_mode)?;
    }

    Ok(())
}

fn validate_args(test_args: &TestArgs) -> Result<(), OxyError> {
    if let Some(threshold) = test_args.min_accuracy
        && !(0.0..=1.0).contains(&threshold)
    {
        return Err(OxyError::ConfigurationError(format!(
            "min-accuracy must be between 0.0 and 1.0, got: {threshold}"
        )));
    }

    if test_args.case.is_some() && test_args.file.is_none() {
        return Err(OxyError::ConfigurationError(
            "--case requires a specific file to be specified".to_string(),
        ));
    }

    if let Some(file) = test_args.file.as_deref()
        && !file.ends_with(TEST_FILE_SUFFIX)
    {
        return Err(OxyError::ConfigurationError(format!(
            "Unsupported test file: {file}. `oxy test` only runs {TEST_FILE_SUFFIX} files \
             (the inline workflow.tests / agent.tests entry points were removed)."
        )));
    }

    Ok(())
}

/// Resolves the set of `.test.yml` files to run: either the single explicit
/// file argument, or every discoverable test file in the project.
///
/// The `.test.yml` suffix is already enforced by [`validate_args`].
async fn resolve_test_files(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    file: Option<&str>,
) -> Result<Vec<PathBuf>, OxyError> {
    let Some(file) = file else {
        let test_files = workspace_manager.config_manager.list_tests().await?;
        if test_files.is_empty() {
            return Err(OxyError::ConfigurationError(
                "No .test.yml files found in the project".to_string(),
            ));
        }
        return Ok(test_files);
    };

    let current_dir = std::env::current_dir()
        .map_err(|e| OxyError::RuntimeError(format!("Could not get current directory: {e}")))?;
    let file_path = current_dir.join(file);
    if !file_path.exists() {
        return Err(OxyError::ConfigurationError(format!(
            "File not found: {file_path:?}"
        )));
    }

    Ok(vec![file_path])
}

/// Resolves `--case` to a 0-based index. Accepts an integer index, a case
/// `name`, or an exact `prompt` string.
async fn resolve_case_index(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    case_str: &str,
    file_path: &Path,
) -> Result<Option<usize>, OxyError> {
    let test_config = workspace_manager
        .config_manager
        .resolve_test(file_path)
        .await?;

    if let Ok(idx) = case_str.parse::<usize>() {
        if idx >= test_config.cases.len() {
            return Err(OxyError::ConfigurationError(format!(
                "Case index {idx} is out of bounds: {:?} has {} case(s) (0-based)",
                file_path,
                test_config.cases.len()
            )));
        }
        return Ok(Some(idx));
    }

    let mut matching = test_config
        .cases
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name.as_deref() == Some(case_str) || c.prompt == case_str)
        .map(|(i, _)| i);

    let idx = matching.next().ok_or_else(|| {
        OxyError::ConfigurationError(format!(
            "No test case with name or prompt {case_str:?} found in {file_path:?}"
        ))
    })?;

    if matching.next().is_some() {
        tracing::warn!(
            "Multiple cases with name or prompt {:?} found in {:?}; using the first (index {})",
            case_str,
            file_path,
            idx
        );
    }

    Ok(Some(idx))
}

/// Case labels and per-case run count, used to drive the progress bar so it can
/// show which case is currently being worked on.
async fn collect_case_info(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    file_path: &Path,
    test_args: &TestArgs,
    case_index: Option<usize>,
) -> (Vec<String>, usize) {
    let Ok(test_config) = workspace_manager
        .config_manager
        .resolve_test(file_path)
        .await
    else {
        return (vec![], 0);
    };

    let labels = test_config
        .cases
        .iter()
        .enumerate()
        .filter(|(idx, _)| case_index.is_none_or(|i| *idx == i))
        .filter(|(_, c)| test_args.tag.as_ref().is_none_or(|t| c.tags.contains(t)))
        .map(|(_, c)| case_label(c.name.as_deref(), &c.prompt))
        .collect::<Vec<_>>();

    (labels, test_config.settings.runs)
}

/// A case's display label: its `name` if set, otherwise its prompt truncated to
/// 60 characters.
fn case_label(name: Option<&str>, prompt: &str) -> String {
    if let Some(name) = name {
        return name.to_string();
    }
    let prompt = prompt.trim();
    let truncated: String = prompt.chars().take(60).collect();
    if truncated.chars().count() < prompt.chars().count() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

async fn run_one_file(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    file_path: &Path,
    test_args: &TestArgs,
    case_index: Option<usize>,
    token_stats: &SharedTokenStats,
) -> Result<Vec<EvalResult>, OxyError> {
    let file_name = file_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string_lossy().to_string());

    let (case_labels, runs_per_case) =
        collect_case_info(workspace_manager, file_path, test_args, case_index).await;

    let handler = EvalEventsHandler::new(test_args.quiet, Arc::clone(token_stats))
        .with_test_label(file_name.clone())
        .with_case_info(case_labels, runs_per_case);

    let mut results = run_eval_with_tag(
        workspace_manager.clone(),
        file_path,
        case_index,
        test_args.tag.clone(),
        handler,
    )
    .await?;

    for result in &mut results {
        result.test_name = Some(file_name.clone());
    }

    Ok(results)
}

fn emit_reports(
    results: &[EvalResult],
    test_args: &TestArgs,
    tokens: &TokenStats,
    duration_ms: f64,
) -> Result<(), OxyError> {
    let reporter: Box<dyn Reporter> = match test_args.format {
        OutputFormat::Pretty => Box::new(PrettyReporter {
            quiet: test_args.quiet,
            verbose: test_args.verbose,
            total_input_tokens: tokens.total_input_tokens,
            total_output_tokens: tokens.total_output_tokens,
            duration_ms,
        }),
        OutputFormat::Json => Box::new(JsonReporter),
    };
    let mut stdout = std::io::stdout();
    reporter.report(results, &mut stdout)?;

    if test_args.output_json {
        write_json_results(results, test_args.file.as_deref())?;
    }

    Ok(())
}

/// Writes full JSON results to a file, for improvement loops that re-read them.
fn write_json_results(results: &[EvalResult], file: Option<&str>) -> Result<(), OxyError> {
    let output_path = json_results_path(file);
    let handle = std::fs::File::create(&output_path).map_err(|e| {
        OxyError::RuntimeError(format!("Failed to create output file '{output_path}': {e}"))
    })?;
    let mut buf_writer = std::io::BufWriter::new(handle);
    JsonReporter.report(results, &mut buf_writer)?;
    eprintln!("Results written to {output_path}");
    Ok(())
}

fn json_results_path(file: Option<&str>) -> String {
    match file {
        Some(file) => {
            let stem = file.trim_end_matches(".yml").trim_end_matches(".yaml");
            format!("{stem}.results.json")
        }
        None => "test-results.json".to_string(),
    }
}

/// Fails the run when accuracy is below `min_accuracy`. Returning `Err` is what
/// produces the non-zero exit code.
///
/// Note on granularity: for a `.test.yml` run the eval pipeline emits exactly
/// one `Correctness` metric per file (`EvalMapper` builds
/// `metrics: vec![correctness_solver]`, and `Correctness::from_records` averages
/// across every case × run into that single score). So the flattened
/// `accuracies` list below holds one entry per *file*, not per case — and the
/// `Test {n}` labels in the `All`-mode failure message are positions in that
/// flat list, not case indices. This is preserved pre-removal behavior; the
/// message wording is left as it was.
fn enforce_threshold(
    results: &[EvalResult],
    min_accuracy: f32,
    threshold_mode: &ThresholdMode,
) -> Result<(), OxyError> {
    let accuracies: Vec<f32> = results
        .iter()
        .flat_map(|r| &r.metrics)
        .filter_map(|m| match m {
            MetricKind::Similarity(s) => Some(s.score),
            MetricKind::Correctness(c) => Some(c.score),
            _ => None,
        })
        .collect();

    if accuracies.is_empty() {
        eprintln!("Warning: --min-accuracy specified but no accuracy metrics found");
        return Ok(());
    }

    match threshold_mode {
        ThresholdMode::Average => {
            let avg_accuracy: f32 = accuracies.iter().sum::<f32>() / accuracies.len() as f32;
            if avg_accuracy < min_accuracy {
                return Err(OxyError::RuntimeError(format!(
                    "Average accuracy {avg_accuracy:.4} below threshold {min_accuracy:.4}"
                )));
            }
        }
        ThresholdMode::All => {
            let failing_tests: Vec<(usize, f32)> = accuracies
                .iter()
                .enumerate()
                .filter(|(_, acc)| **acc < min_accuracy)
                .map(|(i, acc)| (i, *acc))
                .collect();

            if !failing_tests.is_empty() {
                let failure_msg = failing_tests
                    .iter()
                    .map(|(i, acc)| format!("Test {}: {:.4}", i + 1, acc))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(OxyError::RuntimeError(format!(
                    "{} test(s) below threshold {min_accuracy:.4}: {failure_msg}",
                    failing_tests.len(),
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "test_command_tests.rs"]
mod tests;
