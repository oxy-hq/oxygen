//! What the queue carries for a custom-app procedure run, and the one write
//! that puts it there.
//!
//! A procedure run used to be a `tokio::spawn` in the handler, on whichever
//! serve replica took the request: a deploy or a pod restart killed it and the
//! row sat `running` until the two-hour sweep. It is now one
//! `TaskSpec::Custom { kind: "custom_app_procedure_run" }` enqueued
//! `TaskScope::Global`, claimed and executed by a driver process (a worker-fleet
//! pod, or this one under `OXY_ROLE=all`) through
//! [`super::executor::ProcedureRunExecutor`].
//!
//! Two things travel **by value**, and both are the point:
//!
//! * **The automation.** The handler resolved it (compile boundary first,
//!   working copy second, at the staging pin when there is one). The driver
//!   runs exactly that, rather than resolving a second time on another node at
//!   another moment.
//! * **Who asked.** A driver has no request. Left to the platform context the
//!   driver loop builds for a workspace, the run would carry no subject — and
//!   `airhouse_managed` mints a *system Admin* credential for a subject-less
//!   context, where the handler's context (subject, no role) mints the
//!   caller's *Reader*. So the caller's id and staging pin ride the payload
//!   and the executor rebuilds the context through
//!   `custom_apps_gates::build_caller_context`, the function the handler's
//!   `CustomAppContext::build_project_context` goes through. (Not the free
//!   `custom_apps_gates::build_project_context`, which ignores the pin.)

use agentic_automation::AutomationConfig;
use agentic_core::delegation::TaskSpec;
use chrono::Utc;
use entity::customer_app_procedure_runs::ActiveModel as ProcRunActiveModel;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, DbErr, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::server::api::custom_apps_gates::CustomAppContext;

/// `TaskSpec::Custom` discriminator. Becomes the run's `source_type`.
pub const PROCEDURE_RUN_KIND: &str = "custom_app_procedure_run";

/// One queued procedure run. `run_id` is the `customer_app_procedure_runs`
/// row, the `agentic_runs` row and the queue task at once — one task per run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureRunTask {
    pub run_id: Uuid,
    /// The project (= workspace) the gate authorized the caller for.
    pub workspace_id: Uuid,
    pub procedure_id: String,
    /// The authenticated caller. The run's context is built with this as its
    /// subject, exactly as the handler's was.
    pub user_id: Uuid,
    /// The staging revision the request was pinned to, when it was a staging
    /// request. `None` for every live request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_pin: Option<Uuid>,
    /// The resolved `AutomationConfig`, serialized.
    pub automation: JsonValue,
    /// The request's `params`, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<JsonValue>,
}

impl ProcedureRunTask {
    /// The task for a request that has passed the gate. Identity is read from
    /// the gate's own context, in this one place, so a caller cannot pass a
    /// user or a pin the gate did not establish.
    pub fn for_request(
        gate: &CustomAppContext,
        procedure_id: &str,
        automation: &AutomationConfig,
        params: Option<JsonValue>,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            run_id: Uuid::new_v4(),
            workspace_id: gate.project_id,
            procedure_id: procedure_id.to_string(),
            user_id: gate.user.id,
            staging_pin: gate.staging_pin,
            automation: serde_json::to_value(automation)?,
            params,
        })
    }

    /// Read a claimed task back. Refuses any other kind rather than guessing.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        let TaskSpec::Custom { kind, payload } = spec else {
            return Err(format!("unexpected spec for a procedure run: {spec:?}"));
        };
        if kind != PROCEDURE_RUN_KIND {
            return Err(format!("unknown procedure-run kind: {kind}"));
        }
        serde_json::from_value(payload.clone())
            .map_err(|e| format!("bad procedure-run payload: {e}"))
    }

    pub fn automation_config(&self) -> Result<AutomationConfig, String> {
        serde_json::from_value(self.automation.clone())
            .map_err(|e| format!("queued automation did not deserialise: {e}"))
    }

    /// The render context the inline runner seeds: `{{ params.<key> }}`.
    pub fn render_context(&self) -> Option<JsonValue> {
        self.params
            .as_ref()
            .map(|p| serde_json::json!({ "params": p }))
    }

    fn spec(&self) -> Result<TaskSpec, DbErr> {
        Ok(TaskSpec::Custom {
            kind: PROCEDURE_RUN_KIND.to_string(),
            payload: serde_json::to_value(self)
                .map_err(|e| DbErr::Custom(format!("encode procedure-run payload: {e}")))?,
        })
    }
}

