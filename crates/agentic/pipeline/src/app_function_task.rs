//! The payload of a queued custom-app function run:
//! `TaskSpec::Custom { kind: "app_function", payload }`.
//!
//! Written here (a cron fire, `enqueue_app_function_job` for a run-now or a
//! webhook) and read by the host's `AppFunctionTaskExecutor`. It was untyped
//! JSON that each side built and picked apart by hand, which is how a field one
//! side adds becomes a field the other side silently ignores.
//!
//! **`environment` is the field that must never be silently ignored.** Custom
//! apps gain non-production environments
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3.4): a
//! staging schedule or webhook enqueues a task for its environment, and a
//! worker that dropped the field would run that staging task against
//! production. So the reader that knows the field shipped a release **before**
//! any writer set it, and the reader refuses any environment it cannot run.
//! One writer sets it now: `scheduler::enqueue_app_function_job_in`, for a
//! check run a staff caller asked for in a named environment. Every other
//! task — cron fires, Run now, webhooks — still carries `None` (production).
//!
//! Wire-compatible both ways. Absent optional fields are omitted rather than
//! written as `null`, which readers of the untyped payload already treated the
//! same, and a payload written before this type existed deserializes into it.

use serde::{Deserialize, Serialize};

/// The `kind` of the `TaskSpec::Custom` that carries an [`AppFunctionTask`].
pub const APP_FUNCTION_KIND: &str = "app_function";

/// One queued custom-app function run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppFunctionTask {
    /// The app's id, as a string (the scheduler is entity-free).
    pub app_id: String,
    pub function_name: String,
    /// What seeded the run: `scheduled`, `manual` or `webhook`. `None` on tasks
    /// queued before the field was carried, which were all cron fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// The function's input params, handed to the isolate as its request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// W3C `traceparent` of the request that enqueued the run, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
    /// The app environment to run in, as an `app_environments.name`. `None`
    /// means production. Set only by `scheduler::enqueue_app_function_job_in`
    /// — see the module docs for why the reader came first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

impl AppFunctionTask {
    /// A task for `function_name` of `app_id` with every optional field unset.
    pub fn new(app_id: impl Into<String>, function_name: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            function_name: function_name.into(),
            trigger: None,
            input: None,
            traceparent: None,
            environment: None,
        }
    }

    /// The JSON the task queue stores.
    pub fn to_payload(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("AppFunctionTask serializes to a JSON object")
    }

    /// Parse a stored payload. An error names what is missing or malformed.
    pub fn from_payload(payload: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(payload.clone())
            .map_err(|e| format!("malformed app_function payload: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The shape the cron arm wrote before this type existed.
    #[test]
    fn a_legacy_cron_payload_reads_as_production() {
        let task = AppFunctionTask::from_payload(&json!({
            "app_id": "a1",
            "function_name": "refresh",
            "trigger": "scheduled",
        }))
        .expect("legacy payload parses");
        assert_eq!(task.environment, None);
        assert_eq!(task.trigger.as_deref(), Some("scheduled"));
    }

    /// The shape `enqueue_app_function_job` wrote: explicit `null`s.
    #[test]
    fn a_legacy_job_payload_with_nulls_parses() {
        let task = AppFunctionTask::from_payload(&json!({
            "app_id": "a1",
            "function_name": "refresh",
            "trigger": "manual",
            "input": null,
            "traceparent": null,
        }))
        .expect("legacy payload parses");
        assert_eq!(task.input, None);
        assert_eq!(task.traceparent, None);
        assert_eq!(task.environment, None);
    }

    #[test]
    fn the_environment_is_read_when_present() {
        let task = AppFunctionTask::from_payload(&json!({
            "app_id": "a1",
            "function_name": "refresh",
            "environment": "staging",
        }))
        .expect("parses");
        assert_eq!(task.environment.as_deref(), Some("staging"));
    }

    /// An old reader looks fields up by name and treats a missing one as
    /// absent, so omitting unset fields is what keeps new payloads readable by
    /// a worker one release behind.
    #[test]
    fn unset_fields_are_omitted_not_null() {
        let mut task = AppFunctionTask::new("a1", "refresh");
        task.trigger = Some("manual".into());
        assert_eq!(
            task.to_payload(),
            json!({ "app_id": "a1", "function_name": "refresh", "trigger": "manual" })
        );
    }

    #[test]
    fn a_payload_without_an_app_is_refused() {
        assert!(AppFunctionTask::from_payload(&json!({ "function_name": "x" })).is_err());
    }
}
