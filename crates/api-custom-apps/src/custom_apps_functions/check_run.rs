//! A queued function run in a named app environment: how one is triggered,
//! and the rule that admits it there.
//!
//! **Outside production a queued run is admitted by what queued it**
//! ([`queued_admission`](super::queued_admission)): a manual run is a check —
//! a function its own build marks `"check": true` in `oxy-app.json`, the same
//! flag that lets a publish token run it in production
//! (`admin::apps::handlers::machine_may_run`); Run now `as=schedule` is a
//! function that declares a `schedule`; a cron fire also needs the
//! environment's schedules switch on; a webhook needs its `webhook` block.
//!
//! The rule is applied twice, on purpose. [`trigger`] refuses to *queue* a run
//! the build does not declare, so the caller hears `not_a_check` or
//! `not_scheduled` at once. [`admit_queued`] refuses to *run* one, reading the
//! declarations again from the build the worker actually resolved — the task
//! sat in a queue, and the environment's pointer may have moved to a build
//! where the function no longer declares what it was queued as.
//!
//! Always compiled: the trigger only writes a run and a task, so it works on a
//! node built without the isolate (`custom-app-functions`).

use entity::prelude::AppFunctions;
use entity::{app_functions, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::environment_gate::{self, Admission, Entrance, EnvironmentRefused};
use super::queued_admission::{
    self, Declared, EnvironmentState, QueuedRefusal, QueuedRefused, QueuedTrigger,
};
use super::{FunctionJobTrigger, function_task_policy};
use crate::custom_apps_env_resolve::{ResolvedEnvironment, resolve_function_environment};

/// Why a run was not queued.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TriggerError {
    #[error("app not found")]
    AppNotFound,
    #[error("the {0} environment of this app has no build")]
    NoBuild(AppEnvironment),
    #[error("function '{0}' not found in the build the environment serves")]
    FunctionNotFound(String),
    #[error(
        "function '{function}' is not marked `\"check\": true` in the build the {environment} \
         environment serves; outside production only a check runs as a job"
    )]
    NotACheck {
        function: String,
        environment: AppEnvironment,
    },
    #[error(
        "function '{function}' declares no `schedule` in the build the {environment} environment \
         serves, so it cannot run as a schedule"
    )]
    NotScheduled {
        function: String,
        environment: AppEnvironment,
    },
    #[error(
        "function '{function}' declares no `webhook` block in the build the {environment} \
         environment serves"
    )]
    NoWebhook {
        function: String,
        environment: AppEnvironment,
    },
    #[error("failed to enqueue function job: {0}")]
    Enqueue(String),
    #[error("{0}")]
    Db(String),
}

/// Whether a function's manifest entry marks it as a check. Absent,
/// non-boolean and `false` all mean no: the grant is to run what the app
/// DECLARED as a check, so anything the manifest does not say yes to is a no.
/// A manifest that is missing entirely is the same answer for the same reason.
///
/// The one reader of the flag for a decision. `FunctionManifestEntry` does not
/// carry it, so both the publish-token rule and the environment rule read the
/// row's raw `manifest_json` through here.
pub(crate) fn manifest_marks_check(manifest: Option<&serde_json::Value>) -> bool {
    manifest
        .and_then(|m| m.get("check"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Why the runner refused a queued run: the trigger's rule
/// ([`queued_admission::decide`]), or the environment gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QueuedError {
    Refused(QueuedRefused),
    Gate(EnvironmentRefused),
}

impl QueuedError {
    /// The run's failure message.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Refused(refused) => refused.message(),
            Self::Gate(refused) => refused.message(),
        }
    }
}

/// A queued run's admission, by what queued it (`trigger`), what the function
/// declares in the build resolved here (`declared`), and the environment's
/// own state where the trigger's rule reads it
/// (`queued_admission::environment_state`).
///
/// A run admitted outside production takes the route entrance with reach
/// granted: a queued run has no viewer, and who may ask was decided when it
/// was queued (`admin::apps::environment_scope::resolve`, a webhook's
/// signature, a schedule row staff turned on). The gate still decides which
/// environments run functions at all, so an environment it refuses on a
/// route call is refused here too. Production takes `Entrance::Queued`, as it
/// always has.
///
/// Called by the runner, which only a node built with the isolate has.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
pub(crate) fn admit_queued(
    resolved: &ResolvedEnvironment,
    trigger: QueuedTrigger,
    declared: Declared,
    state: Option<EnvironmentState>,
) -> Result<Admission, QueuedError> {
    queued_admission::decide(trigger, declared, resolved.is_production(), state).map_err(
        |why| {
            QueuedError::Refused(QueuedRefused {
                environment: resolved.environment.clone(),
                why,
            })
        },
    )?;
    let entrance = if resolved.is_production() {
        Entrance::Queued
    } else {
        Entrance::Route {
            non_production_reach: true,
        }
    };
    environment_gate::admit(resolved, entrance).map_err(QueuedError::Gate)
}

