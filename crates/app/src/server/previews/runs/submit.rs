//! `POST /previews/runs`: record a run staff started — a procedure dry run or
//! an Airway sample — and start it if the workspace's queue is free.

use agentic_airway::preview::RequestedWindow;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{RunRequestError, advance};
use crate::server::previews::namespace::preview_key;
use crate::server::previews::sample;

/// `POST /previews/runs`.
#[derive(Debug, Deserialize)]
pub struct SubmitRun {
    pub branch: String,
    /// `procedure` | `airway_sample`.
    pub kind: String,
    /// The automation (`procedure`) or `.airway.yml` (`airway_sample`) path.
    #[serde(rename = "ref")]
    pub target_ref: String,
    #[serde(default)]
    pub variables: Option<Value>,
    /// Read live tables even where the preview holds a copy (D6). Writes land
    /// in the preview either way.
    #[serde(default)]
    pub read_live_only: bool,
    /// `airway_sample` of a windowed source: `[from, to)`, RFC 3339. Absent is
    /// the last 7 days.
    #[serde(default)]
    pub window: Option<RequestedWindow>,
    /// `airway_sample`: the resources to read.
    #[serde(default)]
    pub resources: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Submitted {
    pub run_id: String,
    pub state: String,
}

/// Record the run and start it if the workspace's queue is free.
pub async fn submit(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    requested_by: Uuid,
    req: SubmitRun,
) -> Result<Submitted, RunRequestError> {
    if !matches!(req.kind.as_str(), "procedure" | sample::RUN_KIND) {
        return Err(RunRequestError::BadRequest(format!(
            "kind `{}` is not supported; `procedure` and `{}` runs are",
            req.kind,
            sample::RUN_KIND
        )));
    }
    let branch = req.branch.trim();
    crate::server::previews::service::validate_branch_name(branch)
        .map_err(|e| RunRequestError::BadRequest(e.to_string()))?;
    let revision_id = ready_revision(db, workspace_id, branch).await?;
    check_ref(&req.target_ref)?;
    let options = run_options(db, workspace_id, revision_id, &req).await?;
    let run_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO workspace_preview_runs \
             (run_id, workspace_id, branch, preview_key, revision_id, kind, target_ref, options, \
              state, requested_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'queued', $9)",
        [
            run_id.clone().into(),
            workspace_id.into(),
            branch.into(),
            preview_key(workspace_id, branch).into(),
            revision_id.into(),
            req.kind.clone().into(),
            req.target_ref.clone().into(),
            options.into(),
            requested_by.into(),
        ],
    ))
    .await?;
    tracing::info!(target: "preview", %workspace_id, branch, %run_id, kind = %req.kind,
        target_ref = %req.target_ref, "preview run queued");
    advance(db, workspace_id).await?;
    Ok(Submitted {
        state: current_state(db, &run_id).await?,
        run_id,
    })
}

/// The run's stored options, once its kind's rules accept it: a sample's
/// (`previews::sample::validate`), or a procedure's variables.
async fn run_options(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    revision_id: Uuid,
    req: &SubmitRun,
) -> Result<Value, RunRequestError> {
    let target = &req.target_ref;
    if req.kind == sample::RUN_KIND {
        let options = sample::validate(
            db,
            workspace_id,
            revision_id,
            target,
            req.window,
            &req.resources,
        )
        .await?;
        return Ok(options.to_json());
    }
    require_automation(db, revision_id, target).await?;
    Ok(json!({ "variables": req.variables, "read_live_only": req.read_live_only }))
}

/// The preview's ready staging revision.
async fn ready_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: &str,
) -> Result<Uuid, RunRequestError> {
    let preview = crate::server::previews::store::find(db, workspace_id, branch)
        .await?
        .ok_or_else(|| RunRequestError::PreviewNotFound(branch.to_string()))?;
    let staged =
        crate::server::api::compile_staging::status_for_sha(db, workspace_id, &preview.git_sha)
            .await
            .map_err(|(_, message)| RunRequestError::Internal(message))?;
    match (staged.status.as_str(), staged.revision_id) {
        ("ready", Some(revision_id)) => Ok(revision_id),
        _ => Err(RunRequestError::NotReady(branch.to_string())),
    }
}

/// `target_ref` is a workspace-relative path that no preview scoped.
fn check_ref(target_ref: &str) -> Result<(), RunRequestError> {
    let path = std::path::Path::new(target_ref);
    if target_ref.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || agentic_automation::preview_names::is_scoped(target_ref)
    {
        return Err(RunRequestError::BadRequest(format!(
            "ref {target_ref:?} is not a workspace-relative path"
        )));
    }
    Ok(())
}

/// `target_ref` has an automation definition in the staging revision.
async fn require_automation(
    db: &DatabaseConnection,
    revision_id: Uuid,
    target_ref: &str,
) -> Result<(), RunRequestError> {
    let found = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS found FROM automation_definitions WHERE revision_id = $1 AND file_path = $2",
            [revision_id.into(), target_ref.into()],
        ))
        .await?;
    match found {
        Some(_) => Ok(()),
        None => Err(RunRequestError::RefNotInRevision(target_ref.to_string())),
    }
}

async fn current_state(db: &DatabaseConnection, run_id: &str) -> Result<String, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT state FROM workspace_preview_runs WHERE run_id = $1",
            [run_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::RecordNotFound(format!("preview run {run_id}")))?;
    row.try_get("", "state")
}
