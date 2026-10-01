//! A workspace preview's Airway sample, on the executor side (phase 2b, D3).
//!
//! A sample is not an automation step — a step's `airway` is held
//! ([`super::preview_hold`]) — but a run of its own, queued by the host as
//! `TaskSpec::Custom { kind: "preview_airway_sample" }` and started here through
//! [`PipelineTaskExecutor::execute_airway_preview_sample`]:
//!
//! 1. the pipeline is loaded **through the preview platform**, so it is the
//!    branch's (staging revision), and parsed with no variables;
//! 2. [`PreviewSample::apply`] renames it `preview:<key>:<name>` before anything
//!    reads the name, points a rotate-on-use source at the sandbox company and
//!    makes the run single-flight;
//! 3. the same tail production runs (`launch_prepared_airway`) takes the lease,
//!    resolves secrets through the preview platform (which withholds production's
//!    QuickBooks vars) and the destination (which it maps into the preview's
//!    own schemas), and starts the worker — with the pipeline-global state store
//!    under the preview name, never the run-scoped one ([`resume_run_id_for`]).
//!
//! So the sample's lease, cursor, stored schema, load audit and run extension
//! are all keyed by the preview name, and production's `(workspace, name)` rows
//! are never read or written.

use agentic_airway::preview::{PreviewSample, scoped_pipeline_name};
use agentic_runtime::worker::ExecutingTask;

use super::{LoadFailureAction, PipelineTaskExecutor, action_for_load_failure, deferred_task};

/// The `TaskSpec::Custom` kind that runs a sample, the `source_type` of its
/// `agentic_runs` row, and the key its Airway events are rendered under.
pub const PREVIEW_AIRWAY_SAMPLE: &str = "preview_airway_sample";

/// Who a prepared Airway run belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaunchMode {
    /// An ordinary run: production's name, lease, cursor and store choice.
    Production,
    /// A workspace preview's sample: its own `preview:` name, and never the
    /// run-scoped store.
    PreviewSample,
}

/// The run id that selects the run-scoped state store, or `None` for the
/// pipeline-global one. A sample always takes the pipeline-global store under
/// its preview name: the run-scoped store reads and records under whatever
/// name it is given but writes its cursor to the run and never saves a schema,
/// so a sample through it would have no stored schema to compare with live.
pub(super) fn resume_run_id_for(mode: LaunchMode, resumable: bool, run_id: &str) -> Option<String> {
    match mode {
        LaunchMode::Production => resumable.then(|| run_id.to_string()),
        LaunchMode::PreviewSample => None,
    }
}

/// A sample runs under a preview name, or not at all: checked where the lease
/// is taken, so no path can lease production's name for a sample.
pub(super) fn check_launch_name(mode: LaunchMode, name: &str) -> Result<(), String> {
    let prefix = agentic_airway::config::RESERVED_NAME_PREFIX;
    match mode {
        LaunchMode::PreviewSample if !name.starts_with(prefix) => Err(format!(
            "airway: a preview sample must run under its `{prefix}` name, not `{name}`"
        )),
        _ => Ok(()),
    }
}

impl PipelineTaskExecutor {
    /// Start a workspace preview's sample of `pipeline_ref` (module doc).
    ///
    /// Refused unless this executor's platform is the preview `sample` belongs
    /// to, driving run `run_id`: on any other platform the destination and the
    /// secrets would be production's. A failure after the lease was taken
    /// releases it, as the production dispatch does.
    pub async fn execute_airway_preview_sample(
        &self,
        run_id: &str,
        pipeline_ref: &str,
        sample: &PreviewSample,
    ) -> Result<ExecutingTask, String> {
        let scope = self.platform.preview_scope().ok_or_else(|| {
            "airway: a preview sample runs only on its workspace preview's platform".to_string()
        })?;
        if scope.preview_key != sample.preview_key || scope.run_id != run_id {
            return Err(format!(
                "airway: sample run {run_id} of preview {} is not this platform's \
                 (run {}, preview {})",
                sample.preview_key, scope.run_id, scope.preview_key
            ));
        }
        let yaml =
            match crate::pipeline_ref::load_pipeline_yaml(self.platform.as_ref(), pipeline_ref)
                .await
            {
                Ok(yaml) => yaml,
                Err(e) => match action_for_load_failure(e) {
                    LoadFailureAction::Defer {
                        delay_secs,
                        max_wait_secs,
                        reason,
                    } => return Ok(deferred_task(delay_secs, max_wait_secs, reason)),
                    LoadFailureAction::Fail(m) => return Err(m),
                },
            };
        let started = self.start_sample(run_id, pipeline_ref, &yaml, sample).await;
        if started.is_err() {
            crate::airway_run::release_airway_lease(&self.db, run_id).await;
        }
        started
    }

