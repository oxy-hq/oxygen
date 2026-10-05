//! Who a custom-app ask runs as when something other than its request drives it.
//!
//! The handler drives an ask on the context the gate built: the caller's id as
//! its subject, no role, at the request's staging pin. Recovery does not have
//! that context. It drives a root on the platform its tick built for the
//! workspace, which carries **no subject** — and `airhouse_managed` mints a
//! *system Admin* for a subject-less context, where a subject with no role
//! mints the caller's *Reader* (`project_ctx::mint_airhouse_managed_creds`).
//! So an ask picked up after a restart — its parent resumed, its delegated
//! automation steps re-run — changed credential, upwards, with nothing saying
//! so.
//!
//! Two halves close that:
//!
//! * [`RunCaller`] is written into the run's `metadata` by the insert that
//!   creates the run (`PipelineBuilder::run_metadata`), from the gate's own
//!   context.
//! * [`CallerRunResolver`] is the host's `RunPlatformResolver` for every
//!   recovery entry point. A root carrying the record is driven on a context
//!   rebuilt through `custom_apps_gates::build_caller_context` — the function
//!   the handler's own context goes through — and never on the tick's.
//!
//! This is the identity half of moving asks onto the task queue
//! (`internal-docs/worker-fleet.md` § "Custom-app runs on the queue"): a queued
//! ask is driven by the same three entry points, so it inherits this as is.

use std::sync::Arc;

use agentic_pipeline::platform::{PlatformContext, RunPlatformResolver};
use async_trait::async_trait;
use entity::prelude::Workspaces;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::agentic_wiring::OxyProjectContext;
use crate::server::api::custom_apps_gates::{CustomAppContext, build_caller_context};
use crate::server::previews::runtime::PreviewRunResolver;

/// The key the record sits under in `agentic_runs.metadata`.
pub const RUN_CALLER_KEY: &str = "custom_app_caller";

/// The identity a custom-app ask was started with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCaller {
    /// The authenticated caller: the subject of the run's context.
    pub user_id: Uuid,
    /// The staging revision the request was pinned to. `None` for every live
    /// request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_pin: Option<Uuid>,
}

/// Why a recorded caller's context could not be produced. Either way the root
/// is not driven: never on the tick's platform instead.
#[derive(Debug, Error)]
pub enum CallerContextError {
    #[error("the run's `{RUN_CALLER_KEY}` record is unreadable: {0}")]
    Unreadable(String),
    #[error("workspace lookup failed: {0}")]
    Lookup(String),
    #[error("the run's workspace no longer exists")]
    WorkspaceGone,
    #[error("could not build the caller's workspace context (status {0})")]
    Build(u16),
}

impl RunCaller {
    /// The record for a request that has passed the gate. Read from the gate's
    /// own context, in this one place, so a caller cannot be recorded as a
    /// user or at a pin the gate did not establish.
    pub fn of(gate: &CustomAppContext) -> Self {
        Self {
            user_id: gate.user.id,
            staging_pin: gate.staging_pin,
        }
    }

    /// The value stored under [`RUN_CALLER_KEY`].
    pub fn to_metadata(&self) -> serde_json::Value {
        serde_json::json!(self)
    }

    /// The record a run carries. `Ok(None)` when it carries none — every run
    /// that is not a custom-app ask, and an ask started before this existed.
    /// A key that is present but does not read back is an error, not an
    /// absence: treating it as "no record" would drive the run as the tick.
    pub fn recorded(
        metadata: Option<&serde_json::Value>,
    ) -> Result<Option<Self>, CallerContextError> {
        let Some(value) = metadata.and_then(|m| m.get(RUN_CALLER_KEY)) else {
            return Ok(None);
        };
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|e| CallerContextError::Unreadable(e.to_string()))
    }

    /// The context the handler had: this caller as subject, no role, at the
    /// recorded pin. The workspace row is re-read rather than carried — it is
    /// what names the working copy and the promoted revision now, on this
    /// node.
    pub async fn context(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<OxyProjectContext, CallerContextError> {
        let workspace = Workspaces::find_by_id(workspace_id)
            .one(db)
            .await
            .map_err(|e| CallerContextError::Lookup(e.to_string()))?
            .ok_or(CallerContextError::WorkspaceGone)?;
        build_caller_context(&workspace, self.user_id, workspace_id, self.staging_pin)
            .await
            .map_err(|response| CallerContextError::Build(response.status().as_u16()))
    }
}

