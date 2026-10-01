//! Workspace-preview runs: which platform drives a root run, and what a preview
//! platform tells the executor about itself.
//!
//! A preview run is an ordinary queued automation whose platform answers every
//! compile-boundary read from a branch's staging revision and holds every write.
//! The global-run driver does not know which runs those are; the host does
//! (`workspace_preview_runs` in Oxy). So the three recovery entry points take a
//! **required** [`RunPlatformResolver`] and ask it, per root run, which platform
//! to drive that root with. Children in the tree are claimed by the root's
//! scoped worker and share its platform.
//!
//! Required rather than a builder step for the reason `AirwayWorker::new` takes
//! its admission as an argument: a call site that forgot to wire it would drive
//! preview runs with the production platform, and nothing would fail.

use std::sync::Arc;

use async_trait::async_trait;

use super::PlatformContext;

/// What an automation's `airway` step does inside a preview run.
///
/// One answer: held. A bounded sample of a pipeline into preview-owned state
/// (phase 2b) is not a step mode but a run of its own, queued by the host and
/// started through `PipelineTaskExecutor::execute_airway_preview_sample`, so
/// no step inside a procedure dry run can take a lease or a destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AirwayStepMode {
    /// The step succeeds without running: its result records the pipeline it
    /// would have run, and the production lease, cursor and destination are
    /// never touched.
    Hold,
}

/// The preview a platform is driving, when it is one. `None` from
/// [`super::ProjectContext::preview_scope`] — every production host — means
/// "not a preview".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewScope {
    /// The root run this platform drives. Names scoped to any other run id are
    /// refused (`agentic_automation::preview_names::unscope`).
    pub run_id: String,
    /// The preview's namespace key (`<slug>_<hash>`).
    pub preview_key: String,
    /// The staging revision every compile-boundary read answers from.
    pub revision_id: uuid::Uuid,
    pub airway_steps: AirwayStepMode,
}

/// Picks the platform a root run is driven with.
#[async_trait]
pub trait RunPlatformResolver: Send + Sync {
    /// `Ok(base)` for an ordinary run, a preview platform for a preview-owned
    /// one. `Err` means "could not tell": the root is **not driven this tick**,
    /// so a transient lookup failure can never drive a preview run as
    /// production.
    async fn platform_for(
        &self,
        root: &agentic_runtime::entity::run::Model,
        base: Arc<dyn PlatformContext>,
    ) -> Result<Arc<dyn PlatformContext>, String>;
}

/// Every root runs on the base platform. For tests, the CLI, and any host with
/// no previews.
pub struct IdentityResolver;

#[async_trait]
impl RunPlatformResolver for IdentityResolver {
    async fn platform_for(
        &self,
        _root: &agentic_runtime::entity::run::Model,
        base: Arc<dyn PlatformContext>,
    ) -> Result<Arc<dyn PlatformContext>, String> {
        Ok(base)
    }
}

/// The platform `root` is driven with, or `None` when the resolver could not
/// say — logged here so each recovery loop skips the root the same way.
pub(crate) async fn platform_for_root(
    resolver: &dyn RunPlatformResolver,
    root: &agentic_runtime::entity::run::Model,
    base: &Arc<dyn PlatformContext>,
) -> Option<Arc<dyn PlatformContext>> {
    match resolver.platform_for(root, base.clone()).await {
        Ok(platform) => Some(platform),
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                run_id = %root.id,
                error = %e,
                "could not resolve the platform for this run; not driving it this tick"
            );
            None
        }
    }
}

/// Whether `platform` serves a workspace preview — a dry run's scope, or a
/// request pinned to a preview. Either signal is enough: this answers "may
/// anything here write?", so it fails closed.
pub fn is_preview(platform: &dyn PlatformContext) -> bool {
    platform.preview_scope().is_some() || platform.is_workspace_preview()
}

/// The automation runner an analytics agent may delegate to — `None` when its
/// `context:` resolved no automation files (the callers' rule: an empty list
/// would fall back to every automation in the project), and `None` on any
/// workspace-preview platform ([`is_preview`]): a dry run's, and a chat
/// request pinned to a preview.
///
/// The preview case fails closed. The runner's delegation enqueues
/// `TaskSpec::Automation` with the plain automation ref — the one child spec
/// no preview scope covers — so a pod without preview code that recovered the
/// run would resolve production's automation, with production's names, and
/// its default review would let every write through. A chat request in a
/// preview reads the branch, so a runner there would delegate to the branch's
/// automation, and its `execute_sql` / `http_request` / `airway` steps would
/// run against production. Without a runner the agent answers without
/// delegating, as it does with no builder bridges.
///
/// Every analytics start and resume, and the headless runner, go through
/// here; `tests::no_runner_is_built_outside_the_gate` keeps it that way.
/// Public so a host can assert the gate against its own platform.
pub fn automation_subrun_runner(
    platform: &Arc<dyn PlatformContext>,
    automation_files: Vec<std::path::PathBuf>,
) -> Option<Arc<dyn agentic_core::subrun::SubrunRunner>> {
    if automation_files.is_empty() {
        return None;
    }
    if is_preview(platform.as_ref()) {
        tracing::info!(
            target: "preview",
            "no automation delegation for an agent in a workspace preview"
        );
        return None;
    }
    let workspace: Arc<dyn agentic_automation::WorkspaceContext> = platform.clone();
    let runner = agentic_automation::OxyAutomationRunner::new(workspace)
        .with_automation_files(automation_files);
    Some(Arc::new(runner))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::automation_subrun_runner;
    use crate::executor::preview_hold_tests::{PreviewAirwayPlatform, preview_scope};
    use crate::platform::PlatformContext;

    fn platform(preview: bool) -> Arc<dyn PlatformContext> {
        Arc::new(PreviewAirwayPlatform {
            scope: preview.then(preview_scope),
            request_preview: false,
            yaml_reads: Default::default(),
        })
    }

    /// A request pinned to a preview: no dry-run scope, still a preview.
    fn request_preview() -> Arc<dyn PlatformContext> {
        Arc::new(PreviewAirwayPlatform {
            scope: None,
            request_preview: true,
            yaml_reads: Default::default(),
        })
    }

    /// A preview-scoped analytics agent gets no automation runner, whatever
    /// its `context:` resolved; the same agent on a production platform does.
    #[test]
    fn a_preview_scoped_agent_gets_no_automation_runner() {
        let files = || vec![PathBuf::from("workflows/je.procedure.yml")];
        assert!(automation_subrun_runner(&platform(true), files()).is_none());
        assert!(
            automation_subrun_runner(&platform(false), files()).is_some(),
            "the control: production delegates"
        );
        assert!(automation_subrun_runner(&platform(false), vec![]).is_none());
    }

    /// Chat in a preview reads the branch, so a runner would delegate to the
    /// branch's automation and run its writes against production. It has no
    /// dry-run scope, and still gets no runner.
    #[test]
    fn a_chat_request_in_a_preview_gets_no_automation_runner() {
        let files = vec![PathBuf::from("workflows/je.procedure.yml")];
        assert!(automation_subrun_runner(&request_preview(), files).is_none());
    }

    /// No analytics path builds an automation runner except through the gate.
    #[test]
    fn no_runner_is_built_outside_the_gate() {
        let lib = include_str!("../lib.rs");
        assert!(
            !lib.contains("OxyAutomationRunner::new"),
            "lib.rs builds an automation runner directly; use `automation_subrun_runner`"
        );
        assert_eq!(
            lib.matches("automation_subrun_runner(").count(),
            3,
            "start_analytics, resume_analytics and run_agentic_headless each go through the gate"
        );
    }
}
