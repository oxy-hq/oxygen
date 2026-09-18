use futures::stream::StreamExt;

use oxy::{
    config::{
        constants::{EVAL_METRICS_POSTFIX, EVAL_SOURCE},
        model::SolverKind,
    },
    exec_runtime::ExecutionContext,
    exec_types::{Output, ProgressType, TargetOutput},
};
use oxy_shared::errors::OxyError;

use super::{
    correctness_solver::{parse_correctness_record, render_correctness_prompt},
    types::{Correctness, MetricKind, Record},
};

/// Score one metric over the target outputs, on the plain-async path (no
/// old-executor pipeline). The `.test.yml` entry point only ever builds
/// `SolverKind::Correctness`; the `Similarity` / `ContextRecall` solvers went
/// with the classic-agent eval retirement and are rejected here.
pub(super) async fn run_solver(
    execution_context: &ExecutionContext,
    solver_kind: SolverKind,
    outputs: Vec<(TargetOutput, TargetOutput)>,
    errors_with_expected: Vec<(String, TargetOutput)>,
    concurrency: usize,
) -> Result<MetricKind, OxyError> {
    let SolverKind::Correctness(correctness_solver) = solver_kind else {
        return Err(OxyError::ConfigurationError(
            "Only the correctness solver is supported; the similarity / context-recall solvers \
             were removed with the classic-agent eval retirement."
                .to_string(),
        ));
    };

    let metric_context = execution_context.with_child_source(
        format!("{}-{}", execution_context.source.id, EVAL_METRICS_POSTFIX),
        EVAL_SOURCE.to_string(),
    );
    // The judge reaches the agentic LLM stack through the pipeline's one-shot
    // completer rather than building a provider client directly. Resolve the
    // model (explicit `model_ref`, else the project default) ONCE up front — so
    // a missing model/key fails fast before any judging, and the client is built
    // once and reused across pairs instead of per pair.
    let project_ctx =
        crate::agentic_wiring::OxyProjectContext::new(execution_context.workspace.clone());
    let judge = agentic_pipeline::prepare_one_shot(
        &project_ctx,
        correctness_solver.model_ref.as_deref(),
        "eval-judge",
    )
    .await
    .map_err(OxyError::RuntimeError)?;
    let prompt_template = correctness_solver.prompt.to_string();

    // Judge each (actual, expected) pair concurrently. `buffered` preserves
    // input order, though this arm no longer relies on it (each future pairs its
    // own record). Progress events are re-emitted on `metric_context` — the old
    // `ExecutableBuilder` concurrency wrapper drove the "Judging responses" bar
    // (service/eval.rs) and the Test Dashboard SSE series (service/test.rs) via
    // exactly these. Telemetry writes are best-effort.
    let total = outputs.len();
    if let Err(e) = metric_context
        .write_progress(ProgressType::Started(Some(total)))
        .await
    {
        tracing::warn!("eval: failed to emit judging progress Started: {e}");
    }
    let mut stream = futures::stream::iter(outputs)
        .map(|(actual, expected)| {
            let judge = judge.clone();
            let prompt_template = prompt_template.clone();
            let metric_context = metric_context.clone();
            async move {
                let prompt = render_correctness_prompt(
                    &metric_context,
                    &prompt_template,
                    &actual,
                    &expected,
                )?;
                // The rendered prompt is self-contained; send it as the single
                // user turn (empty system) so every vendor gets a user message.
                let response = judge
                    .complete("", &prompt)
                    .await
                    .map_err(OxyError::RuntimeError)?;
                let mut record = parse_correctness_record(Output::Text(response))?;
                record.prompt = expected.task_description.clone();
                record.expected = Some(expected.output.clone());
                record.actual_output = Some(actual.output.clone());
                record.references = actual.references.clone();
                record.duration_ms = actual.duration_ms;
                record.input_tokens = actual.input_tokens;
                record.output_tokens = actual.output_tokens;
                Ok(record)
            }
        })
        .buffered(concurrency.max(1));
    let mut judged: Vec<Result<Record, OxyError>> = Vec::with_capacity(total);
    while let Some(record) = stream.next().await {
        if let Err(e) = metric_context
            .write_progress(ProgressType::Updated(1))
            .await
        {
            tracing::warn!("eval: failed to emit judging progress Updated: {e}");
        }
        judged.push(record);
    }
    if let Err(e) = metric_context.write_progress(ProgressType::Finished).await {
        tracing::warn!("eval: failed to emit judging progress Finished: {e}");
    }
    let mut records = judged
        .into_iter()
        .collect::<Result<Vec<Record>, OxyError>>()?;

    // Errored runs count as FAILs so the denominator is correct.
    for (error_msg, expected) in &errors_with_expected {
        records.push(Record {
            cot: format!("Run failed with error: {error_msg}"),
            choice: "FAIL".to_string(),
            score: 0.0,
            prompt: expected.task_description.clone(),
            expected: Some(expected.output.clone()),
            actual_output: Some(format!("[ERROR] {error_msg}")),
            references: vec![],
            duration_ms: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        });
    }

    Ok(MetricKind::Correctness(Correctness::from_records(records)))
}
