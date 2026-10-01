//! An Airway sample's side of the preview platform (phase 2b S11).
//!
//! Every preview platform holds airway steps and resolves no pipeline
//! destination. A platform built for a sample run (`workspace_preview_runs`
//! kind `airway_sample`) differs in three places:
//!
//! * **its destination** is the preview's own schemas on the managed Airhouse
//!   (`airhouse_samples`); any other database is refused;
//! * **for a rotate-on-use source (QuickBooks) only the registered sandbox's
//!   secrets resolve** — an allowlist of the var names staff registered for the
//!   pipeline (`workspace_preview_sources`); every other name answers nothing,
//!   whatever it is (production's, an app-scoped `apps/<id>/…` token, …);
//! * **one secret may be written**: the sandbox's rotating var, updated in place
//!   and never created ([`super::sample_secrets`]), so the sampler is that
//!   sandbox grant's only rotator. Production's QuickBooks vars stay withheld —
//!   never resolved, never written — whatever the registry says.
//!
//! The registered sandbox is checked again here as the platform is built
//! (`previews::sources::check_sandbox`): a row naming an app-scoped or
//! app-declared var, or production's grant, makes the platform unusable.
//!
//! Why a destination was refused is kept ([`SampleSide::refusal`]) so the
//! sample's outcome can say it: the pipeline port only answers "none".

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use agentic_pipeline::airway_preview::{SandboxSource, rotates_on_use};
use agentic_pipeline::platform::ResolvedPipelineDestination;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use super::{PreviewError, PreviewPlatformContext};
use crate::server::previews::sources::{SourceRequestError, check_sandbox};

/// The preview run kind a sample's platform is built for.
pub(crate) const SAMPLE_KIND: &str = "airway_sample";

/// Where the sandbox's rotated token is written back: the workspace's own
/// secrets table, attributed to the staffer who started the sample.
pub(crate) struct TokenStore {
    pub(super) db: DatabaseConnection,
    pub(super) updated_by: Uuid,
}

pub(crate) struct SampleSide {
    sandbox: Option<SandboxSource>,
    /// For a rotate-on-use source: the only secret names that resolve.
    allowlist: Option<HashSet<String>>,
    pub(super) store: Option<TokenStore>,
    refusal: Mutex<Option<String>>,
}

impl SampleSide {
    /// A sample of a source that rotates on use (`rotate_on_use`) resolves only
    /// `sandbox`'s var names — none at all without a sandbox.
    pub(crate) fn new(sandbox: Option<SandboxSource>, rotate_on_use: bool) -> Self {
        let allowlist = rotate_on_use.then(|| {
            sandbox
                .iter()
                .flat_map(SandboxSource::var_names)
                .map(str::to_string)
                .collect()
        });
        Self {
            sandbox,
            allowlist,
            store: None,
            refusal: Mutex::new(None),
        }
    }

    pub(crate) fn with_store(mut self, db: DatabaseConnection, updated_by: Uuid) -> Self {
        self.store = Some(TokenStore { db, updated_by });
        self
    }

