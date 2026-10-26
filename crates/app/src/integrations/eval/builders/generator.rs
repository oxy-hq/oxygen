use futures::stream::StreamExt;

use oxy::{
    config::model::EvalKind,
    exec_types::{EventKind, ProgressType, TargetOutput, Usage},
};
use oxy_shared::errors::OxyError;

use super::{EvalRun, target_agentic::run_target, types::AgenticInput};

/// Run the eval target for every case × run and pair each result with its
/// expected answer, on the plain-async agentic path (no old-executor
/// pipeline). Returns `(successful (actual, expected) pairs, (error, expected)
/// pairs)` for the solver to score.
///
/// Only `EvalKind::TestCase` is reachable — the `.test.yml` mapper is the sole
/// `EvalConfig` constructor. The `Consistency` / `Custom` kinds fed the inline
/// `agent.tests` / `workflow.tests` blocks, which were removed with the
/// classic-agent retirement, so they are rejected here.
pub(super) async fn run_generator(
    execution_context: &EvalRun,
    eval_kind: EvalKind,
    target: AgenticInput,
    concurrency: usize,
) -> Result<
    (
        Vec<(TargetOutput, TargetOutput)>,
        Vec<(String, TargetOutput)>,
    ),
    OxyError,
> {
    let EvalKind::TestCase(test_case_eval) = eval_kind else {
        return Err(OxyError::ConfigurationError(
            "Only .test.yml (TestCase) evals are supported; the consistency / custom kinds went \
             with the classic-agent inline-tests retirement."
                .to_string(),
        ));
    };

    let runs = test_case_eval.runs;

    // Flatten all cases × runs into a single concurrent batch.
    let mut all_targets = Vec::new();
    let mut expected_outputs = Vec::new();
    for case in &test_case_eval.cases {
        let expected = TargetOutput {
            output: case.expected.clone(),
            task_description: Some(case.prompt.clone()),
            relevant_contexts: vec![],
            references: vec![],
            duration_ms: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        };
        for _ in 0..runs {
            all_targets.push(AgenticInput {
                config_path: target.config_path.clone(),
                prompt: case.prompt.clone(),
            });
            expected_outputs.push(expected.clone());
        }
    }

    // `run_target` is scaffolding-free, so the two side-effects the old
    // `ExecutableBuilder` produced are re-emitted here at the call site:
    // progress events (CLI bar + Test Dashboard SSE) and per-run
    // `EventKind::Usage` (run-level `TokenStats`). Telemetry writes are
    // best-effort — a failed emit must not cancel in-flight runs or skip
    // `Finished`. `buffered` preserves input order so results still line up
    // with `expected_outputs`.
    let total = all_targets.len();
    if let Err(e) = execution_context
        .write_progress(ProgressType::Started(Some(total)))
        .await
    {
        tracing::warn!("eval: failed to emit target progress Started: {e}");
    }
    let workspace = execution_context.workspace.clone();
    let mut stream = futures::stream::iter(all_targets)
        .map(|input| {
            let workspace = workspace.clone();
            async move { run_target(&workspace, input).await }
        })
        .buffered(concurrency.max(1));
    // `Updated(1)` fires in input order as `buffered` yields (not at real
    // completion) — deliberate: it keeps the `case n/m` label monotonic.
    let mut results: Vec<Result<Vec<TargetOutput>, OxyError>> = Vec::with_capacity(total);
    while let Some(result) = stream.next().await {
        if let Ok(outputs) = &result {
            for out in outputs {
                if let Err(e) = execution_context
                    .write_kind(EventKind::Usage {
                        usage: Usage::new(out.input_tokens, out.output_tokens),
                    })
                    .await
                {
                    tracing::warn!("eval: failed to emit target token usage: {e}");
                }
            }
        }
        if let Err(e) = execution_context
            .write_progress(ProgressType::Updated(1))
            .await
        {
            tracing::warn!("eval: failed to emit target progress Updated: {e}");
        }
        results.push(result);
    }
    if let Err(e) = execution_context
        .write_progress(ProgressType::Finished)
        .await
    {
        tracing::warn!("eval: failed to emit target progress Finished: {e}");
    }

    // Pair results back with their expected outputs.
    let mut all_outputs = Vec::new();
    let mut all_errors = Vec::new();
    for (result, expected) in results.into_iter().zip(expected_outputs) {
        match result {
            Ok(actual_outputs) => {
                for actual in actual_outputs {
                    all_outputs.push((actual, expected.clone()));
                }
            }
            Err(err) => {
                // Pair the error with its expected output so the solver can
                // count it as a FAIL against the correct denominator.
                all_errors.push((err.to_string(), expected));
            }
        }
    }

    Ok((all_outputs, all_errors))
}
