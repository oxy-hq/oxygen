//! The results document, pinned byte for byte against the engine's own record
//! constructors and serializer. `oxy test --output-json` wrote it, an eval
//! run's row stores it, and `oxyc evals run --output-json` writes it verbatim;
//! customer agents parse it (skill `oxy-evals/references/results-json.md`).
//! A change here is a contract change: update the fixture, the skill reference
//! and `sdk/cli`'s copy of the fixture together.

use oxy::exec_types::TargetOutput;

use super::solver::{errored_record, judged_record};
use super::types::{Correctness, EvalResult, MetricKind, RunStats};

const FIXTURE: &str = include_str!("fixtures/results.json");

fn expected_side() -> TargetOutput {
    TargetOutput {
        output: "Total revenue across all stores in 2025 was approximately $6.7 billion."
            .to_string(),
        task_description: Some("What was total revenue across all stores in 2025?".to_string()),
        ..Default::default()
    }
}

fn engine_output() -> Vec<EvalResult> {
    let actual = TargetOutput {
        output: "Total revenue in 2025 was $6.74B across 45 stores.".to_string(),
        duration_ms: 18234.5,
        input_tokens: 41210,
        output_tokens: 1830,
        ..Default::default()
    };
    let passed = judged_record(
        "1. Key facts: $6.7B.\n2. The answer states $6.74B.\nVerdict: PASS".to_string(),
        &actual,
        &expected_side(),
    )
    .unwrap();
    let errored = errored_record(
        "agentic pipeline asked a clarifying question: Which region do you mean?",
        &expected_side(),
    );
    let metric = MetricKind::Correctness(Correctness::from_records(vec![passed, errored]));
    let mut result = EvalResult::new(
        vec!["agentic pipeline asked a clarifying question: Which region do you mean?".to_string()],
        vec![metric],
        RunStats {
            total_attempted: 2,
            answered: 1,
        },
    );
    result.test_name = Some("sales.performance.test.yml".to_string());
    vec![result]
}

#[test]
fn the_results_document_holds_byte_for_byte() {
    let written = crate::reporters::results_json(&engine_output()).unwrap();
    assert_eq!(
        written, FIXTURE,
        "the results document changed shape; it is a contract (see this module's doc)"
    );
}

/// Whole-number scores keep their `.0`: a JSON round trip in JavaScript would
/// write `1`, which is why `oxyc` writes the server's text, never a re-encoding.
#[test]
fn whole_scores_keep_their_decimal_point() {
    let written = crate::reporters::results_json(&engine_output()).unwrap();
    assert!(written.contains("\"score\": 1.0,"), "{written}");
    assert!(written.contains("\"score\": 0.0,"), "{written}");
    assert!(written.ends_with("]\n"));
}

/// `oxyc`'s tests hold its writer to a copy of the same document; the two must
/// not drift apart.
#[test]
fn oxyc_pins_the_same_bytes() {
    let oxyc = include_str!("../../../../sdk/cli/fixtures/evals/results.json");
    assert_eq!(
        oxyc, FIXTURE,
        "sdk/cli/fixtures/evals/results.json drifted from this fixture"
    );
}

#[test]
fn the_gate_reads_the_correctness_score() {
    let gated: Vec<f32> = engine_output()[0].gated_accuracies().collect();
    assert_eq!(gated, vec![0.5]);
}
