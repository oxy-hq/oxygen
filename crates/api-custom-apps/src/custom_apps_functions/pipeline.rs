//! The stages every function invocation walks, whichever door it came in by.
//!
//! A route call (`invoke_function`) and a queued run (`run_scheduled_function`
//! — a schedule, a webhook, a Run now, an Airway step) used to re-implement
//! the same preparation side by side: resolve the environment, admit the run,
//! find the function in its build, load the build record and the policy the
//! run executes under, fetch the artifact; and, after the isolate returned,
//! settle the invocation row, page on a failure and record the telemetry.
//! Two copies of one pipeline drift — one grew a reaper, the other an event
//! sink — so the stages live here once, as [`admit`], [`prepare`] and
//! [`record`], and the two orchestrators in `mod.rs` compose them.
//!
//! What differs between the doors is kept at the doors: *when* a run is
//! admitted ([`Door`]), what the route layer does between admission and
//! preparation (idempotency, result cache, rate limit), and how an error is
//! reported — a JSON body with a stable `error` name on the route, a message
//! on the queue. [`PrepareError`] carries the cause and each door renders it
//! in its own contract, unchanged.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use axum::http::StatusCode;
use axum::response::Response;
use entity::prelude::AppBuilds;
use entity::{app_builds, app_function_invocations, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use tracing::error;
use uuid::Uuid;

use super::super::custom_apps_cache::{cached_build, set_cached_build};
use super::super::custom_apps_env_resolve::{
    ResolvedEnvironment, resolve_function_environment, resolve_function_environment_cached,
};
use super::super::custom_apps_telemetry::{self, FunctionEvent, InvocationMeters};
use super::check_run::QueuedError;
use super::env_policy::EnvPolicy;
use super::environment_gate::{self, Admission, EnvironmentRefused, GateError};
use super::queued_admission::{self, QueuedRefused, QueuedTrigger};
use super::{
    FunctionManifestEntry, LogLine, RunOutcome, artifact_cache, check_run, failure_page,
    failure_signal, function_cache, json_error, no_build_response, run_policy,
};

/// Which door a run came in by, which decides when it is admitted.
pub(super) enum Door<'a> {
    /// A route call: admitted for `caller` **before** the function is looked
    /// up, so a caller the environment refuses learns nothing about what the
    /// build holds.
    Route {
        caller: &'a oxy_server_authz::Caller,
        /// The app's builds from the resolution cache, for production's answer.
        cached: &'a super::super::custom_apps_env_resolve::EnvironmentBuilds,
    },
    /// A queued run has no viewer. Production runs as it always has; outside
    /// production what queued it decides (`queued_admission`): a check, Run
    /// now `as=schedule`, a cron fire with the switch on, a webhook — each
    /// read from the build resolved here, not trusted from whoever queued the
    /// task — so the function row comes first.
    Queued { trigger: QueuedTrigger },
}

/// What the door decided before the function lookup: a route caller is
/// admitted (or refused) up front; a queued run waits for the function row.
enum AtTheDoor {
    Admitted(Admission),
    Queued(QueuedTrigger),
}

/// A run the environment let in, with the function it names.
pub(super) struct Admitted {
    pub(super) resolved: ResolvedEnvironment,
    pub(super) admission: Admission,
    pub(super) build_id: Uuid,
    pub(super) manifest: FunctionManifestEntry,
}

/// Everything the runtime needs that the admission did not already carry.
pub(super) struct PreparedRun {
    pub(super) build: app_builds::Model,
    pub(super) policy: EnvPolicy,
    pub(super) artifact_js: String,
}

