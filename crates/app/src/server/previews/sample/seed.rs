//! Seeding a sample's run when the workspace's preview queue starts it
//! (`runs::advance`, in its transaction): the `agentic_runs` row and the
//! worker-fleet task. Rows only; the sample runs on the fleet.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use sea_orm::{ConnectionTrait, DbErr};
use serde_json::{Value, json};
use uuid::Uuid;

use super::PREVIEW_AIRWAY_SAMPLE_KIND;

/// The run `runs::advance` just started.
pub struct SampleSeed<'a> {
    pub run_id: &'a str,
    pub workspace_id: Uuid,
    pub branch: &'a str,
    pub preview_key: &'a str,
    pub target_ref: &'a str,
    pub options: &'a Value,
}

/// The sample's `agentic_runs` row — `source_type = preview_airway_sample`, so
/// the run viewer renders its Airway events, and `metadata.trigger =
/// "preview"`, so workspace health never counts it — and its task, on the
/// global queue. No retry policy: a failed sample is looked at, not re-run.
pub async fn seed<C: ConnectionTrait>(db: &C, s: &SampleSeed<'_>) -> Result<(), DbErr> {
    let metadata = json!({
        "trigger": "preview",
        "branch": s.branch,
        "preview_key": s.preview_key,
        "target_ref": s.target_ref,
        "pipeline_name": s.options.get("pipeline_name"),
    });
    agentic_runtime::crud::insert_run(
        db,
        s.run_id,
        &format!("Airway sample of {} on {}", s.target_ref, s.branch),
        None,
        PREVIEW_AIRWAY_SAMPLE_KIND,
        Some(metadata),
        s.workspace_id,
    )
    .await?;
    agentic_runtime::crud::enqueue_task(
        db,
        s.run_id,
        s.run_id,
        None,
        &TaskSpec::Custom {
            kind: PREVIEW_AIRWAY_SAMPLE_KIND.to_string(),
            payload: json!({ "preview_run_id": s.run_id }),
        },
        None,
        TaskScope::Global,
    )
    .await
}
