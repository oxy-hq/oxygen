use oxy::{
    config::{
        constants::EVAL_SOURCE,
        model::{
            CorrectnessSolver, EvalConfig, EvalKind, SolverKind, TestCaseEval,
            default_correctness_prompt,
        },
    },
    exec_types::EventKind,
};
use oxy_shared::errors::OxyError;

use super::{
    EvalInput, EvalResult, EvalRun,
    generator::run_generator,
    solver::run_solver,
    types::{AgenticInput, RunStats},
};

/// Resolve an `EvalInput` (`.test.yml` reference + optional case index / tag
/// filters) into the concrete evals to run. Only `.test.yml` files targeting an
/// `.agentic.yml` agent are supported — the classic `.agent.yml` runtime and the
/// inline `agent.tests` / `workflow.tests` entry points were removed.
async fn resolve_eval_inputs(
    execution_context: &EvalRun,
    eval_input: EvalInput,
) -> Result<(EvalConfig, AgenticInput), OxyError> {
    let EvalInput {
        target_ref,
        index,
        tag,
    } = eval_input;

    if !target_ref.ends_with("test.yml") {
        return Err(OxyError::ConfigurationError(format!(
            "Invalid file extension: {target_ref}. Expected .test.yml (the inline \
             workflow.tests / agent.tests entry points were removed with classic agent retirement)."
        )));
    }

    let config_manager = &execution_context.workspace.config_manager;
    let test_config = config_manager.resolve_test(&target_ref).await?;
    let resolved_target = test_config.target.ok_or_else(|| {
        OxyError::ConfigurationError(format!(
            "Could not determine target for test file: {target_ref}"
        ))
    })?;

    // Only `.agentic.yml` agents are runnable as eval targets. The classic
    // `.agent.yml` runtime was retired with the oxy-agent crate.
    if !(resolved_target.ends_with("agentic.yml") || resolved_target.ends_with("agentic.yaml")) {
        return Err(OxyError::ConfigurationError(format!(
            "Unsupported test target: {resolved_target}. Expected .agentic.yml or .agentic.yaml \
             — the classic .agent.yml runtime was removed."
        )));
    }
    let eval_target = AgenticInput {
        config_path: resolved_target,
        prompt: String::new(),
    };

    let correctness_solver = SolverKind::Correctness(CorrectnessSolver {
        prompt: default_correctness_prompt(),
        model_ref: test_config.settings.judge_model.clone(),
    });

    // Filter cases by index and/or tag.
    let cases: Vec<_> = test_config
        .cases
        .into_iter()
        .enumerate()
        .filter(|(idx, _)| index.is_none_or(|i| *idx == i))
        .filter(|(_, c)| {
            tag.as_ref()
                .is_none_or(|tag_filter| c.tags.contains(tag_filter))
        })
        .map(|(_, c)| c)
        .collect();

    Ok((
        EvalConfig {
            kind: EvalKind::TestCase(TestCaseEval {
                cases,
                runs: test_config.settings.runs,
                judge_model: test_config.settings.judge_model,
            }),
            metrics: vec![correctness_solver],
            concurrency: test_config.settings.concurrency,
            task_ref: None,
        },
        eval_target,
    ))
}

/// Run one resolved eval: generate the target outputs, then score each metric.
async fn run_one_eval(
    execution_context: &EvalRun,
    idx: usize,
    eval: EvalConfig,
    target: AgenticInput,
) -> Result<EvalResult, OxyError> {
    let eval_context =
        execution_context.with_child_source(format!("eval-{idx}"), EVAL_SOURCE.to_string());
    eval_context
        .write_kind(EventKind::Started {
            name: format!("{target}::Test{idx}"),
            attributes: Default::default(),
        })
        .await?;

    eval_context
        .write_kind(EventKind::Message {
            message: "🔄Generating outputs".to_string(),
        })
        .await?;
    let (outputs, errors_with_expected) =
        run_generator(&eval_context, eval.kind, target, eval.concurrency).await?;

    let answered = outputs.len();
    let total_attempted = answered + errors_with_expected.len();
    let error_strings: Vec<String> = errors_with_expected
        .iter()
        .map(|(e, _)| e.clone())
        .collect();
    let stats = RunStats {
        total_attempted,
        answered,
    };

    eval_context
        .write_kind(EventKind::Message {
            message: "🔄Evaluating records".to_string(),
        })
        .await?;

    // Score each configured metric (only `Correctness` on the `.test.yml` path).
    let mut metrics = Vec::with_capacity(eval.metrics.len());
    for solver in eval.metrics {
        metrics.push(
            run_solver(
                execution_context,
                solver,
                outputs.clone(),
                errors_with_expected.clone(),
                eval.concurrency,
            )
            .await?,
        );
    }

    let result = EvalResult::new(error_strings, metrics, stats);
    eval_context
        .write_kind(EventKind::Finished {
            message: format!("{result:?}"),
            attributes: Default::default(),
            error: None,
        })
        .await?;
    Ok(result)
}

/// Resolve then run the evals for an `EvalInput`, one result per eval.
pub(crate) async fn run_eval(
    execution_context: &EvalRun,
    eval_input: EvalInput,
) -> Result<Vec<Result<EvalResult, OxyError>>, OxyError> {
    // A `.test.yml` resolves to exactly one eval (all its cases run under it);
    // the `Vec<Result<..>>` return shape is kept for `launch`'s callers.
    let (eval, target) = resolve_eval_inputs(execution_context, eval_input).await?;
    Ok(vec![run_one_eval(execution_context, 0, eval, target).await])
}
