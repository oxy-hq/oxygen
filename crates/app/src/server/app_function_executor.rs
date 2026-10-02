//! `AppFunctionTaskExecutor`: runs a **scheduled** custom-app Oxy Function as a
//! `TaskSpec::Custom { kind: "app_function" }` on the global-run fleet. Registered
//! into the `CustomTaskRegistry` by `server::router::recovery`;
//! `PipelineTaskExecutor` delegates the `app_function` kind here. The actual run
//! is `custom_apps_functions::run_scheduled_function` (org-owner identity,
//! `mode="schedule"` invocation record). See
//! `internal-docs/customer-apps-functions.md`.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_pipeline::app_function_task::AppFunctionTask;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::DatabaseConnection;
use sentry::SentryFutureExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

/// The `kind` discriminator for a scheduled custom-app function Custom task.
pub use agentic_pipeline::app_function_task::APP_FUNCTION_KIND;

/// A queued task, read and checked: everything the job needs, typed.
#[derive(Debug)]
struct JobRequest {
    app_id: uuid::Uuid,
    function_name: String,
    mode: String,
    input: Vec<u8>,
    traceparent: Option<String>,
    environment: AppEnvironment,
}

/// Read an `app_function` task.
///
/// **The environment is carried, never dropped.** A worker that ignored it
/// would run a staging task on production's build and secrets
/// (`internal-docs/2026-09-10-custom-app-environments-design.md` §3.4). An
/// unknown name is refused here; a known one rides to
/// `run_scheduled_function`, which resolves that environment's build and
/// admits the run through `check_run::admit_queued` — outside production only
/// a function that build marks `check: true` runs, under that environment's
/// policy; everything else is refused there, in one place.
fn read_task(spec: &TaskSpec) -> Result<JobRequest, String> {
    let TaskSpec::Custom { kind, payload } = spec else {
        return Err(format!(
            "unexpected spec for AppFunctionTaskExecutor: {spec:?}"
        ));
    };
    if kind != APP_FUNCTION_KIND {
        return Err(format!("unknown app_function kind: {kind}"));
    }
    let task = AppFunctionTask::from_payload(payload)?;
    let environment = match task.environment.as_deref() {
        None => AppEnvironment::Production,
        Some(name) => AppEnvironment::parse(name)
            .ok_or_else(|| format!("app_function task names unknown environment {name:?}"))?,
    };
    let app_id: uuid::Uuid = task
        .app_id
        .parse()
        .map_err(|e| format!("bad app_id: {e}"))?;
    // Optional input params (JSON) supplied at trigger time — serialized back
    // to the request-body bytes the isolate receives as `req`. Absent (cron
    // fire) → empty body.
    let input: Vec<u8> = match task.input.as_ref().filter(|v| !v.is_null()) {
        // Propagate a serialize failure instead of silently defaulting to an
        // empty body: running the isolate with `req.body = ""` would execute
        // the function against no input and mask the malformed trigger.
        Some(v) => serde_json::to_vec(v)
            .map_err(|e| format!("failed to serialize app_function input: {e}"))?,
        None => Vec::new(),
    };
    Ok(JobRequest {
        app_id,
        mode: invocation_mode(task.trigger.as_deref()),
        function_name: task.function_name,
        input,
        traceparent: task.traceparent,
        environment,
    })
}

pub struct AppFunctionTaskExecutor {
    pub db: DatabaseConnection,
    /// The node's Layer-1 preagg cache, injected by `build_custom_task_registry`
    /// so a scheduled function's `ctx.semantic` resolves rollups the same way an
    /// HTTP-invoked one does. Default (no cache) compiles to warehouse SQL.
    pub preagg: crate::server::api::middlewares::workspace_context::PreaggCacheCtx,
}