/// Drives a root that records its caller as that caller; hands every other
/// root to `inner`.
///
/// `inner` is asked first and a platform it substitutes is kept: a
/// preview-owned root is the preview resolver's to answer (and to retire), and
/// its platform is what holds that run's writes.
pub struct CallerRunResolver {
    db: DatabaseConnection,
    inner: Arc<dyn RunPlatformResolver>,
}

impl CallerRunResolver {
    pub fn new(db: DatabaseConnection, inner: Arc<dyn RunPlatformResolver>) -> Self {
        Self { db, inner }
    }

    /// The resolver every recovery entry point is handed
    /// (`router::recovery`): a preview-owned root on its preview platform, a
    /// custom-app ask as its caller, everything else on the tick's.
    pub fn shared(db: &DatabaseConnection) -> Arc<dyn RunPlatformResolver> {
        Arc::new(Self::new(db.clone(), PreviewRunResolver::shared(db)))
    }

    /// The context `root` is driven on when it records its caller; `None`
    /// when it records none. Typed, so a test can read the identity the
    /// resolver hands recovery rather than infer it.
    pub async fn caller_platform(
        &self,
        root: &agentic_runtime::entity::run::Model,
    ) -> Result<Option<OxyProjectContext>, CallerContextError> {
        let Some(caller) = RunCaller::recorded(root.metadata.as_ref())? else {
            return Ok(None);
        };
        caller.context(&self.db, root.workspace_id).await.map(Some)
    }
}

#[async_trait]
impl RunPlatformResolver for CallerRunResolver {
    async fn platform_for(
        &self,
        root: &agentic_runtime::entity::run::Model,
        base: Arc<dyn PlatformContext>,
    ) -> Result<Arc<dyn PlatformContext>, String> {
        let platform = self.inner.platform_for(root, base.clone()).await?;
        if !Arc::ptr_eq(&platform, &base) {
            return Ok(platform);
        }
        match self.caller_platform(root).await {
            Ok(Some(context)) => Ok(Arc::new(context)),
            Ok(None) => Ok(platform),
            // Not driven this tick — and never on `platform` instead.
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn caller(staging_pin: Option<Uuid>) -> RunCaller {
        RunCaller {
            user_id: Uuid::new_v4(),
            staging_pin,
        }
    }

    /// What the handler writes is what recovery reads, pin or no pin.
    #[test]
    fn a_record_reads_back_as_it_was_written() {
        for recorded in [caller(None), caller(Some(Uuid::new_v4()))] {
            let metadata = json!({
                "agent_id": "sales",
                RUN_CALLER_KEY: recorded.to_metadata(),
            });
            assert_eq!(
                RunCaller::recorded(Some(&metadata)).expect("readable"),
                Some(recorded)
            );
        }
        assert_eq!(
            caller(None).to_metadata().get("staging_pin"),
            None,
            "a live request records no pin at all"
        );
    }

    /// No record is the answer for every other run: chat, a schedule, an ask
    /// from before the record existed.
    #[test]
    fn a_run_with_no_record_has_no_caller() {
        assert!(matches!(RunCaller::recorded(None), Ok(None)));
        assert!(matches!(
            RunCaller::recorded(Some(&json!({ "agent_id": "sales" }))),
            Ok(None)
        ));
    }

    /// A record that is there and does not parse must not read as "none":
    /// that is the answer under which the run is driven as the tick.
    #[test]
    fn an_unreadable_record_is_an_error_not_an_absence() {
        for broken in [
            json!(null),
            json!("someone"),
            json!({ "user_id": "not-a-uuid" }),
            json!({ "staging_pin": Uuid::new_v4() }),
        ] {
            let metadata = json!({ RUN_CALLER_KEY: broken });
            assert!(
                matches!(
                    RunCaller::recorded(Some(&metadata)),
                    Err(CallerContextError::Unreadable(_))
                ),
                "{metadata}"
            );
        }
    }
}