/// Register the run and queue its task, atomically.
///
/// Three rows, one transaction: the `customer_app_procedure_runs` row the poll
/// endpoint reads, the `agentic_runs` row the driver leases, and the queue row
/// it claims. Without the transaction a failure between them leaves a row at
/// `running` that nothing will ever drive. The queue's wake-up is a NOTIFY
/// fired by a trigger and held until commit, so a driver is only ever woken
/// for a task it can claim.
///
/// Run row before queue row: `agentic_task_queue.run_id` references
/// `agentic_runs.id`.
pub async fn submit(db: &DatabaseConnection, task: &ProcedureRunTask) -> Result<(), DbErr> {
    let spec = task.spec()?;
    let task_id = task.run_id.to_string();
    let txn = db.begin().await?;

    ProcRunActiveModel {
        id: ActiveValue::Set(task.run_id),
        workspace_id: ActiveValue::Set(task.workspace_id),
        procedure_id: ActiveValue::Set(task.procedure_id.clone()),
        status: ActiveValue::Set("running".to_string()),
        params: ActiveValue::Set(task.params.clone()),
        progress_step: ActiveValue::Set(None),
        progress_percent: ActiveValue::Set(None),
        result_summary: ActiveValue::Set(None),
        result_outputs: ActiveValue::Set(None),
        error_message: ActiveValue::Set(None),
        error_code: ActiveValue::Set(None),
        cancel_requested_at: ActiveValue::Set(None),
        started_at: ActiveValue::Set(Utc::now().into()),
        // Stamped by the one attempt that begins executing (`settle::begin_execution`), never here.
        execution_started_at: ActiveValue::Set(None),
        execution_heartbeat_at: ActiveValue::Set(None),
        completed_at: ActiveValue::Set(None),
    }
    .insert(&txn)
    .await?;

    agentic_runtime::crud::insert_run(
        &txn,
        &task_id,
        &format!("procedure: {}", task.procedure_id),
        None,
        PROCEDURE_RUN_KIND,
        Some(serde_json::json!({
            "workspace_id": task.workspace_id,
            "procedure_id": task.procedure_id,
            "user_id": task.user_id,
            "trigger": "custom_app",
        })),
        task.workspace_id,
    )
    .await?;

    agentic_runtime::crud::enqueue_task(
        &txn,
        &task_id,
        &task_id,
        None,
        &spec,
        None,
        agentic_runtime::orchestrator::crud::queue::TaskScope::Global,
    )
    .await?;

    txn.commit().await
}

/// A run exactly as the start handler queues it — the row, its `agentic_runs`
/// twin and its `queued` entry — in a workspace of its own, since the row's
/// `workspace_id` is a real foreign key.
#[cfg(test)]
pub(super) async fn submitted_for_test(db: &DatabaseConnection) -> Uuid {
    let workspace_id = Uuid::new_v4();
    entity::workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(format!("procedure-run-{workspace_id}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let task = ProcedureRunTask {
        run_id: Uuid::new_v4(),
        workspace_id,
        procedure_id: "weekly".into(),
        user_id: Uuid::new_v4(),
        staging_pin: None,
        automation: serde_json::json!({ "name": "weekly", "tasks": [] }),
        params: None,
    };
    submit(db, &task).await.expect("queue the run");
    task.run_id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(automation: JsonValue) -> ProcedureRunTask {
        ProcedureRunTask {
            run_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            procedure_id: "weekly".into(),
            user_id: Uuid::new_v4(),
            staging_pin: None,
            automation,
            params: Some(serde_json::json!({ "store": "s-7" })),
        }
    }

    /// The queue is a JSON hop the handler's spawn never had. Every shipped
    /// example the handler can run must come out the other side as the
    /// automation that went in — same canonical hash — or a queued run
    /// executes something other than what the request resolved.
    ///
    /// An example that does not parse as an `AutomationConfig` is skipped, not
    /// failed: the handler answers `automation_invalid_yaml` for it and it
    /// never reaches the queue (`fruit_loop_report` declares an export format
    /// this runner has no variant for). The floor below is what keeps a skip
    /// from becoming the whole test.
    #[test]
    fn every_shipped_automation_survives_the_queue_hop() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/procedures");
        let mut checked = 0usize;
        for entry in std::fs::read_dir(&dir).expect("examples/procedures") {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".automation.yml") && !name.ends_with(".procedure.yml") {
                continue;
            }
            let yaml = std::fs::read_to_string(&path).expect("read example");
            let Ok(original) = serde_yaml::from_str::<AutomationConfig>(&yaml) else {
                continue;
            };

            let queued = task(serde_json::to_value(&original).expect("serialize"));
            let TaskSpec::Custom { payload, .. } = queued.spec().expect("spec") else {
                unreachable!()
            };
            // Through the same text encoding the `spec` column stores.
            let stored: JsonValue =
                serde_json::from_str(&serde_json::to_string(&payload).unwrap()).unwrap();
            let claimed = ProcedureRunTask::from_spec(&TaskSpec::Custom {
                kind: PROCEDURE_RUN_KIND.to_string(),
                payload: stored,
            })
            .expect("claimed task reads back");
            let restored = claimed.automation_config().expect("automation reads back");

            assert_eq!(
                agentic_automation::hash::canonical_hash(&original).unwrap(),
                agentic_automation::hash::canonical_hash(&restored).unwrap(),
                "{name} changed across the queue hop"
            );
            assert_eq!(restored.tasks.len(), original.tasks.len(), "{name}");
            checked += 1;
        }
        assert!(
            checked >= 10,
            "expected the shipped examples, found {checked}"
        );
    }

    #[test]
    fn a_claimed_task_of_another_kind_is_refused() {
        let spec = TaskSpec::Custom {
            kind: "app_function".into(),
            payload: serde_json::to_value(task(serde_json::json!({ "tasks": [] }))).unwrap(),
        };
        let err = ProcedureRunTask::from_spec(&spec).unwrap_err();
        assert!(err.contains("unknown procedure-run kind"), "{err}");
    }

    /// `{{ params.store }}` is the contract the handler's doc states; the
    /// render context the driver seeds must nest the request's params under
    /// `params`, and be absent when the request sent none.
    #[test]
    fn params_are_seeded_under_the_params_key() {
        let with = task(serde_json::json!({ "tasks": [] }));
        assert_eq!(
            with.render_context(),
            Some(serde_json::json!({ "params": { "store": "s-7" } }))
        );
        let without = ProcedureRunTask {
            params: None,
            ..with
        };
        assert_eq!(without.render_context(), None);
    }
}
