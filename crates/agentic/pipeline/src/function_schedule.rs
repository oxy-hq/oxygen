//! A custom-app function's schedule row: what it points at, and how a fire
//! of it is queued.
//!
//! A `function` row of `agentic_schedules` names its target in `target_ref`.
//! Production's is `<app_id>/<function>`, as it has been since custom-app
//! schedules existed; a non-production environment's is
//! `<app_id>/<environment>/<function>`
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §4.5). App
//! ids, environment names and function names never contain `/`, so the number
//! of segments alone tells the two apart. The host writes the rows
//! (`custom_apps_env_schedules::reconcile`) and the fire arm reads them, both
//! through [`FunctionScheduleRef`].
//!
//! **A fire names its environment twice**: on the task
//! ([`AppFunctionTask::environment`](crate::app_function_task::AppFunctionTask)),
//! which is what the worker resolves and admits, and on the run
//! (`RUN_ENVIRONMENT_KEY`), which keeps it out of the tenant's run feed.
//!
//! **A worker one release behind** splits the ref at its first `/` only, so it
//! reads `<environment>/<function>` as the function's name, finds no such
//! function and fails the run. An environment's fire never runs on production.

use agentic_runtime::entity::schedule;
use agentic_runtime::lifecycle::crud::runs::insert_run_with_schedule;
use sea_orm::{DatabaseConnection, TransactionTrait};

use crate::app_function_task::{APP_FUNCTION_KIND, AppFunctionTask};

/// Where a `function` schedule row points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionScheduleRef<'a> {
    pub app_id: &'a str,
    /// The `app_environments.name` the function fires in; `None` is
    /// production.
    pub environment: Option<&'a str>,
    pub function_name: &'a str,
}

impl<'a> FunctionScheduleRef<'a> {
    /// Read a `target_ref`: two segments are production's, three an
    /// environment's. Anything else — an empty segment, or more than three —
    /// names nothing.
    pub fn parse(target_ref: &'a str) -> Option<Self> {
        let segments: Vec<&str> = target_ref.split('/').collect();
        if segments.iter().any(|segment| segment.is_empty()) {
            return None;
        }
        match segments.as_slice() {
            [app_id, function_name] => Some(Self {
                app_id,
                environment: None,
                function_name,
            }),
            [app_id, environment, function_name] => Some(Self {
                app_id,
                environment: Some(environment),
                function_name,
            }),
            _ => None,
        }
    }

    /// The `target_ref` this names.
    pub fn target_ref(&self) -> String {
        match self.environment {
            None => format!("{}/{}", self.app_id, self.function_name),
            Some(environment) => format!("{}/{environment}/{}", self.app_id, self.function_name),
        }
    }

    /// The schedule row's `name`: `fn:` and the ref.
    pub fn schedule_name(&self) -> String {
        format!("fn:{}", self.target_ref())
    }
}

/// Seed a run for a fire of the `function` row `s` and queue its task.
/// `trigger` is the run's label: `scheduled` for the tick, `manual` for the
/// tenant's run-now (production rows only — the host's schedules API never
/// reaches an environment's).
///
/// Entity/oxy-free: the host's `AppFunctionTaskExecutor` resolves the app,
/// the environment's build and whether it may run there at all.
pub(crate) async fn fire(
    db: &DatabaseConnection,
    s: &schedule::Model,
    trigger: &str,
) -> Result<String, String> {
    let target = FunctionScheduleRef::parse(&s.target_ref).ok_or_else(|| {
        format!(
            "function schedule target_ref must be '<app_id>/<function_name>' or \
             '<app_id>/<environment>/<function_name>', got {:?}",
            s.target_ref
        )
    })?;
    let run_id = uuid::Uuid::new_v4().to_string();
    // The run row and its queue row commit together: a run committed alone is
    // "never started" to a booting server's cleanup, which fails it.
    let txn = db.begin().await.map_err(|e| e.to_string())?;
    insert_run_with_schedule(
        &txn,
        &run_id,
        &s.name,
        None,
        "app_function",
        Some(run_metadata(trigger, target.environment)),
        &s.id,
        s.workspace_id,
    )
    .await
    .map_err(|e| e.to_string())?;
    // Carry the trigger (`scheduled` for a cron fire, `manual` for a run-now)
    // so the executor records the invocation `mode` to match, and the
    // environment so it resolves that environment's build.
    let mut task = AppFunctionTask::new(target.app_id, target.function_name);
    task.trigger = Some(trigger.to_string());
    task.environment = target.environment.map(str::to_string);
    let spec = agentic_core::delegation::TaskSpec::Custom {
        kind: APP_FUNCTION_KIND.into(),
        payload: task.to_payload(),
    };
    agentic_runtime::crud::enqueue_task(
        &txn,
        &run_id,
        &run_id,
        None,
        &spec,
        task_policy(s).as_ref(),
        agentic_runtime::crud::TaskScope::Global,
    )
    .await
    .map_err(|e| e.to_string())?;
    txn.commit().await.map_err(|e| e.to_string())?;
    Ok(run_id)
}

