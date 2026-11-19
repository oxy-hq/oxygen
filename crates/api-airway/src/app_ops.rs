//! `ctx.oxygen.sourceUploads.report`'s work, after the function host decided
//! the call (`oxy-api-custom-apps`, `host/app_access_op.rs`). `oxy-server`
//! hands these to the function runtime through
//! `oxy_app_core::app_platform_seam`.
//!
//! A function names a pipeline, never a workspace: the manifest lists pipeline
//! names, an org admin approves those names, and [`resolve`] finds which
//! workspace of the org holds the pipeline. The upload route still files the
//! report under that workspace, so the name has to pick exactly one.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use entity::{airway_pipelines, workspaces};
use oxy_app_core::app_platform_seam::{PlatformActionError, ResolvedPipeline, SourceReport};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use crate::source_upload::{ReportFields, store_report_from};

/// The one compiled pipeline named `name` in `org_id`: looked for in the
/// promoted revision (`current_revision_id`) of each of the org's workspaces.
/// A workspace with nothing promoted holds no pipeline here.
///
/// Read off the rows, not through the request path's revision resolver: that
/// trusts a revision pinned for the current request whatever workspace it is
/// asked about, and this asks about every workspace of the org. Nor through a
/// `ConfigManager`, which answers for one workspace; this searches them all by
/// name, reading only `current_revision_id` and the compiled pipeline rows.
///
/// `NotFound` when no workspace holds it; `Invalid` when more than one
/// pipeline has the name, because then the name does not pick a landing zone.
pub async fn resolve(
    db: &DatabaseConnection,
    org_id: Uuid,
    name: &str,
) -> Result<ResolvedPipeline, PlatformActionError> {
    let in_org = workspaces::Entity::find()
        .filter(workspaces::Column::OrgId.eq(org_id))
        .all(db)
        .await
        .map_err(unavailable("the organization's workspaces"))?;
    // Revision → the workspace it is promoted in.
    let mut promoted: BTreeMap<Uuid, Uuid> = BTreeMap::new();
    for workspace in &in_org {
        if let Some(revision) = workspace.current_revision_id {
            promoted.insert(revision, workspace.id);
        }
    }
    let rows = if promoted.is_empty() {
        Vec::new()
    } else {
        airway_pipelines::Entity::find()
            .filter(airway_pipelines::Column::RevisionId.is_in(promoted.keys().copied()))
            .filter(airway_pipelines::Column::Name.eq(name))
            .all(db)
            .await
            .map_err(unavailable("compiled pipelines"))?
            .into_iter()
            .map(|row| (row.revision_id, row.file_path, row.definition))
            .collect()
    };
    pick(name, &promoted, rows)
}

/// One compiled pipeline row: its revision, file path and definition.
pub(crate) type PipelineRow = (Uuid, String, Value);

/// The resolution itself, apart from the queries: exactly one row in a
/// promoted revision, or why not. A revision's names are unique (the table's
/// key), so two rows mean two workspaces.
pub(crate) fn pick(
    name: &str,
    promoted: &BTreeMap<Uuid, Uuid>,
    rows: Vec<PipelineRow>,
) -> Result<ResolvedPipeline, PlatformActionError> {
    let mut found = rows
        .into_iter()
        .filter_map(|(revision, file_path, definition)| {
            promoted
                .get(&revision)
                .map(|ws| (*ws, file_path, definition))
        });
    let Some((workspace_id, file_path, definition)) = found.next() else {
        return Err(PlatformActionError::NotFound(format!(
            "no compiled pipeline in this organization is named `{name}`. It may not exist, \
             or it was added after the last compile; a compile that promotes it makes it \
             resolvable."
        )));
    };
    if found.next().is_some() {
        return Err(PlatformActionError::Invalid(format!(
            "more than one pipeline in this organization is named `{name}`, so the name \
             does not pick one landing zone. Rename one of them."
        )));
    }
    Ok(ResolvedPipeline {
        workspace_id,
        file_path,
        definition,
    })
}

/// Put `report` in `pipeline`'s landing zone, with every check the upload
/// route makes. The route's words come back: a client error is the report's
/// fault and the function may show it; anything else is the platform's.
pub async fn report(
    pipeline: ResolvedPipeline,
    report: SourceReport,
) -> Result<Value, PlatformActionError> {
    let fields = ReportFields {
        bytes: Some(report.file),
        pipeline_ref: Some(pipeline.file_path),
        filename: Some(report.file_name),
        workflow_id: Some(report.workflow_id),
        year: report.period.map(|(year, _)| year),
        month: report.period.map(|(_, month)| month),
    };
    match store_report_from(pipeline.definition, pipeline.workspace_id, fields).await {
        Ok(uploaded) => serde_json::to_value(uploaded).map_err(|e| {
            tracing::error!(error = %e, "an uploaded report's reply did not serialize");
            PlatformActionError::Unavailable("the upload's reply could not be read".into())
        }),
        Err((status, message)) if status.is_client_error() => {
            Err(PlatformActionError::Invalid(message))
        }
        // `503` is the route's "not in the compiled workspace" and its
        // validation outage alike: worth a retry either way.
        Err((status, message)) => {
            if status != StatusCode::SERVICE_UNAVAILABLE {
                tracing::error!(%status, error = %message, "an app's report upload failed");
            }
            Err(PlatformActionError::Unavailable(message))
        }
    }
}

fn unavailable(what: &'static str) -> impl Fn(sea_orm::DbErr) -> PlatformActionError {
    move |e| {
        tracing::error!(error = %e, "{what} could not be read");
        PlatformActionError::Unavailable(format!("{what} could not be read just now"))
    }
}

#[cfg(test)]
#[path = "app_ops_tests.rs"]
mod tests;
