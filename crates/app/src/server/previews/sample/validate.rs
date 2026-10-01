//! Submit-time validation of a sample (P4 step 2): the host half of
//! `agentic_airway::preview::SamplePolicy`. The branch's pipeline is read from
//! the staging revision, its resources counted with a connector built offline
//! (placeholder credentials, as the change check does — nothing is called),
//! and the answer stored with the run as [`SampleOptions`].

use agentic_airway::preview::{
    RequestedWindow, SampleAsk, SamplePolicy, SampleRefusal, SampleWindow, metadata_in_main,
    rotates_on_use,
};
use agentic_airway::schema_compat::Schema;
use agentic_airway::{AirwayPipelineSpec, DestinationSpec};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::server::previews::runs::RunRequestError;

/// What a queued sample runs, as stored in `workspace_preview_runs.options`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleOptions {
    /// The pipeline's own name (its YAML `name:`), which production's rows
    /// are keyed by and its sandbox source is registered under.
    pub pipeline_name: String,
    /// The live dataset the destination writes; its preview schema is where
    /// the sample lands. `None` for an inline (memory) destination.
    pub dataset_name: Option<String>,
    pub window: Option<SampleWindow>,
    #[serde(default)]
    pub resources: Vec<String>,
    /// No window bounds it: the wall-clock cap does.
    pub wall_clock_capped: bool,
}

impl SampleOptions {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_json(options: &Value) -> Result<Self, String> {
        serde_json::from_value(options.clone()).map_err(|e| format!("sample options: {e}"))
    }
}

/// Validate a sample of `target_ref` in the staging revision (module doc).
pub async fn validate(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    revision_id: Uuid,
    target_ref: &str,
    window: Option<RequestedWindow>,
    resources: &[String],
) -> Result<SampleOptions, RunRequestError> {
    let spec = staged_pipeline(db, revision_id, target_ref).await?;
    let has_sandbox =
        rotates_on_use(&spec.source.kind) && has_sandbox(db, workspace_id, &spec.name).await?;
    let managed = match spec.destination.database_ref() {
        Some(database) => is_managed_airhouse(db, revision_id, database).await?,
        None => true,
    };
    let stored = super::record::stored_schema(db, workspace_id, &spec.name)
        .await
        .map_err(RunRequestError::Internal)?;
    let asked = Asked {
        window,
        resources,
        has_sandbox,
    };
    let options =
        check(&spec, &asked, stored.as_ref(), Utc::now()).map_err(RunRequestError::Sample)?;
    if !managed {
        let database = spec.destination.database_ref().unwrap_or_default();
        return Err(RunRequestError::Sample(SampleRefusal::NotManagedAirhouse {
            database: database.to_string(),
        }));
    }
    Ok(options)
}

/// What a sample request asks for.
pub struct Asked<'a> {
    pub window: Option<RequestedWindow>,
    pub resources: &'a [String],
    /// A sandbox company is registered for the pipeline.
    pub has_sandbox: bool,
}

/// The pure rules, over a parsed spec and production's stored schema (`None`
/// when it never loaded): `SamplePolicy` with the resources the source
/// advertises; then no table whose load writes airway's metadata into `main`
/// (`metadata_in_main`, `422 sample_unsupported`); and no inline destination
/// but the test `memory` one.
pub fn check(
    spec: &AirwayPipelineSpec,
    asked: &Asked<'_>,
    stored: Option<&Schema>,
    now: DateTime<Utc>,
) -> Result<SampleOptions, SampleRefusal> {
    let infos = crate::server::previews::analyze::offline_connector(spec)
        .ok()
        .map(|c| c.resources());
    let advertised = if spec.resources.is_empty() {
        infos
            .as_ref()
            .map(|i| i.iter().map(|r| r.name.clone()).collect())
    } else {
        Some(spec.resources.clone())
    };
    let plan = SamplePolicy::check(&SampleAsk {
        pipeline: &spec.name,
        kind: &spec.source.kind,
        window: asked.window,
        resources: asked.resources,
        advertised: advertised.as_deref(),
        has_sandbox: asked.has_sandbox,
        now,
    })?;
    let sampled = if plan.resources.is_empty() {
        &spec.resources
    } else {
        &plan.resources
    };
    let tables = metadata_in_main(infos.as_deref().unwrap_or_default(), stored, sampled);
    if !tables.is_empty() {
        return Err(SampleRefusal::Unsupported { tables });
    }
    let dataset_name = match &spec.destination {
        DestinationSpec::Reference(reference) => Some(reference.dataset_name.clone()),
        DestinationSpec::Inline(inline) if inline.kind == "memory" => None,
        DestinationSpec::Inline(inline) => {
            return Err(SampleRefusal::InlineDestination {
                kind: inline.kind.clone(),
            });
        }
    };
    Ok(SampleOptions {
        pipeline_name: spec.name.clone(),
        dataset_name,
        window: plan.window,
        resources: plan.resources,
        wall_clock_capped: plan.wall_clock_capped,
    })
}

/// The branch's pipeline at `target_ref`, parsed.
async fn staged_pipeline(
    db: &DatabaseConnection,
    revision_id: Uuid,
    target_ref: &str,
) -> Result<AirwayPipelineSpec, RunRequestError> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT definition FROM airway_pipelines WHERE revision_id = $1 AND file_path = $2",
            [revision_id.into(), target_ref.into()],
        ))
        .await?
        .ok_or_else(|| RunRequestError::RefNotInRevision(target_ref.to_string()))?;
    let definition: Value = row.try_get("", "definition")?;
    let spec: AirwayPipelineSpec = serde_json::from_value(definition)
        .map_err(|e| RunRequestError::BadRequest(format!("{target_ref}: {e}")))?;
    spec.validate()
        .map_err(|e| RunRequestError::BadRequest(format!("{target_ref}: {e}")))?;
    Ok(spec)
}

async fn has_sandbox(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline: &str,
) -> Result<bool, RunRequestError> {
    let found = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS found FROM workspace_preview_sources \
             WHERE workspace_id = $1 AND pipeline_name = $2 AND environment = 'sandbox'",
            [workspace_id.into(), pipeline.into()],
        ))
        .await?;
    Ok(found.is_some())
}

/// `database` is an `airhouse_managed` database in the revision's config.
async fn is_managed_airhouse(
    db: &DatabaseConnection,
    revision_id: Uuid,
    database: &str,
) -> Result<bool, RunRequestError> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT EXISTS (SELECT 1 FROM workspace_compiled_configs c, \
                 jsonb_array_elements(c.databases) d \
               WHERE c.revision_id = $1 AND d->>'name' = $2 \
                 AND d->>'type' = 'airhouse_managed') AS managed",
            [revision_id.into(), database.into()],
        ))
        .await?;
    Ok(match row {
        Some(r) => r.try_get("", "managed")?,
        None => false,
    })
}