#[async_trait]
impl TaskExecutor for AppFunctionTaskExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let JobRequest {
            app_id,
            function_name,
            mode,
            input,
            traceparent,
            environment,
        } = read_task(&assignment.spec)?;

        // The worker's run is a root trace of its own; this span is that root,
        // linked — not parented — to the request that enqueued the task when
        // the payload carries its `traceparent`. HyperDX shows the link both
        // ways; the request's latency stays its own.
        let job_span = tracing::info_span!(
            "custom_app_function.job",
            otel.name = %format!("job fn {function_name}"),
            app_id = %app_id,
            function = %function_name,
            mode = %mode,
            faas.trigger = if mode == "schedule" { "timer" } else { "other" },
        );
        if let Some(traceparent) = traceparent.as_deref()
            && !oxy_telemetry::propagation::link_from_traceparent(&job_span, traceparent)
        {
            tracing::debug!(
                traceparent,
                "app_function: unusable traceparent on the task payload"
            );
        }

        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        // The runtime trips `cancel` (returned in ExecutingTask) to stop this
        // task; hand a clone to the run so the isolate actually terminates
        // mid-flight instead of running to its timeout.
        let job = JobArgs {
            db: self.db.clone(),
            preagg: self.preagg.clone(),
            app_id,
            environment,
            function_name,
            mode,
            input,
            cancel: cancel.clone(),
            event_tx,
            outcome_tx,
        };
        // A job has no request hub. Give it one tagged as a custom-app surface;
        // the isolate thread and host calls inherit it (`middlewares::sentry_surface`).
        tokio::spawn(
            run_job(job)
                .instrument(job_span)
                .bind_hub(crate::server::api::middlewares::sentry_surface::custom_app_hub()),
        );

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

/// Everything one job carries onto its spawned task.
struct JobArgs {
    db: DatabaseConnection,
    preagg: crate::server::api::middlewares::workspace_context::PreaggCacheCtx,
    app_id: uuid::Uuid,
    environment: AppEnvironment,
    function_name: String,
    mode: String,
    input: Vec<u8>,
    cancel: CancellationToken,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    outcome_tx: mpsc::Sender<TaskOutcome>,
}

/// The job body: announce the start on the run's event log, run the function
/// under the org owner's identity, report the outcome. Runs under the
/// `custom_app_function.job` span `execute` builds.
async fn run_job(args: JobArgs) {
    let JobArgs {
        db,
        preagg,
        app_id,
        environment,
        function_name,
        mode,
        input,
        cancel,
        event_tx,
        outcome_tx,
    } = args;
    let _ = event_tx
        .send((
            "app_function_started".into(),
            serde_json::json!({ "app_id": app_id, "function_name": function_name }),
        ))
        .await;

    #[cfg(feature = "custom-app-functions")]
    let outcome = match crate::server::api::custom_apps_functions::run_scheduled_function(
        &db,
        app_id,
        &environment,
        &function_name,
        &mode,
        input,
        cancel,
        // Stream the run's log lines onto the run's event log so a
        // scheduled/manual function's output is persisted + observable.
        Some(event_tx.clone()),
        // The worker's own composition root: hand the runtime the shared
        // data-plane query executor as a trait object, so the function
        // module never imports `projects::query`.
        std::sync::Arc::new(crate::server::api::projects::query::DataPlaneQueryExecutor)
            as std::sync::Arc<
                dyn crate::server::api::custom_apps_functions::seam::FunctionQueryExecutor,
            >,
        preagg,
    )
    .await
    {
        Ok(body) => TaskOutcome::Done {
            answer: body,
            metadata: Some(serde_json::json!({ "app_id": app_id, "function_name": function_name })),
        },
        Err(e) => TaskOutcome::Failed(e),
    };
    #[cfg(not(feature = "custom-app-functions"))]
    let outcome = {
        let _ = (
            &db,
            app_id,
            &environment,
            &function_name,
            &mode,
            input,
            cancel,
            &preagg,
        );
        TaskOutcome::Failed("custom-app-functions feature not enabled".to_string())
    };

    let _ = outcome_tx.send(outcome).await;
}