/// The function `name` in the build `environment` of `app` serves, as a job
/// queued by `trigger` may run it there: outside production a manual run is
/// a check and a webhook needs its block, and `as=schedule` needs a declared
/// schedule everywhere ([`queued_admission::decide`]).
async fn runnable_function(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    name: &str,
    trigger: FunctionJobTrigger,
) -> Result<app_functions::Model, TriggerError> {
    let build_id = resolve_function_environment(db, app, environment)
        .await
        .map_err(|e| TriggerError::Db(format!("environment lookup failed: {e}")))?
        .build_id
        .ok_or_else(|| TriggerError::NoBuild(environment.clone()))?;
    // Validate the function exists in the build before enqueuing, so a typo is
    // an answer now rather than a failed run later.
    let function = AppFunctions::find()
        .filter(app_functions::Column::BuildId.eq(build_id))
        .filter(app_functions::Column::Name.eq(name))
        .one(db)
        .await
        .map_err(|e| TriggerError::Db(format!("app_functions lookup failed: {e}")))?
        .ok_or_else(|| TriggerError::FunctionNotFound(name.to_string()))?;
    let declared = Declared::of(function.manifest_json.as_ref());
    let production = *environment == AppEnvironment::Production;
    // The environment's switch and expiry are not asked here: no trigger a
    // request can queue reads them before the run (a webhook's route already
    // did), and the runner reads them again regardless.
    queued_admission::decide(trigger.queued(), declared, production, None)
        .map_err(|why| refused(why, name, environment))?;
    Ok(function)
}

/// The [`TriggerError`] a refusal of [`runnable_function`] answers.
fn refused(why: QueuedRefusal, function: &str, environment: &AppEnvironment) -> TriggerError {
    let (function, environment) = (function.to_string(), environment.clone());
    match why {
        QueuedRefusal::NotScheduled => TriggerError::NotScheduled {
            function,
            environment,
        },
        QueuedRefusal::NoWebhook => TriggerError::NoWebhook {
            function,
            environment,
        },
        // A check is the rule for a manual run; the others need the
        // environment's state (or an event, which no request queues), which a
        // request-time decision never reads.
        QueuedRefusal::NotACheck
        | QueuedRefusal::SchedulesOff
        | QueuedRefusal::EventsOff
        | QueuedRefusal::NoOn
        | QueuedRefusal::Expired => TriggerError::NotACheck {
            function,
            environment,
        },
    }
}

/// How a queued run is recorded by the worker that runs it: its invocation
/// `mode`, and the sandbox agent token that queued it, when one did — which
/// stamps the invocation row and the run's held-write row.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct QueuedBy<'a> {
    pub mode: &'a str,
    /// What queued the run: the rule [`admit_queued`] applies to it.
    pub trigger: QueuedTrigger,
    pub credential_token_id: Option<Uuid>,
}

/// What queued a run, where it runs, and the token that asked for it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Asked<'a> {
    pub trigger: FunctionJobTrigger,
    pub environment: &'a AppEnvironment,
    /// The sandbox agent token that asked, when one did. It rides the task,
    /// and the worker admits it again before the run starts
    /// (`app_function_agent::recheck`): a run queued by a token that has
    /// since been revoked, or whose minter lost reach, is cancelled.
    pub credential_token_id: Option<Uuid>,
}

/// Queue a one-off run of `function_name` of app `app_id`, as `asked`.
/// See `custom_apps_functions::trigger_function_job_in`, the entry point.
///
/// Production queues the task it always queued, naming no environment. Any
/// other environment's task names it, so the worker resolves that
/// environment's build and runs under its policy — and a worker that predates
/// the field's writers refuses the task rather than run it on production.
pub(crate) async fn trigger(
    db: &DatabaseConnection,
    app_id: Uuid,
    function_name: &str,
    input: Option<serde_json::Value>,
    asked: Asked<'_>,
) -> Result<String, TriggerError> {
    let environment = asked.environment;
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|e| TriggerError::Db(format!("app lookup failed: {e}")))?
        .ok_or(TriggerError::AppNotFound)?;
    let function = runnable_function(db, &app, environment, function_name, asked.trigger).await?;
    let policy = function
        .manifest_json
        .as_ref()
        .and_then(function_task_policy);
    let mut task = agentic_pipeline::app_function_task::AppFunctionTask::new(
        app_id.to_string(),
        function_name,
    );
    task.trigger = Some(asked.trigger.as_str().to_string());
    task.input = input.filter(|v| !v.is_null());
    task.traceparent = oxy_telemetry::propagation::current_traceparent();
    task.environment = (*environment != AppEnvironment::Production).then(|| environment.name());
    task.credential_token_id = asked.credential_token_id.map(|id| id.to_string());
    agentic_pipeline::scheduler::enqueue_app_function_task(db, app.project_id, policy, task)
        .await
        .map_err(|e| TriggerError::Enqueue(format!("{e:?}")))
}

#[cfg(test)]
#[path = "check_run_tests.rs"]
mod tests;