/// Why a run did not reach the isolate, rendered by each door in its own
/// contract ([`Self::into_response`], [`Self::into_message`]).
pub(super) enum PrepareError {
    EnvironmentLookup(String),
    Refused(EnvironmentRefused),
    /// A queued run its trigger's rule refuses (`queued_admission::decide`).
    QueuedRefused(QueuedRefused),
    NoBuild(AppEnvironment),
    FunctionLookup(String),
    /// The build the environment serves has no function by that name; `build`
    /// is that live build, which the route's stale-caller answer names.
    FunctionNotFound {
        build: Uuid,
    },
    BuildLookup(String),
    BuildNotFound,
    Policy(String),
    BuildStore(String),
    ArtifactMissing(String),
}

impl From<EnvironmentRefused> for PrepareError {
    fn from(refused: EnvironmentRefused) -> Self {
        Self::Refused(refused)
    }
}

impl From<QueuedError> for PrepareError {
    fn from(error: QueuedError) -> Self {
        match error {
            QueuedError::Refused(refused) => Self::QueuedRefused(refused),
            QueuedError::Gate(refused) => Self::Refused(refused),
        }
    }
}

impl PrepareError {
    /// The route's answer: a JSON body whose `error` name is a contract.
    pub(super) fn into_response(self, function_name: &str) -> Response {
        use StatusCode as S;
        match self {
            Self::EnvironmentLookup(e) => {
                error!("app environment lookup failed: {e}");
                json_error(
                    S::INTERNAL_SERVER_ERROR,
                    "EnvironmentLookupFailed",
                    "could not read the app's environments; retry shortly",
                )
            }
            Self::Refused(refused) => refused.into_response(),
            // Only the queued door produces this; a route never sees it.
            Self::QueuedRefused(refused) => {
                json_error(S::FORBIDDEN, "QueuedRunRefused", &refused.message())
            }
            Self::NoBuild(environment) => no_build_response(&environment),
            Self::FunctionLookup(e) => {
                error!("app_functions lookup failed: {e}");
                json_error(
                    S::INTERNAL_SERVER_ERROR,
                    "FunctionLookupFailed",
                    "could not read the function registry; retry shortly",
                )
            }
            Self::FunctionNotFound { .. } => json_error(
                S::NOT_FOUND,
                "FunctionNotFound",
                &format!(
                    "no function named '{function_name}' in the live build; check the \
                     `functions` entry in oxy-app.json and re-publish"
                ),
            ),
            Self::BuildLookup(e) => {
                error!("app_builds lookup failed: {e}");
                json_error(
                    S::INTERNAL_SERVER_ERROR,
                    "BuildLookupFailed",
                    "could not read the app build record; retry shortly",
                )
            }
            Self::BuildNotFound => json_error(
                S::NOT_FOUND,
                "BuildNotFound",
                "the build this app points at no longer exists; re-publish the app",
            ),
            Self::Policy(e) => {
                error!("environment policy lookup failed: {e}");
                json_error(
                    S::INTERNAL_SERVER_ERROR,
                    "EnvironmentLookupFailed",
                    "could not read the environment's policy (its planes or the org's OLTP \
                     staging branch); retry shortly",
                )
            }
            Self::BuildStore(e) => {
                error!("build store fetch failed: {e}");
                json_error(
                    S::INTERNAL_SERVER_ERROR,
                    "BuildStoreUnavailable",
                    "could not fetch the function artifact from the build store; retry shortly",
                )
            }
            Self::ArtifactMissing(artifact_rel) => json_error(
                S::NOT_FOUND,
                "FunctionArtifactMissing",
                &format!(
                    "'{artifact_rel}' is absent from the build store for this build; \
                     re-publish the app"
                ),
            ),
        }
    }

