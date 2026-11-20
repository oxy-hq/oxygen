//! Unit tests for the `oxy test` subcommand.
//!
//! Co-located via `#[path = "test_command_tests.rs"] mod tests;` from
//! `test_command.rs` to keep it under the 400-line file limit.
//!
//! Everything here is LLM-free and database-free — argument parsing, pure
//! validation, and the threshold gate.

use super::*;
use clap::Parser;
use oxy_app::surface::eval::{Correctness, Execution, RunStats, Similarity};

/// Builds a `TestArgs` from a CLI-style argv, mirroring how clap parses it in
/// `main`. The leading "oxy" stands in for argv[0].
fn parse(argv: &[&str]) -> TestArgs {
    let mut full = vec!["oxy"];
    full.extend_from_slice(argv);
    TestArgs::parse_from(full)
}

fn result_with_correctness(scores: &[f32]) -> EvalResult {
    let metrics = scores
        .iter()
        .map(|score| {
            MetricKind::Correctness(Correctness {
                score: *score,
                records: vec![],
            })
        })
        .collect();
    EvalResult::new(vec![], metrics, RunStats::default())
}

// ---------------------------------------------------------------- arg parsing

#[test]
fn test_args_parse_with_defaults() {
    let args = parse(&[]);
    assert_eq!(args.file, None);
    assert_eq!(args.tag, None);
    assert!(!args.quiet);
    assert!(!args.verbose);
    assert!(matches!(args.format, OutputFormat::Pretty));
    assert_eq!(args.min_accuracy, None);
    assert!(matches!(args.threshold_mode, ThresholdMode::Average));
    assert!(!args.output_json);
    assert_eq!(args.case, None);
}

#[test]
fn test_args_parse_positional_file() {
    let args = parse(&["sales.agentic.test.yml"]);
    assert_eq!(args.file.as_deref(), Some("sales.agentic.test.yml"));
}

#[test]
fn test_args_parse_every_flag() {
    let args = parse(&[
        "sales.agentic.test.yml",
        "--tag",
        "critical",
        "--quiet",
        "--verbose",
        "--format",
        "json",
        "--min-accuracy",
        "0.8",
        "--threshold-mode",
        "all",
        "--output-json",
        "--case",
        "3",
    ]);
    assert_eq!(args.file.as_deref(), Some("sales.agentic.test.yml"));
    assert_eq!(args.tag.as_deref(), Some("critical"));
    assert!(args.quiet);
    assert!(args.verbose);
    assert!(matches!(args.format, OutputFormat::Json));
    assert_eq!(args.min_accuracy, Some(0.8));
    assert!(matches!(args.threshold_mode, ThresholdMode::All));
    assert!(args.output_json);
    assert_eq!(args.case.as_deref(), Some("3"));
}

#[test]
fn test_args_parse_short_flags() {
    let args = parse(&["-q", "-v"]);
    assert!(args.quiet);
    assert!(args.verbose);
}

// ------------------------------------------------------------ validate_args

#[test]
fn validate_args_accepts_in_range_accuracy() {
    for value in ["0", "0.5", "1"] {
        let args = parse(&["--min-accuracy", value]);
        assert!(
            validate_args(&args).is_ok(),
            "--min-accuracy {value} should be accepted"
        );
    }
}

#[test]
fn validate_args_rejects_out_of_range_accuracy() {
    // The negative case uses the `=` form deliberately. With a space,
    // `--min-accuracy -0.1` never reaches validate_args: clap reads `-0.1` as a
    // flag and bails with "unexpected argument '-0' found". That is inherited
    // clap behavior (unchanged from the original command), not something this
    // command decides, so `=` is the only way to pass a negative threshold.
    for argv in [vec!["--min-accuracy", "1.5"], vec!["--min-accuracy=-0.1"]] {
        let args = parse(&argv);
        let err = validate_args(&args).expect_err("should reject out-of-range accuracy");
        assert!(
            err.to_string().contains("min-accuracy must be between"),
            "unexpected error for {argv:?}: {err}"
        );
    }
}

#[test]
fn validate_args_rejects_case_without_file() {
    let args = parse(&["--case", "0"]);
    let err = validate_args(&args).expect_err("--case without a file should be rejected");
    assert!(err.to_string().contains("--case requires a specific file"));
}

