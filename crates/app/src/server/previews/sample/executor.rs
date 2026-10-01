//! The `preview_airway_sample` task on the worker fleet.

use std::sync::Arc;

use agentic_airway::preview::PreviewSample;
use agentic_core::delegation::{TaskAssignment, TaskSpec};
use agentic_pipeline::executor::PipelineTaskExecutor;
use agentic_pipeline::platform::PlatformContext;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::Value;

use super::outcome::{Watched, watch};
use super::{PREVIEW_AIRWAY_SAMPLE_KIND, RUN_KIND, SampleOptions, deadline, max_secs, remaining};
use crate::agentic_wiring::preview_airhouse::{PreviewAirhousePorts, WorkspaceAirhouse};
use crate::agentic_wiring::preview_ctx::PreviewPlatformContext;

/// Runs a `preview_airway_sample` task. Registered in
/// `router::recovery::build_custom_task_registry`; a pod without it fails the
/// task (unknown kind) rather than running anything.
pub struct PreviewAirwaySampleExecutor {
    pub db: DatabaseConnection,
    pub airhouse: Arc<dyn PreviewAirhousePorts>,
}

impl PreviewAirwaySampleExecutor {
    /// On the workspace's own Airhouse.
    pub fn airhouse(db: DatabaseConnection) -> Self {
        Self {
            db,
            airhouse: WorkspaceAirhouse::shared(),
        }
    }
}

#[async_trait]
impl TaskExecutor for PreviewAirwaySampleExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let run_id = sample_run_id(&assignment.spec)?;
        let row = entity::workspace_preview_runs::Entity::find_by_id(run_id.clone())
            .one(&self.db)
            .await
            .map_err(|e| format!("preview sample {run_id}: {e}"))?
            .filter(|r| r.kind == RUN_KIND)
            .ok_or_else(|| format!("preview sample {run_id} has no workspace_preview_runs row"))?;
        let options = SampleOptions::from_json(&row.options)?;
        let platform = PreviewPlatformContext::new_with(&self.db, &row, Arc::clone(&self.airhouse))
            .await
            .map_err(|e| e.to_string())?;
        let platform = Arc::new(platform);
        let sample = PreviewSample {
            preview_key: row.preview_key.clone(),
            window: options.window,
            resources: options.resources.clone(),
            sandbox: platform.sample().and_then(|s| s.sandbox()).cloned(),
        };
        let as_platform: Arc<dyn PlatformContext> = platform.clone();
        let executor = PipelineTaskExecutor::bare(as_platform, self.db.clone());
        let target = row.target_ref.clone().unwrap_or_default();
        let started = executor
            .execute_airway_preview_sample(&run_id, &target, &sample)
            .await;
        let inner = match started {
            Ok(inner) => inner,
            // The platform's own words when it refused the destination: the
            // pipeline port could only answer "none".
            Err(e) => return Err(platform.sample().and_then(|s| s.refusal()).unwrap_or(e)),
        };
        let watched = Watched {
            db: self.db.clone(),
            workspace_id: row.workspace_id,
            preview_key: row.preview_key,
            run_id,
            pipeline: options.pipeline_name,
            dataset: options.dataset_name,
            // From the run's start: a task claimed again after a crash keeps
            // the clock it had.
            deadline: remaining(
                deadline(
                    options.wall_clock_capped,
                    max_secs(),
                    crate::server::previews::runs::max_minutes(),
                ),
                row.started_at,
                chrono::Utc::now(),
            ),
        };
        Ok(watch(inner, watched))
    }
}

pub(super) fn sample_run_id(spec: &TaskSpec) -> Result<String, String> {
    match spec {
        TaskSpec::Custom { kind, payload } if kind == PREVIEW_AIRWAY_SAMPLE_KIND => payload
            .get("preview_run_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "preview_airway_sample payload missing string preview_run_id".into()),
        other => Err(format!(
            "unexpected spec for PreviewAirwaySampleExecutor: {other:?}"
        )),
    }
}