    /// The queue's answer: the run's error message.
    pub(super) fn into_message(self, function_name: &str) -> String {
        match self {
            Self::EnvironmentLookup(e) => format!("environment lookup failed: {e}"),
            Self::Refused(refused) => refused.message(),
            Self::QueuedRefused(refused) => refused.message(),
            Self::NoBuild(AppEnvironment::Production) => "app has no build".to_string(),
            Self::NoBuild(environment) => {
                format!("the {environment} environment of this app has no build")
            }
            Self::FunctionLookup(e) => format!("app_functions lookup failed: {e}"),
            Self::FunctionNotFound { .. } => {
                format!("function '{function_name}' not found in build")
            }
            Self::BuildLookup(e) => format!("app_builds lookup failed: {e}"),
            Self::BuildNotFound => "build not found".to_string(),
            Self::Policy(e) => format!("environment policy lookup failed: {e}"),
            Self::BuildStore(e) => format!("build store fetch failed: {e}"),
            Self::ArtifactMissing(artifact_rel) => format!("artifact '{artifact_rel}' not found"),
        }
    }
}

/// Resolve the environment, admit the run (see [`Door`]) and find the
/// function in the build the environment serves.
pub(super) async fn admit(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    function_name: &str,
    door: Door<'_>,
) -> Result<Admitted, PrepareError> {
    let resolved = match &door {
        Door::Route { cached, .. } => {
            resolve_function_environment_cached(db, app, cached, environment).await
        }
        Door::Queued { .. } => resolve_function_environment(db, app, environment).await,
    }
    .map_err(|e| PrepareError::EnvironmentLookup(e.to_string()))?;
    // The one environment decision (see `environment_gate`): production for
    // everyone; staging for staff, with every write held by the policy the
    // host is built from; dev slots refused.
    let at_the_door = match door {
        Door::Route { caller, .. } => {
            let entrance = environment_gate::route_entrance(db, &resolved, caller, app).await;
            AtTheDoor::Admitted(environment_gate::admit(&resolved, entrance)?)
        }
        Door::Queued { trigger } => AtTheDoor::Queued(trigger),
    };
    let build_id = resolved
        .build_id
        .ok_or_else(|| PrepareError::NoBuild(resolved.environment.clone()))?;
    let func_row = function_cache::function_row(db, app.id, build_id, function_name)
        .await
        .map_err(|e| PrepareError::FunctionLookup(e.to_string()))?
        .ok_or(PrepareError::FunctionNotFound { build: build_id })?;
    let admission = match at_the_door {
        AtTheDoor::Admitted(admission) => admission,
        // Refused before the invocation row, so a refusal leaves none. What
        // the function declares and the environment's switch are read here,
        // from the resolved build, not trusted from whoever queued the task.
        AtTheDoor::Queued(trigger) => {
            let declared = queued_admission::Declared::of(func_row.manifest_json.as_ref());
            let state = queued_admission::environment_state(db, app, &resolved, trigger)
                .await
                .map_err(|e| PrepareError::EnvironmentLookup(e.to_string()))?;
            check_run::admit_queued(&resolved, trigger, declared, state)?
        }
    };
    let manifest: FunctionManifestEntry = func_row
        .manifest_json
        .as_ref()
        .and_then(|j| serde_json::from_value(j.clone()).ok())
        .unwrap_or_default();
    Ok(Admitted {
        resolved,
        admission,
        build_id,
        manifest,
    })
}

/// Load the build record, decide the policy the run executes under and fetch
/// its artifact — before any invocation row is written, so a failed lookup
/// leaves none.
pub(super) async fn prepare(
    db: &DatabaseConnection,
    app: &apps::Model,
    admission: Admission,
    build_id: Uuid,
    function_name: &str,
) -> Result<PreparedRun, PrepareError> {
    // The build's string `build_id` (build store key) from its uuid.
    let build = load_build(db, build_id).await?;
    // What the run may do in its environment (`run_policy`), decided once for
    // the whole invocation: outside production, where `ctx.oltp` lands, the
    // semantic model the build pins, the `env` keys it shares with production
    // and where its warehouse writes are mapped. Production asks nothing.
    let policy = run_policy::build(db, admission, app, &build)
        .await
        .map_err(|e| match e {
            GateError::Refused(refused) => PrepareError::Refused(refused),
            GateError::Db(e) => PrepareError::Policy(e.to_string()),
        })?;
    let artifact_rel = format!("functions/{function_name}.js");
    let artifact = artifact_cache::get(app.id, &build.build_id, &artifact_rel)
        .await
        .map_err(|e| PrepareError::BuildStore(e.to_string()))?
        .ok_or(PrepareError::ArtifactMissing(artifact_rel))?;
    Ok(PreparedRun {
        build,
        policy,
        artifact_js: String::from_utf8_lossy(&artifact).into_owned(),
    })
}