    /// Parse, apply the sample, record its run extension and launch.
    async fn start_sample(
        &self,
        run_id: &str,
        pipeline_ref: &str,
        yaml: &str,
        sample: &PreviewSample,
    ) -> Result<ExecutingTask, String> {
        let mut spec = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(yaml, None)
            .map_err(|e| format!("airway: parse `{pipeline_ref}`: {e}"))?;
        let workspace_id = self.platform.workspace_id();
        let resolved =
            crate::airway_config::resolve_admission(&self.db, &spec.source.kind, workspace_id)
                .await
                .map_err(|e| format!("airway: resolving the sample's admission: {e}"))?;
        let mut admission = agentic_airway::AirwayAdmission::from_strings(
            resolved.contract_policy.as_deref(),
            resolved.environment.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        let applied = sample
            .apply(&mut spec, &mut admission)
            .map_err(|e| format!("airway: preview sample refused: {e}"))?;
        tracing::info!(
            target: "preview", %run_id, pipeline = %applied.live_name, name = %applied.name,
            "starting a preview Airway sample"
        );
        let policy = resolved.contract_policy.as_deref();
        self.record_sample_extension(run_id, pipeline_ref, &spec, sample, (policy, admission))
            .await?;
        let window = sample.window.map(|w| w.as_backfill());
        let backfill = window
            .as_ref()
            .map(|(from, to)| (from.as_str(), to.as_str()));
        self.launch_prepared_airway(
            run_id,
            pipeline_ref,
            spec,
            &[],
            backfill,
            admission,
            LaunchMode::PreviewSample,
        )
        .await
    }

    /// The sample's `airway_run_extensions` row, under its preview name and a
    /// preview-scoped ref (so no production Airway route resolves it). Kept
    /// when a re-driven sample already has one.
    async fn record_sample_extension(
        &self,
        run_id: &str,
        pipeline_ref: &str,
        spec: &agentic_airway::AirwayPipelineSpec,
        sample: &PreviewSample,
        (contract_policy, admission): (Option<&str>, agentic_airway::AirwayAdmission),
    ) -> Result<(), String> {
        use agentic_airway::extension::run_extension;
        let existing = run_extension::get_run_extension(&self.db, run_id)
            .await
            .map_err(|e| format!("airway: reading the sample's run extension: {e}"))?;
        if existing.is_some() {
            return Ok(());
        }
        let scoped_ref = scoped_pipeline_name(&sample.preview_key, pipeline_ref);
        let environment = match admission.environment {
            agentic_airway::Environment::Sandbox => "sandbox",
            _ => "production",
        };
        run_extension::insert_run_extension(
            &self.db,
            run_id,
            spec,
            Some(&scoped_ref),
            contract_policy,
            Some(environment),
        )
        .await
        .map_err(|e| format!("airway: recording the sample's run extension: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample never selects the run-scoped store, even for a resumable
    /// window; production's choice is unchanged.
    #[test]
    fn a_sample_never_selects_the_run_scoped_store() {
        assert_eq!(
            resume_run_id_for(LaunchMode::PreviewSample, true, "r"),
            None
        );
        assert_eq!(
            resume_run_id_for(LaunchMode::PreviewSample, false, "r"),
            None
        );
        assert_eq!(
            resume_run_id_for(LaunchMode::Production, true, "r").as_deref(),
            Some("r")
        );
        assert_eq!(resume_run_id_for(LaunchMode::Production, false, "r"), None);
    }

    #[test]
    fn a_sample_is_launched_only_under_a_preview_name() {
        assert!(check_launch_name(LaunchMode::PreviewSample, "orders").is_err());
        assert!(check_launch_name(LaunchMode::PreviewSample, "preview:k_1:orders").is_ok());
        assert!(check_launch_name(LaunchMode::Production, "orders").is_ok());
    }
}
