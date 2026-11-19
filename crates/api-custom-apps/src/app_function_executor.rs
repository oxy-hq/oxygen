//! `AppFunctionTaskExecutor`: runs a **scheduled** custom-app Oxy Function as a
//! `TaskSpec::Custom { kind: "app_function" }` on the global-run fleet. Registered
//! into the `CustomTaskRegistry` by `server::router::recovery`;
//! `PipelineTaskExecutor` delegates the `app_function` kind here. The actual run
//! is `custom_apps_functions::run_scheduled_function` (org-owner identity,
//! `mode="schedule"` invocation record). See
//! `internal-docs/customer-apps-functions.md`.
//!
//! An event's delivery (trigger `event`) runs through [`delivery`], which
//! records each attempt on the delivery it answers for.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_pipeline::app_function_task::AppFunctionTask;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::DatabaseConnection;

use crate::custom_apps_functions::queued_admission::QueuedTrigger;
use sentry::SentryFutureExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

/// The `kind` discriminator for a scheduled custom-app function Custom task.
pub use agentic_pipeline::app_function_task::APP_FUNCTION_KIND;

mod delivery;
#[cfg(feature = "custom-app-functions")]
mod sender;

/// A queued task, read and checked: everything the job needs, typed.
#[derive(Debug)]
struct JobRequest {
    app_id: uuid::Uuid,
    function_name: String,
    mode: String,
    /// What queued the run: the rule that admits it outside production.
    trigger: QueuedTrigger,
    input: Vec<u8>,
    traceparent: Option<String>,
    environment: AppEnvironment,
    /// The sandbox agent token that queued the run, admitted again before it
    /// starts (`app_function_agent`).
    credential_token_id: Option<uuid::Uuid>,
    /// The `app_event_deliveries` row an event run answers for. Always set
    /// for the `event` trigger, never read for another.
    event_delivery_id: Option<uuid::Uuid>,
}

/// Read an `app_function` task.
///
/// **The environment is carried, never dropped.** A worker that ignored it
/// would run a staging task on production's build and secrets
/// (`internal-docs/2026-09-10-custom-app-environments-design.md` §3.4). An
/// unknown name is refused here; a known one rides to
/// `run_scheduled_function`, which resolves that environment's build and
/// admits the run through `check_run::admit_queued` by what queued it — a
/// check, Run now `as=schedule`, a cron fire the environment turned on, a
/// webhook its build declares — under that environment's policy; everything
/// else is refused there, in one place.
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
    let credential_token_id =
        super::app_function_agent::token_of(task.credential_token_id.as_deref())?;
    let trigger = QueuedTrigger::from_label(task.trigger.as_deref());
    // A label this release does not know was queued by a newer one: fail the
    // attempt, which the queue retries (likely on a newer pod), rather than
    // run the function as something it was not queued as.
    if trigger == QueuedTrigger::Unknown {
        return Err(format!(
            "this worker does not know the trigger `{}`; it was queued by a newer release",
            task.trigger.as_deref().unwrap_or_default()
        ));
    }
    // An event run answers for its delivery: without one there is nothing to
    // record the attempt on, and no event to hand the listener.
    // A directed event (`ctx.apps.send`) is a call that rides an event's
    // delivery: it carries the delivery's id like any event.
    let event_delivery_id = match (trigger, task.event_delivery_id.as_deref()) {
        (QueuedTrigger::Event, None) => {
            return Err("an event task names no delivery (`event_delivery_id`)".to_string());
        }
        (QueuedTrigger::Event | QueuedTrigger::Call, Some(id)) => Some(
            id.parse()
                .map_err(|e| format!("bad event_delivery_id: {e}"))?,
        ),
        _ => None,
    };
    Ok(JobRequest {
        app_id,
        mode: invocation_mode(task.trigger.as_deref()),
        trigger,
        function_name: task.function_name,
        input,
        traceparent: task.traceparent,
        environment,
        credential_token_id,
        event_delivery_id,
    })
}

