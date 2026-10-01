//! Which platform the global-run driver drives a root run with (D4).
//!
//! [`PreviewRunResolver`] is the host's `RunPlatformResolver`: a root whose id
//! is a `workspace_preview_runs` row gets a [`PreviewPlatformContext`] over that
//! row's staging revision; every other root gets the base platform. Only
//! preview-owned runs ever read a staging revision (I7).
//!
//! * A **lookup error** is `Err`: the root is not driven this tick, so a blip can
//!   never drive a preview run as production.
//! * A platform that **cannot be built for good** (revision gone, config that
//!   will not load) retires the run with the reason, then answers `Err` — a
//!   run that can never be driven must not be re-selected every tick.
//! * `analyze` rows are the Airway change check, a `TaskSpec::Custom` that reads
//!   through its own Reader connection and never through the platform: base.
//! * A root with **no registry row that says it is a preview** anyway
//!   (`metadata.trigger = "preview"`, or a `preview:`-scoped `workflow_ref`)
//!   is retired, never driven with the base: the registry and the run row
//!   disagreeing is not a reason to run a branch's procedure as production.

use std::sync::Arc;

use agentic_pipeline::platform::{PlatformContext, RunPlatformResolver};
use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait};

use crate::agentic_wiring::preview_airhouse::{PreviewAirhousePorts, WorkspaceAirhouse};
use crate::agentic_wiring::preview_ctx::PreviewPlatformContext;

/// The kinds whose runs are driven on the base platform: the change check and
/// the compare are `TaskSpec::Custom` work that reads through its own
/// connection, never through a platform; an Airway sample is `TaskSpec::Custom`
/// work that builds its own preview platform from its row
/// (`previews::sample`), so the driver's platform never reaches it.
const BASE_PLATFORM_KINDS: &[&str] = &["analyze", "compare", "airway_sample"];

pub struct PreviewRunResolver {
    db: DatabaseConnection,
    /// Where preview runs' managed-Airhouse writes go.
    airhouse: Arc<dyn PreviewAirhousePorts>,
}

impl PreviewRunResolver {
    /// On the workspace's own Airhouse.
    pub fn new(db: DatabaseConnection) -> Self {
        Self::with_airhouse(db, WorkspaceAirhouse::shared())
    }

    /// On `airhouse` instead (an in-process stand-in, in tests).
    pub fn with_airhouse(db: DatabaseConnection, airhouse: Arc<dyn PreviewAirhousePorts>) -> Self {
        Self { db, airhouse }
    }

    pub fn shared(db: &DatabaseConnection) -> Arc<dyn RunPlatformResolver> {
        Arc::new(Self::new(db.clone()))
    }
}

#[async_trait]
impl RunPlatformResolver for PreviewRunResolver {
    async fn platform_for(
        &self,
        root: &agentic_runtime::entity::run::Model,
        base: Arc<dyn PlatformContext>,
    ) -> Result<Arc<dyn PlatformContext>, String> {
        let row = entity::workspace_preview_runs::Entity::find_by_id(root.id.clone())
            .one(&self.db)
            .await
            .map_err(|e| format!("preview registry lookup: {e}"))?;
        let Some(row) = row else {
            if let Some(why) = preview_marked(root) {
                return Err(self.retire(root, why).await);
            }
            return Ok(base);
        };
        if BASE_PLATFORM_KINDS.contains(&row.kind.as_str()) {
            return Ok(base);
        }
        if row.workspace_id != root.workspace_id {
            return Err(self
                .retire(
                    root,
                    "the preview run and its agentic run name different workspaces",
                )
                .await);
        }
        match PreviewPlatformContext::new_with(&self.db, &row, Arc::clone(&self.airhouse)).await {
            Ok(platform) => Ok(Arc::new(platform)),
            Err(e) if e.is_transient() => Err(e.to_string()),
            Err(e) => Err(self.retire(root, &e.to_string()).await),
        }
    }
}

/// Why a run with no registry row still reads as a preview's, if it does.
pub(crate) fn preview_marked(root: &agentic_runtime::entity::run::Model) -> Option<&'static str> {
    let metadata = root.metadata.as_ref()?;
    if metadata.get("trigger").and_then(|t| t.as_str()) == Some("preview") {
        return Some("a preview run with no workspace_preview_runs row is never driven");
    }
    let workflow_ref = metadata.get("workflow_ref").and_then(|r| r.as_str())?;
    agentic_automation::preview_names::is_scoped(workflow_ref)
        .then_some("a preview-scoped run with no workspace_preview_runs row is never driven")
}

impl PreviewRunResolver {
    /// Fail a run that can never be driven as a preview, and say why. The
    /// registry row is finished by the next sweep (`runs::mark_finished`).
    async fn retire(&self, root: &agentic_runtime::entity::run::Model, reason: &str) -> String {
        tracing::warn!(target: "preview", run_id = %root.id, reason, "retiring a preview run");
        if let Err(e) = agentic_runtime::crud::retire_run(&self.db, &root.id, reason).await {
            tracing::warn!(target: "preview", run_id = %root.id, error = %e,
                "could not retire the preview run; it stays undriven");
        }
        reason.to_string()
    }
}
