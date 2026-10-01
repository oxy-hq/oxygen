//! The workspace-preview platform: what a staff dry run of a procedure on a
//! previewed branch runs against.
//!
//! [`PreviewPlatformContext`] is a `PlatformContext` built for one root run of
//! `workspace_preview_runs`. It wraps an [`OxyProjectContext`] built from the
//! branch's **staging revision** and owns every side-effect decision (phase 2a:
//! hold every write):
//!
//! * **Reads answer from the staging revision, and only from it.** The inner
//!   manager is built at that revision over a working-copy path that never
//!   exists ([`NO_WORKING_COPY`]), so every "not compiled here → read the
//!   working copy" fallback in the manager finds nothing rather than reading
//!   someone's checkout. `workspace_path()` is `None` and every resolver runs
//!   under the staging pin (`custom_apps_staging_pin::with_staging_pin`: no
//!   rollups, partitioned caches).
//! * **Names are preview-scoped** (D5). The automation YAML it serves is
//!   re-emitted with each side-effecting name as `preview:<run_id>:<name>`
//!   ([`names`]), and it strips that prefix only for its own run. A pod
//!   without this code resolves none of them and fails the run.
//! * **Writes are held — except the managed Airhouse's, which land in the
//!   preview.** `review_sql` holds every statement that is not a read,
//!   `review_http` everything but `GET`/`HEAD`, and every connector it hands
//!   out is a `previews::hold::HoldingConnector` — on every database but the
//!   workspace's managed Airhouse (`airhouse_managed`). There (phase 2b S8,
//!   [`airhouse_writes`]) a step's writes are rewritten into the preview's own
//!   schemas, `preview_<key>__<live schema>`, registered and recorded before
//!   anything is sent, and sent through the preview Airhouse connector
//!   (`agentic_wiring::preview_airhouse`), which every Airhouse read of the run
//!   goes through too, so reads see the preview's copies. An Airhouse that
//!   cannot confine a Writer to those schemas (older than 0.1.49) holds the
//!   write as before; it is never sent live.
//! * **Nothing production-owned is reached.** `resolve_connector` is `None` (a
//!   bare config would be built into an unwrapped connector), secrets never
//!   persist, production's QuickBooks token vars resolve to nothing, and the
//!   rollup, metric-tree, anomaly and compile ports are all off.
//! * **An Airway sample's platform** (phase 2b S11, [`sample`]) additionally
//!   resolves its pipeline's destination into the preview's own schemas on a
//!   confined Writer, and may write exactly one secret: the rotating var of
//!   the sandbox company registered for the pipeline.
//!
//! **Every trait method is stated** (`tests::every_trait_method_is_overridden`
//! is a source scan): a defaulted method on a wrapper is a method that answers
//! without the wrapper having decided anything.

mod airhouse;
mod airhouse_batches;
mod airhouse_copies;
mod airhouse_samples;
mod airhouse_writes;
mod names;
mod project;
mod sample;
mod sample_secrets;
mod secrets;
mod workspace;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use agentic_pipeline::platform::{AirwayStepMode, PreviewScope};
use oxy::adapters::secrets::SecretsManager;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::OxyProjectContext;
use super::preview_airhouse::{PreviewAirhousePorts, WorkspaceAirhouse};
use crate::server::service::secret_manager::SecretManagerService;
pub(crate) use airhouse_writes::PreviewAirhouse;
pub(crate) use sample::{SAMPLE_KIND, SampleSide};

/// The working-copy root the inner manager is built over. It never exists, so
/// a compiled miss falls through to nothing instead of to a checkout.
pub(crate) const NO_WORKING_COPY: &str = "/nonexistent/oxy-workspace-preview";

/// Why a preview platform could not be built for a run.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    /// Could not look (a database blip). The next tick may do better.
    #[error("preview platform unavailable: {0}")]
    Unavailable(String),
    /// This run can never be driven as a preview — its revision is gone, is not
    /// this workspace's, or its config does not load. Fail the run.
    #[error("preview platform unusable: {0}")]
    Unusable(String),
}

impl PreviewError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

type ConnectorCells =
    tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::OnceCell<Arc<dyn DatabaseConnector>>>>>;

pub struct PreviewPlatformContext {
    inner: OxyProjectContext,
    scope: PreviewScope,
    workspace_id: Uuid,
    /// Secret names that must never resolve here: production's QuickBooks
    /// token and client-secret vars.
    withheld_secrets: HashSet<String>,
    /// Connectors by unscoped database name, built once per run.
    connectors: ConnectorCells,
    /// The managed Airhouse's preview side. `None` only for a platform built
    /// from parts without one: its Airhouse writes are held as in phase 2a.
    airhouse: Option<PreviewAirhouse>,
    /// An Airway sample's destination and sandbox ([`sample`]); `None` on
    /// every other run's platform.
    sample: Option<SampleSide>,
}

impl PreviewPlatformContext {
    /// The platform for `row`, a `workspace_preview_runs` row, on the
    /// workspace's own Airhouse.
    pub async fn new(
        db: &DatabaseConnection,
        row: &entity::workspace_preview_runs::Model,
    ) -> Result<Self, PreviewError> {
        Self::new_with(db, row, WorkspaceAirhouse::shared()).await
    }