pub struct AppFunctionTaskExecutor {
    pub db: DatabaseConnection,
    /// The node's Layer-1 preagg cache, injected by `build_custom_task_registry`
    /// so a scheduled function's `ctx.semantic` resolves rollups the same way an
    /// HTTP-invoked one does. Default (no cache) compiles to warehouse SQL.
    pub preagg: oxy_app_core::preagg_cache_ctx::PreaggCacheCtx,
    /// The worker's own composition root hands the runtime the shared data-plane
    /// query executor as a trait object (`DataPlaneQueryExecutor`, built by
    /// `oxy-server`), so the function module never imports `data_plane::query`.
    pub executor: std::sync::Arc<dyn oxy_app_core::function_seam::FunctionQueryExecutor>,
}

#[async_trait]
impl TaskExecutor for AppFunctionTaskExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let request = read_task(&assignment.spec)?;
        let job_span = job_span(&request);

        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        // The runtime trips `cancel` (returned in ExecutingTask) to stop this
        // task; hand a clone to the run so the isolate actually terminates
        // mid-flight instead of running to its timeout.
        let job = self.job(request, cancel.clone(), event_tx, outcome_tx);
        // A job has no request hub. Give it one tagged as a custom-app surface;
        // the isolate thread and host calls inherit it (`middlewares::sentry_surface`).
        tokio::spawn(
            run_job(job)
                .instrument(job_span)
                .bind_hub(oxy_telemetry::sentry_filter::custom_app_hub()),
        );

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

impl AppFunctionTaskExecutor {
    /// The job `request` asks for, carried onto its spawned task with this
    /// executor's handles, the task's cancel token and its channels.
    fn job(
        &self,
        request: JobRequest,
        cancel: CancellationToken,
        event_tx: mpsc::Sender<(String, serde_json::Value)>,
        outcome_tx: mpsc::Sender<TaskOutcome>,
    ) -> JobArgs {
        let JobRequest {
            app_id,
            function_name,
            mode,
            trigger,
            input,
            traceparent: _,
            environment,
            credential_token_id,
            event_delivery_id,
        } = request;
        JobArgs {
            db: self.db.clone(),
            preagg: self.preagg.clone(),
            executor: self.executor.clone(),
            app_id,
            environment,
            credential_token_id,
            function_name,
            mode,
            trigger,
            input,
            event_delivery_id,
            cancel,
            event_tx,
            outcome_tx,
        }
    }
}

/// The worker's run is a root trace of its own; this span is that root,
/// linked — not parented — to the request that enqueued the task when the
/// payload carries its `traceparent`. HyperDX shows the link both ways; the
/// request's latency stays its own.
fn job_span(request: &JobRequest) -> tracing::Span {
    let JobRequest {
        app_id,
        function_name,
        mode,
        traceparent,
        ..
    } = request;
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
    job_span
}

/// Everything one job carries onto its spawned task.
struct JobArgs {
    db: DatabaseConnection,
    preagg: oxy_app_core::preagg_cache_ctx::PreaggCacheCtx,
    executor: std::sync::Arc<dyn oxy_app_core::function_seam::FunctionQueryExecutor>,
    app_id: uuid::Uuid,
    environment: AppEnvironment,
    credential_token_id: Option<uuid::Uuid>,
    function_name: String,
    mode: String,
    trigger: QueuedTrigger,
    input: Vec<u8>,
    event_delivery_id: Option<uuid::Uuid>,
    cancel: CancellationToken,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    outcome_tx: mpsc::Sender<TaskOutcome>,
}