/// The invocation `mode` a queued job reports, from the trigger its seeding path
/// stamped on the task — so the invocation history agrees with the run's
/// `metadata.trigger`.
///
/// **The fallback is the trap.** An unrecognised label does not error, it becomes
/// `schedule`: a trigger added to `FunctionJobTrigger` without a matching arm
/// here would tell a function that a provider's webhook was a cron fire. The
/// fallback exists for legacy tasks queued before the field was carried at all,
/// which really were cron fires — it is not a place to route new labels through.
fn invocation_mode(trigger: Option<&str>) -> String {
    match trigger {
        Some("manual") => "manual",
        // A verified inbound webhook. Distinct from `manual` because a function
        // may legitimately branch on it — the payload is a provider's event, not
        // an operator's click — and distinct from `route` because there is no
        // HTTP caller to attribute it to.
        Some("webhook") => "webhook",
        _ => "schedule",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{APP_FUNCTION_KIND, AppEnvironment, invocation_mode, read_task};
    use crate::server::api::custom_apps_functions::FunctionJobTrigger;
    use agentic_core::delegation::TaskSpec;
    use serde_json::json;

    fn spec(payload: serde_json::Value) -> TaskSpec {
        TaskSpec::Custom {
            kind: APP_FUNCTION_KIND.into(),
            payload,
        }
    }

    const APP: &str = "6f1c1f5e-4b2a-4d8e-9d59-0b8f8a3e2c11";

    /// A task queued before `environment` existed is production, and runs.
    #[test]
    fn a_task_without_an_environment_runs_as_production() {
        let job = read_task(&spec(json!({
            "app_id": APP, "function_name": "refresh", "trigger": "scheduled",
        })))
        .expect("a legacy task is runnable");
        assert_eq!(job.app_id.to_string(), APP);
        assert_eq!(job.mode, "schedule");
        assert_eq!(job.environment, AppEnvironment::Production);
    }

    #[test]
    fn a_task_naming_production_runs() {
        let job = read_task(&spec(json!({
            "app_id": APP, "function_name": "refresh", "environment": "production",
        })))
        .expect("runnable");
        assert_eq!(job.environment, AppEnvironment::Production);
    }

    /// The collision this reader prevents: a worker that ignored the field
    /// would resolve the production build and run a staging task against it.
    /// The environment rides to the runner, which refuses it unless the
    /// function is a check there (`custom_apps::app_environments_phase_1b` and
    /// `custom_app_functions_manual_run::environment_checks` drive both ends).
    #[test]
    fn a_task_for_a_non_production_environment_keeps_its_environment() {
        let job = read_task(&spec(json!({
            "app_id": APP, "function_name": "refresh", "environment": "staging",
        })))
        .expect("a known environment is read");
        assert_eq!(
            job.environment,
            AppEnvironment::Staging,
            "never read as production"
        );
    }

    #[test]
    fn a_task_naming_an_unknown_environment_is_refused() {
        let err = read_task(&spec(json!({
            "app_id": APP, "function_name": "refresh", "environment": "Production",
        })))
        .expect_err("names are exact; an unknown one is not production");
        assert!(err.contains("unknown environment"), "{err}");
    }

    #[test]
    fn the_input_rides_through_as_the_request_body() {
        let job = read_task(&spec(json!({
            "app_id": APP, "function_name": "refresh", "trigger": "manual",
            "input": { "store": 7 },
        })))
        .expect("runnable");
        assert_eq!(job.input, br#"{"store":7}"#);
        assert_eq!(job.mode, "manual");
    }

    /// Every label the seeding side can produce must have an arm here. Written
    /// over the enum rather than string literals so adding a variant without an
    /// arm fails this test instead of silently reporting `schedule`.
    #[test]
    fn every_trigger_label_maps_to_its_own_mode() {
        for trigger in [FunctionJobTrigger::Manual, FunctionJobTrigger::Webhook] {
            let label = trigger.as_str();
            assert_eq!(
                invocation_mode(Some(label)),
                label,
                "trigger '{label}' does not round-trip to its own mode — it is \
                 falling through to the legacy `schedule` default"
            );
        }
    }

    /// A task queued before the field existed is a cron fire, and stays one.
    #[test]
    fn a_payload_without_a_trigger_is_a_schedule() {
        assert_eq!(invocation_mode(None), "schedule");
    }
}