    /// The side for run `row`: `None` unless it is a sample; the pipeline's
    /// registered sandbox (checked again) when there is one.
    pub(super) async fn for_row(
        db: &DatabaseConnection,
        row: &entity::workspace_preview_runs::Model,
    ) -> Result<Option<Self>, PreviewError> {
        if row.kind != SAMPLE_KIND {
            return Ok(None);
        }
        let pipeline = row
            .options
            .get("pipeline_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PreviewError::Unusable("the sample names no pipeline".into()))?;
        let kind = source_kind(db, row).await?;
        let sandbox = registered_sandbox(db, row.workspace_id, pipeline).await?;
        let updated_by = row.requested_by.unwrap_or_else(Uuid::nil);
        Ok(Some(
            Self::new(sandbox, rotates_on_use(&kind)).with_store(db.clone(), updated_by),
        ))
    }

    pub(crate) fn sandbox(&self) -> Option<&SandboxSource> {
        self.sandbox.as_ref()
    }

    /// `Some(false)` when `var` is outside a rotate-on-use sample's allowlist;
    /// `None` when this sample has none.
    pub(super) fn allows(&self, var: &str) -> Option<bool> {
        self.allowlist.as_ref().map(|allowed| allowed.contains(var))
    }

    /// Why the sample's destination was refused, if it was.
    pub(crate) fn refusal(&self) -> Option<String> {
        self.refusal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn refuse(&self, why: String) {
        *self.refusal.lock().unwrap_or_else(PoisonError::into_inner) = Some(why);
    }
}

/// The sampled pipeline's source kind, as the staging revision compiled it.
async fn source_kind(
    db: &DatabaseConnection,
    row: &entity::workspace_preview_runs::Model,
) -> Result<String, PreviewError> {
    let found = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT definition->'source'->>'kind' AS kind FROM airway_pipelines \
             WHERE revision_id = $1 AND file_path = $2",
            [
                row.revision_id.into(),
                row.target_ref.clone().unwrap_or_default().into(),
            ],
        ))
        .await
        .map_err(|e| PreviewError::Unavailable(format!("reading the sampled pipeline: {e}")))?
        .ok_or_else(|| {
            PreviewError::Unusable("the sampled pipeline is not in the revision".into())
        })?;
    found
        .try_get::<Option<String>>("", "kind")
        .map_err(|e| PreviewError::Unavailable(e.to_string()))?
        .ok_or_else(|| PreviewError::Unusable("the sampled pipeline names no source kind".into()))
}

/// `workspace_preview_sources.overrides` for `pipeline`, parsed and checked
/// as the save checked it — against today's production and apps.
async fn registered_sandbox(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline: &str,
) -> Result<Option<SandboxSource>, PreviewError> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT overrides FROM workspace_preview_sources \
             WHERE workspace_id = $1 AND pipeline_name = $2 AND environment = 'sandbox'",
            [workspace_id.into(), pipeline.into()],
        ))
        .await
        .map_err(|e| PreviewError::Unavailable(format!("reading the sandbox source: {e}")))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let overrides: serde_json::Value = row
        .try_get("", "overrides")
        .map_err(|e| PreviewError::Unavailable(e.to_string()))?;
    let sandbox: SandboxSource = serde_json::from_value(overrides)
        .map_err(|e| PreviewError::Unusable(format!("the registered sandbox source: {e}")))?;
    match check_sandbox(db, workspace_id, &sandbox).await {
        Ok(()) => Ok(Some(sandbox)),
        Err(SourceRequestError::Internal(e)) => Err(PreviewError::Unavailable(e)),
        Err(refused) => Err(PreviewError::Unusable(format!(
            "the registered sandbox source is refused: {refused}"
        ))),
    }
}

impl PreviewPlatformContext {
    /// A sample's destination (module doc); `None` on every other platform,
    /// and for a sample whose destination is refused (why is kept).
    pub(super) async fn sample_destination(
        &self,
        db_name: &str,
        dataset: &str,
    ) -> Option<ResolvedPipelineDestination> {
        let side = self.sample.as_ref()?;
        match self.try_sample_destination(db_name, dataset).await {
            Ok(resolved) => Some(resolved),
            Err(why) => {
                tracing::warn!(target: "preview", db = %db_name, dataset, reason = %why,
                    "an Airway sample's destination was refused");
                side.refuse(why);
                None
            }
        }
    }

    async fn try_sample_destination(
        &self,
        db_name: &str,
        dataset: &str,
    ) -> Result<ResolvedPipelineDestination, String> {
        let name = self.own_name(db_name)?;
        let airhouse = self.preview_airhouse(name)?.ok_or_else(|| {
            format!(
                "an Airway sample lands only in the workspace's managed Airhouse, and `{name}` \
                 is not it; nothing was written"
            )
        })?;
        airhouse.pipeline_destination(dataset).await
    }
}
