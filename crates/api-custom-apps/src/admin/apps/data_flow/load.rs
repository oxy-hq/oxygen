//! The reads behind one answer, all of them the platform DB and all scoped to
//! the one org: its workspaces, the pipelines compiled under each one's
//! promoted revision, its apps, the build each app serves, those builds'
//! manifests and functions, and the upload approvals that stand.

use std::collections::{BTreeSet, HashMap};

use entity::{
    airway_pipelines, app_builds, app_functions, app_platform_grants, apps, organizations,
    workspaces,
};
use oxy_authz::Action;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    QuerySelect,
};
use serde_json::Value;
use uuid::Uuid;

use super::assemble::{AppIn, FunctionIn, Inputs, PipelineIn, WorkspaceIn};
use super::{OrgRef, Params, not_found};
use crate::admin::apps::environment_scope::ScopeError;
use crate::custom_apps_env_resolve::load_environment_builds_batch;

fn failed(context: &'static str) -> impl FnOnce(DbErr) -> ScopeError {
    move |e| ScopeError::internal(context, e)
}

/// The org, and everything [`super::assemble::assemble`] needs for it.
pub(crate) struct Loaded {
    pub org: OrgRef,
    pub inputs: Inputs,
}

/// Everything one answer for `params` reads.
pub(crate) async fn inputs(db: &DatabaseConnection, params: &Params) -> Result<Loaded, ScopeError> {
    let org = organizations::Entity::find_by_id(params.org_id)
        .one(db)
        .await
        .map_err(failed("data-flow org lookup failed"))?
        .ok_or_else(|| not_found("org_not_found", "no such organization"))?;
    let workspaces = workspaces::Entity::find()
        .filter(workspaces::Column::OrgId.eq(params.org_id))
        .order_by_asc(workspaces::Column::Name)
        .all(db)
        .await
        .map_err(failed("data-flow workspace read failed"))?;
    if let Some(wanted) = params.workspace_id
        && !workspaces.iter().any(|w| w.id == wanted)
    {
        return Err(not_found(
            "workspace_not_found",
            "no such workspace in this organization",
        ));
    }
    let pipelines = load_pipelines(db, &workspaces).await?;
    let apps = load_apps(db, params).await?;
    let inputs = Inputs {
        level: params.level,
        workspace_filter: params.workspace_id,
        workspaces: workspaces
            .into_iter()
            .map(|w| WorkspaceIn {
                id: w.id,
                name: w.name,
            })
            .collect(),
        pipelines,
        apps,
    };
    Ok(Loaded {
        org: OrgRef {
            id: org.id,
            name: org.name,
            slug: org.slug,
        },
        inputs,
    })
}

#[derive(Debug, FromQueryResult)]
struct PipelineRow {
    revision_id: Uuid,
    name: String,
    file_path: String,
    definition: Value,
}

/// The pipelines compiled under each workspace's promoted revision — the
/// rows `airway_pipelines` keeps for superseded revisions are left out, as
/// `app_ops::resolve` leaves them.
async fn load_pipelines(
    db: &DatabaseConnection,
    workspaces: &[workspaces::Model],
) -> Result<Vec<PipelineIn>, ScopeError> {
    let promoted: HashMap<Uuid, Uuid> = workspaces
        .iter()
        .filter_map(|w| Some((w.current_revision_id?, w.id)))
        .collect();
    if promoted.is_empty() {
        return Ok(Vec::new());
    }
    let rows = airway_pipelines::Entity::find()
        .select_only()
        .column(airway_pipelines::Column::RevisionId)
        .column(airway_pipelines::Column::Name)
        .column(airway_pipelines::Column::FilePath)
        .column(airway_pipelines::Column::Definition)
        .filter(airway_pipelines::Column::RevisionId.is_in(promoted.keys().copied()))
        .order_by_asc(airway_pipelines::Column::Name)
        .into_model::<PipelineRow>()
        .all(db)
        .await
        .map_err(failed("data-flow pipeline read failed"))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(PipelineIn {
                workspace_id: *promoted.get(&row.revision_id)?,
                name: row.name,
                file_path: row.file_path,
                definition: row.definition,
            })
        })
        .collect())
}