    /// [`Self::new`] with the preview's Airhouse writes going to `airhouse`.
    pub async fn new_with(
        db: &DatabaseConnection,
        row: &entity::workspace_preview_runs::Model,
        airhouse: Arc<dyn PreviewAirhousePorts>,
    ) -> Result<Self, PreviewError> {
        let withheld = secrets::production_token_vars(db, row.workspace_id, row.revision_id)
            .await
            .map_err(|e| PreviewError::Unavailable(format!("reading QuickBooks vars: {e}")))?;
        let manager = build_manager(db, row, withheld.clone()).await?;
        let airhouse = PreviewAirhouse::open(db, row, airhouse).await?;
        let sample = SampleSide::for_row(db, row).await?;
        Ok(Self::from_parts(
            OxyProjectContext::new(manager),
            PreviewScope {
                run_id: row.run_id.clone(),
                preview_key: row.preview_key.clone(),
                revision_id: row.revision_id,
                airway_steps: AirwayStepMode::Hold,
            },
            withheld,
        )
        .with_airhouse(airhouse)
        .with_sample(sample))
    }

    /// Assemble from parts — `new` and the tests. No Airhouse side: add one
    /// with [`Self::with_airhouse`].
    pub(crate) fn from_parts(
        inner: OxyProjectContext,
        scope: PreviewScope,
        withheld_secrets: HashSet<String>,
    ) -> Self {
        Self {
            workspace_id: inner.workspace_manager().workspace_id,
            inner,
            scope,
            withheld_secrets,
            connectors: tokio::sync::Mutex::new(HashMap::new()),
            airhouse: None,
            sample: None,
        }
    }

    pub(crate) fn with_airhouse(mut self, airhouse: PreviewAirhouse) -> Self {
        self.airhouse = Some(airhouse);
        self
    }

    /// Make this an Airway sample's platform ([`sample`]).
    pub(crate) fn with_sample(mut self, sample: Option<SampleSide>) -> Self {
        self.sample = sample;
        self
    }

    /// The sample this platform runs, when it runs one.
    pub(crate) fn sample(&self) -> Option<&SampleSide> {
        self.sample.as_ref()
    }

    pub fn scope(&self) -> &PreviewScope {
        &self.scope
    }

    /// Run `fut` pinned to the staging revision.
    ///
    /// `fut` goes on the heap before the two task-local scopes wrap it: each
    /// scope holds its future by value, and a debug build's poll frames copy
    /// it at every layer — one resolver call overflowed a 2 MiB test thread
    /// on Linux. Boxed, each layer moves a pointer.
    fn pinned<F: std::future::Future>(
        &self,
        fut: F,
    ) -> impl std::future::Future<Output = F::Output> {
        crate::server::api::custom_apps_staging_pin::with_staging_pin(
            Some(self.scope.revision_id),
            Box::pin(fut),
        )
    }

    /// `name` with this run's prefix stripped. Unscoped names pass (a semantic
    /// view's `datasource:` is one); a name scoped to another run is refused —
    /// one preview run never reaches another's.
    fn own_name<'a>(&self, name: &'a str) -> Result<&'a str, String> {
        use agentic_automation::preview_names::{is_scoped, unscope};
        if !is_scoped(name) {
            return Ok(name);
        }
        unscope(name, &self.scope.run_id).ok_or_else(|| {
            format!(
                "`{name}` is scoped to another preview run, not {}",
                self.scope.run_id
            )
        })
    }
}

/// A manager at the staging revision, over [`NO_WORKING_COPY`].
///
/// The config is read and parsed here rather than by the builder, whose own
/// load falls back to the working copy — and to an EMPTY config — on any miss,
/// which would make a missing staging config look like a workspace with no
/// databases.
///
/// The manager's secrets withhold `withheld` (production's QuickBooks
/// credentials): a branch's `config.yml` could otherwise name one as a
/// database `password_var` or a model `key_var` beside a host it controls.
async fn build_manager(
    db: &DatabaseConnection,
    row: &entity::workspace_preview_runs::Model,
    withheld: HashSet<String>,
) -> Result<WorkspaceManager<WorkingCopy>, PreviewError> {
    use PreviewError::{Unavailable, Unusable};
    let (workspace_id, revision_id) = (row.workspace_id, row.revision_id);
    let rev = entity::revisions::Entity::find_by_id(revision_id)
        .one(db)
        .await
        .map_err(|e| Unavailable(e.to_string()))?
        .ok_or_else(|| Unusable(format!("staging revision {revision_id} no longer exists")))?;
    if rev.workspace_id != workspace_id {
        return Err(Unusable(format!(
            "revision {revision_id} is not workspace {workspace_id}'s"
        )));
    }
    let value = crate::server::api::compiled_reader::resolve_workspace_config_at(revision_id)
        .await
        .map_err(|e| Unavailable(e.to_string()))?
        .ok_or_else(|| Unusable(format!("revision {revision_id} has no compiled config")))?;
    let config: oxy::config::model::Config = serde_json::from_value(value).map_err(|e| {
        Unusable(format!(
            "revision {revision_id}'s config does not load: {e}"
        ))
    })?;
    WorkspaceBuilder::new(workspace_id)
        .with_working_copy_and_provided_config(NO_WORKING_COPY, config, revision_id)
        .map_err(|e| Unusable(e.to_string()))?
        .with_secrets_manager(preview_secrets(workspace_id, withheld)?)
        .build()
        .await
        .map_err(|e| Unavailable(e.to_string()))
}

/// The workspace's own DB-first secrets, as the background driver's contexts
/// get them — always withholding `withheld`, whichever store answers.
fn preview_secrets(
    workspace_id: Uuid,
    withheld: HashSet<String>,
) -> Result<SecretsManager, PreviewError> {
    let secrets =
        SecretsManager::from_database_with_env_fallback(SecretManagerService::new(workspace_id))
            .or_else(|e| {
                tracing::warn!(target: "preview", %workspace_id, error = %e,
                    "preview platform: no DB secrets manager; secrets resolve from the environment only");
                SecretsManager::from_environment()
            })
            .map_err(|e| PreviewError::Unavailable(format!("secrets manager: {e}")))?;
    Ok(secrets.withholding(withheld))
}