/// The build record, from the replica's build cache when it holds it. A build
/// row is written once and never rewritten (a promote repoints the
/// environment at another build id), so a hit cannot be stale; a miss is read
/// and remembered, and a build that is not there is not.
async fn load_build(
    db: &DatabaseConnection,
    build_id: Uuid,
) -> Result<app_builds::Model, PrepareError> {
    if let Some(build) = cached_build(build_id) {
        return Ok(build);
    }
    let build = AppBuilds::find_by_id(build_id)
        .one(db)
        .await
        .map_err(|e| PrepareError::BuildLookup(e.to_string()))?
        .ok_or(PrepareError::BuildNotFound)?;
    set_cached_build(build_id, build.clone());
    Ok(build)
}

/// Started the moment a run is decided, before the isolate thread spawns, so
/// `init_ms` covers everything between deciding to run and tenant code
/// running, thread spawn included.
pub(super) struct RunClock {
    pub(super) started: Instant,
    /// Shared log buffer: the isolate appends `console.*`/`ctx.log` during the
    /// run; [`record`] drains it after, in one batch.
    pub(super) logs: Arc<Mutex<Vec<LogLine>>>,
    /// Subrequest count and setup time, read back after the run. Shared rather
    /// than returned because the isolate thread can outlive it.
    pub(super) meters: InvocationMeters,
}

impl RunClock {
    pub(super) fn start() -> Self {
        Self {
            started: Instant::now(),
            logs: Arc::new(Mutex::new(Vec::new())),
            meters: InvocationMeters::start(),
        }
    }
}

/// A run the isolate has returned from, with what the row and the telemetry
/// need to say about it.
pub(super) struct Finished<'a> {
    pub(super) app: &'a apps::Model,
    pub(super) environment: &'a AppEnvironment,
    pub(super) function_name: &'a str,
    pub(super) mode: &'a str,
    pub(super) build_id: Uuid,
    pub(super) invocation_id: Uuid,
    /// Whom the telemetry attributes the run to: the caller on a route, the
    /// org owner on a queued run.
    pub(super) user_id: Uuid,
    pub(super) request_id: Option<Uuid>,
    pub(super) clock: RunClock,
    /// Keep a successful result on the row: a queued run always (the run
    /// detail reads it back), a route call only under an idempotency key, so
    /// the audit table is not bloated with every function's output.
    pub(super) persist_result: bool,
}

/// What [`record`] settled, for the door to answer with.
pub(super) struct Recorded {
    pub(super) status: &'static str,
    pub(super) error: Option<String>,
    pub(super) body: String,
    pub(super) http_status: u16,
    pub(super) duration_ms: i64,
    /// The isolate's `console.*` / `ctx.log` lines, drained in one batch
    /// after the run (not live-tailed), already persisted to the log sink.
    pub(super) logs: Vec<LogLine>,
}