/// The run's metadata: the trigger the dashboard labels it with, and — for an
/// environment's fire — the environment, which keeps it out of the tenant's
/// run feed and is what the staff run detail reports while it is queued.
fn run_metadata(trigger: &str, environment: Option<&str>) -> serde_json::Value {
    let mut metadata = serde_json::json!({});
    crate::scheduler::stamp_trigger_metadata(
        &mut metadata,
        &Some(trigger.to_string()),
        &None,
        &None,
    );
    if let Some(name) = environment {
        metadata[agentic_runtime::crud::RUN_ENVIRONMENT_KEY] =
            serde_json::Value::String(name.to_string());
    }
    metadata
}

/// The function's retry policy, stamped onto the row's `variables` by the
/// host (`{"task_policy": …}`), so a fire inherits the queue's retry/backoff
/// without this crate reading a manifest.
fn task_policy(s: &schedule::Model) -> Option<agentic_core::delegation::TaskPolicy> {
    s.variables
        .as_ref()
        .and_then(|v| v.get("task_policy"))
        .and_then(|p| serde_json::from_value(p.clone()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "6f1c1f5e-4b2a-4d8e-9d59-0b8f8a3e2c11";

    #[test]
    fn a_two_segment_ref_is_production() {
        let raw = format!("{APP}/refresh");
        let target = FunctionScheduleRef::parse(&raw).expect("parses");
        assert_eq!(target.app_id, APP);
        assert_eq!(target.environment, None);
        assert_eq!(target.function_name, "refresh");
        assert_eq!(target.target_ref(), format!("{APP}/refresh"));
        assert_eq!(target.schedule_name(), format!("fn:{APP}/refresh"));
    }

    #[test]
    fn a_three_segment_ref_names_its_environment() {
        let raw = format!("{APP}/dev-a1/refresh");
        let target = FunctionScheduleRef::parse(&raw).expect("parses");
        assert_eq!(target.environment, Some("dev-a1"));
        assert_eq!(target.function_name, "refresh");
        assert_eq!(target.target_ref(), raw);
        assert_eq!(target.schedule_name(), format!("fn:{raw}"));
    }

    #[test]
    fn anything_else_names_nothing() {
        for raw in [
            String::new(),
            APP.to_string(),
            format!("{APP}/"),
            format!("{APP}//refresh"),
            format!("/{APP}/refresh"),
            format!("{APP}/staging/refresh/extra"),
        ] {
            assert_eq!(FunctionScheduleRef::parse(&raw), None, "{raw:?}");
        }
    }

    /// What a worker one release behind does with an environment's ref: the
    /// first `/` splits off the app, and the rest is a function name no build
    /// has — never production's function of that name.
    #[test]
    fn an_old_reader_sees_no_production_function_in_an_environments_ref() {
        let raw = format!("{APP}/staging/refresh");
        let (_, old_function_name) = raw.split_once('/').expect("a slash");
        assert_eq!(old_function_name, "staging/refresh");
        assert_ne!(old_function_name, "refresh");
    }

    #[test]
    fn an_environments_fire_stamps_its_environment_on_the_run() {
        let metadata = run_metadata("scheduled", Some("dev-a1"));
        assert_eq!(metadata["trigger"], "scheduled");
        assert_eq!(
            metadata[agentic_runtime::crud::RUN_ENVIRONMENT_KEY],
            "dev-a1"
        );
        let production = run_metadata("scheduled", None);
        assert_eq!(production, serde_json::json!({ "trigger": "scheduled" }));
    }
}
