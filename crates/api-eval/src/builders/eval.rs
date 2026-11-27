use oxy::{
    config::{
        constants::EVAL_SOURCE,
        model::{
            CorrectnessSolver, EvalConfig, EvalKind, ExecutionSolver, SolverKind, TestCaseEval,
            default_correctness_prompt,
        },
        test_config::Grader,
    },
    exec_types::EventKind,
};
use oxy_shared::errors::OxyError;

use super::{
    EvalInput, EvalResult, EvalRun,
    generator::run_generator,
    regrade,
    solver::run_solver,
    types::{AgenticInput, RegradeSource, RunStats},
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
        regrade_from: _,
        test,
    } = eval_input;

    if !target_ref.ends_with("test.yml") {
        return Err(OxyError::ConfigurationError(format!(
            "Invalid file extension: {target_ref}. Expected .test.yml (the inline \
             workflow.tests / agent.tests entry points were removed with classic agent retirement)."
        )));
    }

    let test_config = match test {
        Some(test) => test,
        None => {
            execution_context
                .workspace
                .config_manager
                .resolve_test(&target_ref)
                .await?
        }
    };
    test_config
        .validate_grading()
        .map_err(|e| OxyError::ConfigurationError(format!("{target_ref}: {e}")))?;
    let resolved_target = test_config.target.ok_or_else(|| {
        OxyError::ConfigurationError(format!(
            "Could not determine target for test file: {target_ref}"
        ))
    })?;

    // Only `.agentic.yml` agents are runnable as eval targets: classic agents
    // are retired and the compiler refuses them.
    if !(resolved_target.ends_with("agentic.yml") || resolved_target.ends_with("agentic.yaml")) {
        return Err(OxyError::ConfigurationError(format!(
            "Unsupported test target: {resolved_target}. Expected .agentic.yml or .agentic.yaml \
             — classic .agent.yml agents are retired; `oxy migrate-agents` converts them."
        )));
    }
    let eval_target = AgenticInput {
        config_path: resolved_target,
        prompt: String::new(),
    };

    let grading_solver = match (&test_config.settings.grader, &test_config.settings.database) {
        (Grader::Execution, Some(database)) => SolverKind::Execution(ExecutionSolver {
            database: database.clone(),
            model_ref: test_config.settings.judge_model.clone(),
        }),
        _ => SolverKind::Correctness(CorrectnessSolver {
            prompt: default_correctness_prompt(),
            model_ref: test_config.settings.judge_model.clone(),
        }),
    };

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
            metrics: vec![grading_solver],
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
    regrade_from: Option<&RegradeSource>,
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
    let (outputs, errors_with_expected) = match regrade_from {
        Some(source) => {
            refuse_regrade_unless_execution(&eval.metrics)?;
            regrade::load(source, test_cases(&eval.kind))?
        }
        None => {
            let ran = run_generator(&eval_context, eval.kind, target.clone(), eval.concurrency);
            let ran = ran.await?;
            super::memory_merge::after_pass(&eval_context, &target).await;
            ran
        }
    };

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

/// The test file's cases (after `--case` / `--tag`), whose `expect:` checks a
/// regrade applies to the recorded runs.
fn test_cases(kind: &EvalKind) -> &[oxy::config::test_config::TestCase] {
    match kind {
        EvalKind::TestCase(TestCaseEval { cases, .. }) => cases,
        _ => &[],
    }
}

/// A regrade rebuilds the execution grader's inputs only, so the test file
/// must still grade by execution.
fn refuse_regrade_unless_execution(metrics: &[SolverKind]) -> Result<(), OxyError> {
    if metrics
        .iter()
        .all(|m| matches!(m, SolverKind::Execution(_)))
    {
        Ok(())
    } else {
        Err(OxyError::ConfigurationError(
            "--regrade needs a test file graded by `grader: execution`".to_string(),
        ))
    }
}

/// Resolve then run the evals for an `EvalInput`, one result per eval.
pub(crate) async fn run_eval(
    execution_context: &EvalRun,
    eval_input: EvalInput,
) -> Result<Vec<Result<EvalResult, OxyError>>, OxyError> {
    // A `.test.yml` resolves to exactly one eval (all its cases run under it);
    // the `Vec<Result<..>>` return shape is kept for `launch`'s callers.
    let regrade_from = eval_input.regrade_from.clone();
    let (eval, target) = resolve_eval_inputs(execution_context, eval_input).await?;
    Ok(vec![
        run_one_eval(execution_context, 0, eval, target, regrade_from.as_ref()).await,
    ])
}