/// The org's apps (in the workspace, when one is named), each with the build
/// it serves: production's, else staging's.
async fn load_apps(db: &DatabaseConnection, params: &Params) -> Result<Vec<AppIn>, ScopeError> {
    let mut query = apps::Entity::find().filter(apps::Column::OrgId.eq(params.org_id));
    if let Some(workspace) = params.workspace_id {
        query = query.filter(apps::Column::ProjectId.eq(workspace));
    }
    let rows = query
        .order_by_asc(apps::Column::Name)
        .all(db)
        .await
        .map_err(failed("data-flow app read failed"))?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let environments = load_environment_builds_batch(db, &rows)
        .await
        .map_err(failed("data-flow environment read failed"))?;
    let served: HashMap<Uuid, (Uuid, &'static str)> = environments
        .iter()
        .filter_map(|(app, builds)| {
            let picked = builds
                .production
                .map(|b| (b, "production"))
                .or(builds.staging.map(|b| (b, "staging")))?;
            Some((*app, picked))
        })
        .collect();
    let build_ids: Vec<Uuid> = served.values().map(|(build, _)| *build).collect();
    let manifests = load_manifests(db, &build_ids).await?;
    let mut functions = load_functions(db, &build_ids).await?;
    let approvals = load_approvals(db, &rows).await?;
    Ok(rows
        .into_iter()
        .map(|app| {
            let build = served.get(&app.id).copied();
            let manifest = app
                .manifest_override
                .clone()
                .or_else(|| build.and_then(|(b, _)| manifests.get(&b).cloned()));
            AppIn {
                id: app.id,
                slug: app.slug,
                name: app.name,
                workspace_id: app.project_id,
                environment: build.map(|(_, env)| env),
                manifest,
                functions: build
                    .and_then(|(b, _)| functions.remove(&b))
                    .unwrap_or_default(),
                approved_pipelines: approvals.get(&app.id).cloned().unwrap_or_default(),
            }
        })
        .collect())
}

async fn load_manifests(
    db: &DatabaseConnection,
    builds: &[Uuid],
) -> Result<HashMap<Uuid, Value>, ScopeError> {
    if builds.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Option<Value>)> = app_builds::Entity::find()
        .select_only()
        .column(app_builds::Column::Id)
        .column(app_builds::Column::ManifestJson)
        .filter(app_builds::Column::Id.is_in(builds.iter().copied()))
        .into_tuple()
        .all(db)
        .await
        .map_err(failed("data-flow manifest read failed"))?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, manifest)| Some((id, manifest?)))
        .collect())
}

/// Each build's functions, by name, with the entry the publish recorded.
async fn load_functions(
    db: &DatabaseConnection,
    builds: &[Uuid],
) -> Result<HashMap<Uuid, Vec<FunctionIn>>, ScopeError> {
    if builds.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, String, Option<Value>)> = app_functions::Entity::find()
        .select_only()
        .column(app_functions::Column::BuildId)
        .column(app_functions::Column::Name)
        .column(app_functions::Column::ManifestJson)
        .filter(app_functions::Column::BuildId.is_in(builds.iter().copied()))
        .order_by_asc(app_functions::Column::Name)
        .into_tuple()
        .all(db)
        .await
        .map_err(failed("data-flow function read failed"))?;
    let mut by_build: HashMap<Uuid, Vec<FunctionIn>> = HashMap::new();
    for (build, name, spec) in rows {
        by_build.entry(build).or_default().push(FunctionIn {
            name,
            spec: spec.unwrap_or(Value::Null),
        });
    }
    Ok(by_build)
}

/// The pipelines each app is approved to upload into: standing
/// `source_upload_report` grants, by target.
async fn load_approvals(
    db: &DatabaseConnection,
    apps: &[apps::Model],
) -> Result<HashMap<Uuid, BTreeSet<String>>, ScopeError> {
    let Some(action) = Action::SourceUploadReport.app_action_id() else {
        return Ok(HashMap::new());
    };
    let rows: Vec<(Uuid, Option<String>)> = app_platform_grants::Entity::find()
        .select_only()
        .column(app_platform_grants::Column::AppId)
        .column(app_platform_grants::Column::Target)
        .filter(app_platform_grants::Column::AppId.is_in(apps.iter().map(|a| a.id)))
        .filter(app_platform_grants::Column::Action.eq(action))
        .filter(app_platform_grants::Column::RevokedAt.is_null())
        .into_tuple()
        .all(db)
        .await
        .map_err(failed("data-flow approval read failed"))?;
    let mut approvals: HashMap<Uuid, BTreeSet<String>> = HashMap::new();
    for (app, target) in rows {
        if let Some(target) = target {
            approvals.entry(app).or_default().insert(target);
        }
    }
    Ok(approvals)
}
