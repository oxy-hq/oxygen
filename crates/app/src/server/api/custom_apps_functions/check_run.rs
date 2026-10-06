//! A queued function run in a named app environment: how one is triggered,
//! and the one rule that admits it outside production.
//!
//! **Outside production a queued run is a check run, and nothing else.** A
//! check is a function its own build marks `"check": true` in `oxy-app.json`
//! — the same flag that lets a publish token run it in production
//! (`admin::apps::handlers::machine_may_run`). Run now, schedules and webhooks
//! stay production-only: their tasks name no environment, and a task that
//! names one for a function that is not a check is refused by the runner
//! ([`admit_queued`]), whoever queued it.
//!
//! The rule is applied twice, on purpose. [`trigger`] refuses to *queue* a
//! non-check run outside production, so the caller hears `not_a_check` at
//! once. [`admit_queued`] refuses to *run* one, reading the flag again from
//! the build the worker actually resolved — the task sat in a queue, and the
//! environment's pointer may have moved to a build where the function is no
//! longer a check.
//!
//! Always compiled: the trigger only writes a run and a task, so it works on a
//! node built without the isolate (`custom-app-functions`).

use entity::prelude::AppFunctions;
use entity::{app_functions, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::environment_gate::{self, Admission, Entrance, EnvironmentRefused};
use super::{FunctionJobTrigger, function_task_policy};
use crate::server::api::custom_apps_env_resolve::{
    ResolvedEnvironment, resolve_function_environment,
};

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

/// A queued run's admission. Outside production only a run of a `check: true`
/// function is admitted; everything else takes `Entrance::Queued`, as today.
///
/// A check takes the route entrance with reach granted: a queued run has no
/// viewer, and the reach was decided when the run was requested
/// (`admin::apps::environment_scope::resolve`) — the task carries that
/// decision. The gate still decides which environments run functions at all,
/// so an environment it refuses on a route call is refused here too.
///
/// Called by the runner, which only a node built with the isolate has.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
pub(crate) fn admit_queued(
    resolved: &ResolvedEnvironment,
    is_check: bool,
) -> Result<Admission, EnvironmentRefused> {
    let entrance = if is_check && !resolved.is_production() {
        Entrance::Route {
            non_production_reach: true,
        }
    } else {
        Entrance::Queued
    };
    environment_gate::admit(resolved, entrance)
}

/// The function `name` in the build `environment` of `app` serves, as a job
/// may run it there: outside production, a check.
async fn runnable_function(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    name: &str,
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
    if *environment != AppEnvironment::Production
        && !manifest_marks_check(function.manifest_json.as_ref())
    {
        return Err(TriggerError::NotACheck {
            function: name.to_string(),
            environment: environment.clone(),
        });
    }
    Ok(function)
}

/// How a queued run is recorded by the worker that runs it: its invocation
/// `mode`, and the sandbox agent token that queued it, when one did — which
/// stamps the invocation row and the run's held-write row.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct QueuedBy<'a> {
    pub mode: &'a str,
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
    let function = runnable_function(db, &app, environment, function_name).await?;
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
mod tests {
    use super::*;
    use crate::server::api::custom_apps_functions::env_policy::{Decision, HostOp};
    use crate::server::api::custom_apps_functions::environment_gate::RefusedReason;

    fn resolved(environment: AppEnvironment) -> ResolvedEnvironment {
        ResolvedEnvironment {
            environment,
            build_id: Some(Uuid::nil()),
        }
    }

    /// Production is admitted whatever the flag says, with the policy that
    /// allows every op: a schedule, a webhook and Run now are unchanged.
    #[test]
    fn check_run_admits_production_whether_or_not_the_function_is_a_check() {
        for is_check in [true, false] {
            let admission = admit_queued(&resolved(AppEnvironment::Production), is_check)
                .expect("production is always admitted");
            assert!(admission.policy.is_production(), "is_check={is_check}");
            assert_eq!(admission.policy.decide(HostOp::StoragePut), Decision::Allow);
        }
    }

    /// Outside production a check is admitted, and under the environment's
    /// own policy — never production's — so its writes are held or isolated.
    #[test]
    fn check_run_admits_a_check_outside_production_under_that_environments_policy() {
        let admission = admit_queued(&resolved(AppEnvironment::Staging), true)
            .expect("a check runs in staging");
        assert_eq!(admission.environment.environment, AppEnvironment::Staging);
        assert!(!admission.policy.is_production());
        assert_eq!(admission.policy.environment(), &AppEnvironment::Staging);
        assert_ne!(
            admission.policy.decide(HostOp::WarehouseInsert),
            Decision::Allow,
            "a staging check must not write production's warehouse"
        );
    }

    /// The rule this module exists for: outside production, a queued run of a
    /// function that is not a check is refused — Run now, a schedule and a
    /// webhook all arrive this way.
    #[test]
    fn check_run_refuses_a_queued_run_outside_production_that_is_not_a_check() {
        let dev = AppEnvironment::Dev {
            handle: "a1".into(),
        };
        for environment in [AppEnvironment::Staging, dev] {
            let refused = admit_queued(&resolved(environment.clone()), false)
                .expect_err("only a check runs outside production");
            assert_eq!(refused.environment, environment);
            assert_ne!(
                refused.reason,
                RefusedReason::NotStaff,
                "{environment}: a queued run has no viewer to be staff or not"
            );
        }
        assert_eq!(
            admit_queued(&resolved(AppEnvironment::Staging), false)
                .unwrap_err()
                .reason,
            RefusedReason::QueuedOutsideProduction
        );
    }

    /// `admit_queued` adds no admission of its own: a check is admitted
    /// exactly where the gate admits a staff route call, so an environment
    /// the gate refuses outright stays refused for a check too.
    #[test]
    fn check_run_admits_a_check_only_where_the_gate_admits_a_staff_route_call() {
        let dev = AppEnvironment::Dev {
            handle: "a1".into(),
        };
        for environment in [AppEnvironment::Staging, dev] {
            let resolved = resolved(environment.clone());
            let by_route = environment_gate::admit(
                &resolved,
                Entrance::Route {
                    non_production_reach: true,
                },
            );
            assert_eq!(
                admit_queued(&resolved, true),
                by_route,
                "{environment}: a check takes the staff route entrance and nothing wider"
            );
        }
    }

    #[test]
    fn check_run_reads_the_check_flag_strictly() {
        let yes = serde_json::json!({ "check": true });
        assert!(manifest_marks_check(Some(&yes)));

        // Everything else is a no, and each of these is a shape a real manifest
        // produces: a handler with no `check` key, one that opted out, one
        // whose value is a truthy non-boolean, and a row with no manifest.
        for no in [
            serde_json::json!({}),
            serde_json::json!({ "check": false }),
            serde_json::json!({ "check": "yes" }),
            serde_json::json!({ "check": 1 }),
            serde_json::json!(null),
        ] {
            assert!(!manifest_marks_check(Some(&no)), "{no}");
        }
        assert!(!manifest_marks_check(None));
    }
}