#[test]
fn validate_args_rejects_regrade_without_file() {
    let args = parse(&["--regrade", "old.results.json"]);
    let err = validate_args(&args).expect_err("--regrade without a file should be rejected");
    assert!(err.to_string().contains("--regrade requires the test file"));
}

#[test]
fn regrade_is_refused_alongside_case_or_tag() {
    for extra in [["--case", "0"], ["--tag", "t"]] {
        let mut argv = vec!["oxy", "sales.test.yml", "--regrade", "old.results.json"];
        argv.extend_from_slice(&extra);
        assert!(TestArgs::try_parse_from(argv).is_err(), "{extra:?}");
    }
}

#[test]
fn validate_args_accepts_case_with_file() {
    let args = parse(&["sales.agentic.test.yml", "--case", "0"]);
    assert!(validate_args(&args).is_ok());
}

#[test]
fn validate_args_rejects_non_test_yml_file() {
    // The narrowing this command was restored under: only .test.yml runs.
    for file in [
        "agents/sales.agent.yml",
        "workflows/daily.workflow.yml",
        "sales.agentic.yml",
        "notes.txt",
    ] {
        let args = parse(&[file]);
        let err = validate_args(&args).expect_err("non-.test.yml should be rejected");
        assert!(
            err.to_string().contains("only runs .test.yml files"),
            "unexpected error for {file}: {err}"
        );
    }
}

#[test]
fn validate_args_accepts_test_yml_file() {
    let args = parse(&["examples/testing/analytics.agentic.test.yml"]);
    assert!(validate_args(&args).is_ok());
}

// --------------------------------------------------------- enforce_threshold

#[test]
fn enforce_threshold_average_passes_when_mean_meets_threshold() {
    // mean = 0.75, threshold = 0.75 — the boundary is inclusive (`<`, not `<=`).
    // 0.5/1.0/0.75 are all exactly representable in f32, so this is an exact
    // comparison rather than a knife-edge rounding case.
    let results = vec![result_with_correctness(&[0.5, 1.0])];
    assert!(enforce_threshold(&results, 0.75, &ThresholdMode::Average).is_ok());
}

#[test]
fn enforce_threshold_average_fails_when_mean_below_threshold() {
    // mean = 0.5, threshold = 0.8
    let results = vec![result_with_correctness(&[0.4, 0.6])];
    let err = enforce_threshold(&results, 0.8, &ThresholdMode::Average)
        .expect_err("mean below threshold should fail");
    assert!(err.to_string().contains("Average accuracy"));
    assert!(err.to_string().contains("below threshold"));
}

#[test]
fn enforce_threshold_all_passes_when_every_score_meets_threshold() {
    let results = vec![result_with_correctness(&[0.8, 0.9, 1.0])];
    assert!(enforce_threshold(&results, 0.8, &ThresholdMode::All).is_ok());
}

#[test]
fn enforce_threshold_all_fails_on_a_single_low_score() {
    // Multiple metrics on one result is a synthetic shape — a real .test.yml run
    // emits one Correctness metric per file. It is used here only to exercise
    // the flattening in enforce_threshold; see
    // enforce_threshold_all_labels_by_flat_metric_position for what the numbering
    // actually means.
    //
    // Average would be 0.8 and pass; All must still fail on the 0.4.
    let results = vec![result_with_correctness(&[0.4, 1.0, 1.0])];
    let err = enforce_threshold(&results, 0.8, &ThresholdMode::All)
        .expect_err("one score below threshold should fail in All mode");
    let msg = err.to_string();
    assert!(msg.contains("1 test(s) below threshold"), "got: {msg}");
    assert!(msg.contains("Test 1: 0.4000"), "got: {msg}");
}

#[test]
fn enforce_threshold_all_reports_every_failing_score() {
    let results = vec![result_with_correctness(&[0.1, 1.0, 0.2])];
    let err = enforce_threshold(&results, 0.5, &ThresholdMode::All)
        .expect_err("two scores below threshold should fail");
    let msg = err.to_string();
    assert!(msg.contains("2 test(s) below threshold"), "got: {msg}");
    assert!(msg.contains("Test 1: 0.1000"), "got: {msg}");
    assert!(msg.contains("Test 3: 0.2000"), "got: {msg}");
}

