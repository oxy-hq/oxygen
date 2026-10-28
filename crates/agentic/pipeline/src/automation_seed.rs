//! Seeding an automation run as rows, on whatever connection the caller holds.
//!
//! `start_automation_run` mints its own id and writes on a pooled connection;
//! a caller that must commit the run together with its own bookkeeping (the
//! experiment runner stamps a batch of flip rows in the same transaction) uses
//! `seed_automation_run` with an id it minted. Both build the metadata and the
//! task spec through the functions below, so the two paths cannot drift.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use sea_orm::ConnectionTrait;
use serde_json::Value;
use uuid::Uuid;

use crate::automation_run::{AutomationRunError, StartAutomationRequest};

pub fn automation_metadata(request: &StartAutomationRequest) -> Value {
    let mut metadata = serde_json::json!({
        "workflow_ref": request.workflow_ref,
        "cache_enabled": request.cache_enabled,
        "retry_from_run_id": request.retry_from_run_id,
        // `null` when not supplied — the executor only acts when this is
        // a non-empty array, so the conventional `Option<Vec<_>>` →
        // `Some([...])` round-trip is preserved through Serde.
        "invalidate_steps": request.invalidate_steps,
        // `null` when not supplied. Executor reads this once at seed and
        // stamps onto `agentic_workflow_state.invalidate_iterations` so
        // the decider applies it inline without re-reading metadata.
        "invalidate_iterations": request.invalidate_iterations,
        // Stamped so `retry_run` can reconstruct a parameterised retry
        // without re-reading the queue spec.
        "variables": request.variables,
    });
    crate::scheduler::stamp_trigger_metadata(
        &mut metadata,
        &request.trigger,
        &request.logical_date,
        &request.retry_of,
    );
    metadata
}

pub fn thread_uuid(request: &StartAutomationRequest) -> Result<Option<Uuid>, AutomationRunError> {
    request
        .thread_id
        .as_deref()
        .map(|s| {
            Uuid::parse_str(s)
                .map_err(|e| AutomationRunError::InvalidInput(format!("thread_id {s:?}: {e}")))
        })
        .transpose()
}

pub fn automation_task_spec(request: StartAutomationRequest) -> TaskSpec {
    TaskSpec::Automation {
        workflow_ref: request.workflow_ref,
        variables: request.variables,
        retry_from_run_id: request.retry_from_run_id,
        cache_enabled: request.cache_enabled,
        body: None,
        initial_render_context: None,
    }
}

/// `insert_run_with_schedule` takes a `&DatabaseConnection` only, so a
/// scheduled seed cannot join a caller's transaction; refuse it rather than
/// silently dropping the schedule attribution.
pub fn ensure_unscheduled(request: &StartAutomationRequest) -> Result<(), AutomationRunError> {
    match request.schedule_id {
        Some(_) => Err(AutomationRunError::InvalidInput(
            "seed_automation_run does not stamp schedule_id; use start_automation_run".into(),
        )),
        None => Ok(()),
    }
}

/// Record on the run's metadata who it was started for, so a driver in another
/// process (a `Global` entry is anyone's to claim) runs it as that requester
/// instead of on its own subject-less context. Nothing when there is none.
pub fn stamp_requester(metadata: &mut Value, requester: Option<crate::platform::Requester>) {
    if let (Some(requester), Some(map)) = (requester, metadata.as_object_mut()) {
        map.insert(
            crate::platform::Requester::KEY.to_string(),
            requester.to_metadata(),
        );
    }
}

/// Insert the `agentic_runs` row and enqueue its root task under `run_id`
/// (root task id == run id, like every Global seed — see
/// `scheduler::start_health_eval_run`). Runs on any connection, including an
/// open transaction, and commits nothing itself.
pub async fn seed_automation_run(
    db: &impl ConnectionTrait,
    run_id: &str,
    request: StartAutomationRequest,
    scope: crud::TaskScope,
    workspace_id: Uuid,
) -> Result<(), AutomationRunError> {
    seed_automation_run_as(db, run_id, request, scope, workspace_id, None).await
}

/// [`seed_automation_run`], recording who the run was started for
/// ([`stamp_requester`]).
pub async fn seed_automation_run_as(
    db: &impl ConnectionTrait,
    run_id: &str,
    request: StartAutomationRequest,
    scope: crud::TaskScope,
    workspace_id: Uuid,
    requester: Option<crate::platform::Requester>,
) -> Result<(), AutomationRunError> {
    request.validate()?;
    ensure_unscheduled(&request)?;
    let thread = thread_uuid(&request)?;
    let question = format!("workflow: {}", request.workflow_ref);
    let mut metadata = automation_metadata(&request);
    stamp_requester(&mut metadata, requester);
    crud::insert_run(
        db,
        run_id,
        &question,
        thread,
        agentic_automation::SOURCE_TYPE,
        Some(metadata),
        workspace_id,
    )
    .await?;
    let spec = automation_task_spec(request);
    crud::enqueue_task(db, run_id, run_id, None, &spec, None, scope).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request() -> StartAutomationRequest {
        StartAutomationRequest {
            workflow_ref: "automations/promo_on.automation.yml".into(),
            variables: Some(
                json!({ "unit": "bondi", "state": "on", "experiment": "p", "wave": 0 }),
            ),
            retry_from_run_id: None,
            cache_enabled: false,
            invalidate_steps: None,
            invalidate_iterations: None,
            thread_id: None,
            schedule_id: None,
            trigger: Some("experiment".into()),
            logical_date: Some("2026-03-01T13:00:00Z".parse().unwrap()),
            retry_of: None,
        }
    }

    #[test]
    fn metadata_stamps_the_trigger_the_ref_and_the_variables() {
        let m = automation_metadata(&request());
        assert_eq!(m["trigger"], "experiment");
        assert_eq!(m["workflow_ref"], "automations/promo_on.automation.yml");
        assert_eq!(m["variables"]["unit"], "bondi");
        assert_eq!(m["logical_date"], "2026-03-01T13:00:00+00:00");
    }

    #[test]
    fn task_spec_carries_the_ref_and_the_variables() {
        let TaskSpec::Automation {
            workflow_ref,
            variables,
            body,
            ..
        } = automation_task_spec(request())
        else {
            panic!("an automation spec");
        };
        assert_eq!(workflow_ref, "automations/promo_on.automation.yml");
        assert_eq!(variables.unwrap()["state"], "on");
        assert!(body.is_none(), "the runner loads the automation by path");
    }

    #[test]
    fn a_scheduled_request_is_refused_here() {
        let scheduled = StartAutomationRequest {
            schedule_id: Some("s1".into()),
            ..request()
        };
        assert!(matches!(
            ensure_unscheduled(&scheduled),
            Err(AutomationRunError::InvalidInput(m)) if m.contains("schedule_id")
        ));
        assert!(ensure_unscheduled(&request()).is_ok());
    }

    #[test]
    fn a_malformed_thread_id_is_refused_before_any_write() {
        let bad = StartAutomationRequest {
            thread_id: Some("nope".into()),
            ..request()
        };
        assert!(matches!(
            thread_uuid(&bad),
            Err(AutomationRunError::InvalidInput(_))
        ));
    }
}