/// Settle the invocation row, page on a failure, and record the run and its
/// log lines.
pub(super) async fn record(
    db: &DatabaseConnection,
    finished: Finished<'_>,
    outcome: RunOutcome,
) -> Recorded {
    let (status, error_msg, body, http_status, host_call, typed_kind) = outcome;
    let duration_ms = finished.clock.started.elapsed().as_millis() as i64;
    let failure = failure_signal::Failure::of(
        status,
        http_status,
        error_msg.as_deref(),
        typed_kind.map(failure_signal::label_of_kind),
        host_call.as_ref(),
    );
    let settled = Settled {
        status,
        duration_ms,
        error: error_msg.as_deref(),
        body: &body,
        http_status,
        fingerprint: failure.as_ref().map(|f| f.fingerprint.clone()),
        error_kind: failure.as_ref().map(|f| f.kind),
    };
    if settle_row(db, &finished, &settled).await
        && let Some(failure) = &failure
    {
        failure_page::observe(
            db,
            finished.app,
            finished.environment,
            finished.function_name,
            finished.invocation_id,
            failure,
        )
        .await;
    }
    // The waterfall's trace: spawned, so the answer never waits on it.
    crate::custom_apps_call_trace_store::spawn_settled(
        db,
        finished.app.id,
        finished.invocation_id,
        status,
        &finished.clock.meters,
    );
    custom_apps_telemetry::record_function(FunctionEvent {
        org_id: finished.app.org_id,
        app_id: finished.app.id,
        build_id: finished.build_id,
        request_id: finished.request_id,
        user_id: finished.user_id,
        function_name: finished.function_name,
        mode: finished.mode,
        status_label: status,
        http_status,
        duration_ms: duration_ms.clamp(0, u32::MAX as i64) as u32,
        host_calls: finished.clock.meters.host_calls(),
        init_ms: finished.clock.meters.init_ms(),
        error: error_msg.as_deref(),
        environment: &finished.environment.name(),
    });
    let logs = persist_logs(&finished);
    Recorded {
        status,
        error: error_msg,
        body,
        http_status,
        duration_ms,
        logs,
    }
}

/// What the run left on its row.
struct Settled<'a> {
    status: &'static str,
    duration_ms: i64,
    error: Option<&'a str>,
    body: &'a str,
    http_status: u16,
    fingerprint: Option<String>,
    /// [`failure_signal::Failure::kind`], stored beside the fingerprint.
    error_kind: Option<&'static str>,
}

/// Write the outcome to the invocation row; whether the write landed (a page
/// is filed against a row that says what happened, never against a lost one).
async fn settle_row(
    db: &DatabaseConnection,
    finished: &Finished<'_>,
    settled: &Settled<'_>,
) -> bool {
    let mut update = app_function_invocations::ActiveModel {
        id: Set(finished.invocation_id),
        ..Default::default()
    };
    update.status = Set(settled.status.to_string());
    update.duration_ms = Set(Some(settled.duration_ms));
    update.error = Set(settled.error.map(str::to_string));
    update.failure_fingerprint = Set(settled.fingerprint.clone());
    update.error_kind = Set(settled.error_kind.map(str::to_string));
    if finished.persist_result && settled.status == "success" {
        update.result_body = Set(Some(settled.body.to_string()));
        update.result_status = Set(Some(settled.http_status as i16));
    }
    match update.update(db).await {
        Ok(_) => true,
        Err(e) => {
            error!("failed to update app_function_invocations row: {e}");
            false
        }
    }
}

/// Drain the isolate's log buffer — in one batch, after the run, not
/// live-tailed — and persist it. Until this was shared a route call's logs
/// lived ONLY in its SSE frames, so a browser that had navigated away took
/// them with it, while a scheduled run's survived as `agentic_run_events`.
fn persist_logs(finished: &Finished<'_>) -> Vec<LogLine> {
    // Taken out under the lock, which is released before anything awaits
    // (never hold a std Mutex across `.await`).
    let logs: Vec<LogLine> = {
        let mut guard = finished
            .clock
            .logs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *guard)
    };
    custom_apps_telemetry::record_function_logs(
        finished.app.org_id,
        finished.app.id,
        finished.build_id,
        finished.invocation_id,
        finished.request_id,
        finished.function_name,
        finished.mode,
        &logs
            .iter()
            .map(|l| (l.level.clone(), l.message.clone()))
            .collect::<Vec<_>>(),
        &finished.environment.name(),
    );
    logs
}