#[test]
fn enforce_threshold_all_labels_by_flat_metric_position() {
    // Pins the real granularity: `Test {n}` counts positions in the flattened
    // metric list, which for a .test.yml run is one entry per FILE (EvalMapper
    // emits a single Correctness metric per file, itself already averaged over
    // every case x run). It is not a case index.
    //
    // Three files; the 1st and 3rd fail.
    let results = vec![
        result_with_correctness(&[0.1]),
        result_with_correctness(&[0.9]),
        result_with_correctness(&[0.3]),
    ];
    let err = enforce_threshold(&results, 0.5, &ThresholdMode::All)
        .expect_err("two files below threshold should fail");
    let msg = err.to_string();
    assert!(msg.contains("2 test(s) below threshold"), "got: {msg}");
    assert!(msg.contains("Test 1: 0.1000"), "got: {msg}");
    assert!(msg.contains("Test 3: 0.3000"), "got: {msg}");
}

#[test]
fn enforce_threshold_aggregates_across_result_files() {
    // Two files, each contributing one score: mean = 0.5, below 0.8.
    let results = vec![
        result_with_correctness(&[0.2]),
        result_with_correctness(&[0.8]),
    ];
    assert!(enforce_threshold(&results, 0.8, &ThresholdMode::Average).is_err());
    assert!(enforce_threshold(&results, 0.1, &ThresholdMode::Average).is_ok());
}

#[test]
fn enforce_threshold_reads_similarity_scores_too() {
    let results = vec![EvalResult::new(
        vec![],
        vec![MetricKind::Similarity(Similarity {
            score: 0.2,
            records: vec![],
        })],
        RunStats::default(),
    )];
    assert!(enforce_threshold(&results, 0.8, &ThresholdMode::Average).is_err());
}

#[test]
fn enforce_threshold_gates_execution_on_answer_accuracy_not_ex() {
    let execution = |answer_acc: f32, ex: f32| {
        let mut e = Execution::from_records(vec![]);
        (e.answer_acc, e.ex) = (answer_acc, ex);
        vec![EvalResult::new(
            vec![],
            vec![MetricKind::Execution(e)],
            RunStats::default(),
        )]
    };
    // High answer accuracy passes even though EX is 0.
    assert!(enforce_threshold(&execution(0.9, 0.0), 0.8, &ThresholdMode::All).is_ok());
    // Perfect EX does not rescue low answer accuracy.
    assert!(enforce_threshold(&execution(0.5, 1.0), 0.8, &ThresholdMode::Average).is_err());
}

#[test]
fn enforce_threshold_without_metrics_warns_but_succeeds() {
    // No accuracy metrics at all — the original behavior is to warn on stderr
    // and let the run pass rather than fail it.
    let results = vec![EvalResult::new(vec![], vec![], RunStats::default())];
    assert!(enforce_threshold(&results, 0.9, &ThresholdMode::Average).is_ok());
    assert!(enforce_threshold(&results, 0.9, &ThresholdMode::All).is_ok());
}

// -------------------------------------------------------- json_results_path

#[test]
fn json_results_path_derives_from_file_name() {
    assert_eq!(
        json_results_path(Some("sales.agentic.test.yml")),
        "sales.agentic.test.results.json"
    );
    assert_eq!(
        json_results_path(Some("a/b/sales.agentic.test.yaml")),
        "a/b/sales.agentic.test.results.json"
    );
}

#[test]
fn json_results_path_defaults_when_discovering_all_files() {
    assert_eq!(json_results_path(None), "test-results.json");
}

// ------------------------------------------------------------- case_label

#[test]
fn case_label_prefers_name() {
    assert_eq!(case_label(Some("signups"), "How many signups?"), "signups");
}

#[test]
fn case_label_falls_back_to_trimmed_prompt() {
    assert_eq!(
        case_label(None, "  How many signups?  "),
        "How many signups?"
    );
}

#[test]
fn case_label_truncates_long_prompts() {
    let prompt = "a".repeat(100);
    let label = case_label(None, &prompt);
    assert_eq!(label.chars().count(), 61, "60 chars plus the ellipsis");
    assert!(label.ends_with('…'));
}

#[test]
fn case_label_does_not_truncate_at_the_boundary() {
    let prompt = "a".repeat(60);
    let label = case_label(None, &prompt);
    assert_eq!(label, prompt);
    assert!(!label.ends_with('…'));
}