/// The job body: announce the start on the run's event log, run the function
/// under the org owner's identity, report the outcome. Runs under the
/// `custom_app_function.job` span `execute` builds.
async fn run_job(mut args: JobArgs) {
    // A run a sandbox agent token queued starts only if that token would
    // still be admitted now. Otherwise it is cancelled, before the started
    // event and before any of the function runs.
    if let Some(token_id) = args.credential_token_id
        && let Err(why) =
            super::app_function_agent::recheck(&args.db, token_id, args.app_id, &args.environment)
                .await
    {
        let _ = args
            .event_tx
            .send((
                "app_function_cancelled".into(),
                serde_json::json!({ "app_id": args.app_id, "function_name": args.function_name, "reason": why }),
            ))
            .await;
        let _ = args.outcome_tx.send(TaskOutcome::Cancelled).await;
        return;
    }
    let _ = args
        .event_tx
        .send((
            "app_function_started".into(),
            serde_json::json!({ "app_id": args.app_id, "function_name": args.function_name }),
        ))
        .await;

    let outcome = match args.event_delivery_id {
        Some(delivery_id) => delivery::deliver(&args, delivery_id).await,
        None => {
            let input = std::mem::take(&mut args.input);
            match run_function(&args, input, None).await {
                Ok(run) if run.status == "success" => TaskOutcome::Done {
                    answer: run.body,
                    metadata: Some(serde_json::json!({
                        "app_id": args.app_id,
                        "function_name": args.function_name,
                    })),
                },
                Ok(run) => {
                    TaskOutcome::Failed(run.error.unwrap_or_else(|| "function failed".to_string()))
                }
                Err(refused) => TaskOutcome::Failed(refused),
            }
        }
    };
    let _ = args.outcome_tx.send(outcome).await;
}

/// How a run of the function ended, the fields of it this module reads.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
struct FunctionRun {
    invocation_id: uuid::Uuid,
    status: &'static str,
    http_status: u16,
    body: String,
    error: Option<String>,
    memory_limit: bool,
}

/// Run the function once under the org owner's identity; `event` is set for
/// an event's delivery. `Err` is a run refused before it started.
#[cfg(feature = "custom-app-functions")]
async fn run_function(
    job: &JobArgs,
    input: Vec<u8>,
    event: Option<crate::custom_apps_functions::EventRun>,
) -> Result<FunctionRun, String> {
    let report = crate::custom_apps_functions::run_scheduled_function(
        &job.db,
        job.app_id,
        &job.environment,
        &job.function_name,
        crate::custom_apps_functions::QueuedBy {
            mode: &job.mode,
            trigger: job.trigger,
            credential_token_id: job.credential_token_id,
        },
        event,
        input,
        job.cancel.clone(),
        // Stream the run's log lines onto the run's event log so a
        // scheduled/manual function's output is persisted + observable.
        Some(job.event_tx.clone()),
        job.executor.clone(),
        job.preagg.clone(),
    )
    .await?;
    Ok(FunctionRun {
        invocation_id: report.invocation_id,
        status: report.status,
        http_status: report.http_status,
        body: report.body,
        error: report.error,
        memory_limit: report.memory_limit,
    })
}

#[cfg(not(feature = "custom-app-functions"))]
async fn run_function(
    job: &JobArgs,
    input: Vec<u8>,
    event: Option<()>,
) -> Result<FunctionRun, String> {
    let _ = (
        (
            &job.preagg,
            &job.executor,
            &job.cancel,
            job.trigger,
            &job.mode,
        ),
        input,
        event,
    );
    Err("custom-app-functions feature not enabled".to_string())
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
        // Run now `as=schedule`: a person asked, so it is a manual run — and
        // counts as a sandbox's activity, which a cron fire does not.
        Some("schedule_now") => "manual",
        // A verified inbound webhook. Distinct from `manual` because a function
        // may legitimately branch on it — the payload is a provider's event, not
        // an operator's click — and distinct from `route` because there is no
        // HTTP caller to attribute it to.
        Some("webhook") => "webhook",
        // A published event delivered to a listening function. Its own mode
        // so a function can branch on it: the payload is another app's event,
        // not a cron tick or a provider's webhook.
        Some("event") => "event",
        // Another app's `ctx.apps.call`, or a `ctx.apps.send` delivered: the
        // function is called, not listening or scheduled.
        Some("call") => "call",
        _ => "schedule",
    }
    .to_string()
}

#[cfg(test)]
mod tests;
