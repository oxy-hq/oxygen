//! Experiments as records: create one, read and edit its draft design, adopt a
//! priced proposal onto it, register it (which freezes it), duplicate a frozen
//! one. Thin: parse, call `server::experiment`, serialize. All FleetOk —
//! Postgres only, no working copy.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::{DateTime, FixedOffset, Utc};
use entity::experiment_runs;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_experiment::ExperimentConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{ApiError, connect, internal};
use crate::server::api::middlewares::role_guards::{
    ExperimentAuthorGuard, ExperimentRegisterGuard,
};
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::experiment::adopt::{AdoptError, adopt_proposal};
use crate::server::experiment::design_store::{self, Design, DesignError, Edit};
use crate::server::experiment::register::{RegisterError, register};

/// The experiment's design, as every route here returns it.
#[derive(Debug, Serialize)]
pub struct DesignBody {
    pub id: Uuid,
    pub content_hash: String,
    /// When Register froze it; `None` while it is a draft.
    pub frozen_at: Option<DateTime<FixedOffset>>,
    pub from_proposal_id: Option<Uuid>,
    pub created_by: Uuid,
    pub created_at: DateTime<FixedOffset>,
    pub updated_by: Uuid,
    pub updated_at: DateTime<FixedOffset>,
    pub config: Value,
    /// Frozen, but its only run is a retrospective that was refused: an edit, or
    /// asking for the analysis again, reopens it. Set only by `GET …/design`.
    pub reopenable: bool,
}

impl From<entity::experiment_designs::Model> for DesignBody {
    fn from(d: entity::experiment_designs::Model) -> Self {
        Self {
            id: d.id,
            content_hash: d.content_hash,
            frozen_at: d.frozen_at,
            from_proposal_id: d.from_proposal_id,
            created_by: d.created_by,
            created_at: d.created_at,
            updated_by: d.updated_by,
            updated_at: d.updated_at,
            config: normalized(d.config),
            reopenable: false,
        }
    }
}

/// The stored config as it decodes, so keys the config defaults are present
/// even on a row written outside the API (a seed, or the records migration's
/// backfill). A row that no longer decodes is returned as stored.
fn normalized(config: Value) -> Value {
    serde_json::from_value::<ExperimentConfig>(config.clone())
        .and_then(serde_json::to_value)
        .unwrap_or(config)
}

impl From<Design> for DesignBody {
    fn from(d: Design) -> Self {
        d.row.into()
    }
}

fn design_error(e: DesignError) -> ApiError {
    match e {
        DesignError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
        DesignError::Duplicate(_) | DesignError::Stale { .. } | DesignError::Frozen(_) => {
            (StatusCode::CONFLICT, e.to_string())
        }
        DesignError::Invalid(_) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
        DesignError::Db(e) => internal("write the design")(e),
    }
}

pub(super) async fn experiment_or_404(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<entity::experiments::Model, ApiError> {
    design_store::experiment_by_name(db, workspace_id, name)
        .await
        .map_err(internal("read the experiment"))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("no experiment named `{name}`"),
            )
        })
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub config: ExperimentConfig,
}

/// `POST /experiments` — a new experiment and its draft.
pub async fn create_experiment(
    _: ExperimentAuthorGuard,
    WorkspaceManagerReadOnly(wm): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<DesignBody>), ApiError> {
    let db = connect().await?;
    let (_, design) = design_store::create_experiment(&db, wm.workspace_id, body.config, user.id)
        .await
        .map_err(design_error)?;
    Ok((StatusCode::CREATED, Json(design.into())))
}

/// `GET /experiments/{name}/design` — the one design, draft or frozen.
pub async fn get_design(
    WorkspaceManagerReadOnly(wm): WorkspaceManagerReadOnly,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
) -> Result<Json<DesignBody>, ApiError> {
    let db = connect().await?;
    let experiment = experiment_or_404(&db, wm.workspace_id, &name).await?;
    let mut body: DesignBody = design_store::design_of(&db, experiment.id)
        .await
        .map_err(internal("read the design"))?
        .map(Into::into)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("`{name}` has no design")))?;
    if body.frozen_at.is_some() {
        body.reopenable = crate::server::experiment::reopen::is_reopenable(&db, experiment.id)
            .await
            .map_err(internal("read the run"))?;
    }
    Ok(Json(body))
}

#[derive(Debug, Deserialize)]
pub struct EditBody {
    pub config: ExperimentConfig,
    /// The `content_hash` the editor read; a stale one is a 409.
    #[serde(default)]
    pub base_hash: Option<String>,
}

/// `PUT /experiments/{name}/design` — overwrite the draft. A frozen design is a
/// 409: it is duplicated, never edited.
pub async fn put_design(
    _: ExperimentAuthorGuard,
    // Reopening a refused retrospective deletes a run only the admin ring creates.
    register: Result<ExperimentRegisterGuard, StatusCode>,
    WorkspaceManagerReadOnly(wm): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
    Json(body): Json<EditBody>,
) -> Result<Json<DesignBody>, ApiError> {
    let db = connect().await?;
    let edit = Edit {
        config: body.config,
        base_hash: body.base_hash,
        updated_by: user.id,
        may_reopen: register.is_ok(),
    };
    let design = design_store::update_draft(&db, wm.workspace_id, &name, edit)
        .await
        .map_err(design_error)?;
    Ok(Json(design.into()))
}

#[derive(Debug, Deserialize)]
pub struct DuplicateBody {
    pub name: String,
}

/// `POST /experiments/{name}/duplicate` — a new experiment whose draft is this
/// one's design, renamed. How a registered experiment is changed.
pub async fn duplicate_experiment(
    _: ExperimentAuthorGuard,
    WorkspaceManagerReadOnly(wm): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
    Json(body): Json<DuplicateBody>,
) -> Result<(StatusCode, Json<DesignBody>), ApiError> {
    let db = connect().await?;
    let (_, design) = design_store::duplicate(&db, wm.workspace_id, &name, &body.name, user.id)
        .await
        .map_err(design_error)?;
    Ok((StatusCode::CREATED, Json(design.into())))
}

/// `POST /experiments/{name}/proposals/{id}/use` — pricing's output keys,
/// written onto the draft the proposal priced.
pub async fn use_proposal(
    _: ExperimentAuthorGuard,
    WorkspaceManagerReadOnly(wm): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((_workspace_id, name, proposal_id)): Path<(Uuid, String, Uuid)>,
) -> Result<Json<DesignBody>, ApiError> {
    let db = connect().await?;
    match adopt_proposal(&db, wm.workspace_id, &name, proposal_id, user.id).await {
        Ok(design) => Ok(Json(design.into())),
        Err(AdoptError::Design(e)) => Err(design_error(e)),
        Err(e @ AdoptError::NoSuchProposal { .. }) => Err((StatusCode::NOT_FOUND, e.to_string())),
        Err(e @ AdoptError::Unreadable { .. }) => Err(internal("adopt the proposal")(e)),
        Err(e) => Err((StatusCode::CONFLICT, e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterBody {
    /// The draft's `content_hash` as the caller read it: a draft edited since
    /// is a 409, never registered unseen.
    #[serde(default)]
    pub content_hash: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Registered {
    pub run_id: Uuid,
    pub design_id: Uuid,
    pub registered_at: DateTime<FixedOffset>,
    /// The design was frozen at the same instant.
    pub frozen_at: DateTime<FixedOffset>,
}

/// A registration or a retrospective run, as a response.
pub(super) fn registered(
    result: Result<experiment_runs::Model, RegisterError>,
    what: &'static str,
) -> Result<(StatusCode, Json<Registered>), ApiError> {
    match result {
        Ok(run) => Ok((
            StatusCode::CREATED,
            Json(Registered {
                run_id: run.id,
                design_id: run.design_id,
                registered_at: run.registered_at,
                frozen_at: run.registered_at,
            }),
        )),
        Err(e @ RegisterError::NoDesign(_)) => Err((StatusCode::NOT_FOUND, e.to_string())),
        Err(e @ (RegisterError::AlreadyRegistered(_) | RegisterError::Changed { .. })) => {
            Err((StatusCode::CONFLICT, e.to_string()))
        }
        Err(e @ (RegisterError::Draft(_) | RegisterError::Refused(_))) => {
            Err((StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        Err(e) => Err(internal(what)(e)),
    }
}

/// `POST /experiments/{name}/register` — freeze the draft and make it the run.
pub async fn post_register(
    _: ExperimentRegisterGuard,
    WorkspaceManagerReadOnly(wm): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
    Json(body): Json<RegisterBody>,
) -> Result<(StatusCode, Json<Registered>), ApiError> {
    let db = connect().await?;
    let experiment = experiment_or_404(&db, wm.workspace_id, &name).await?;
    let hash = body.content_hash.as_deref();
    registered(
        register(&db, &experiment, hash, user.id, Utc::now()).await,
        "register",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::experiment::test_design::design;

    fn row(config: Value) -> entity::experiment_designs::Model {
        let at = Utc::now().fixed_offset();
        entity::experiment_designs::Model {
            id: Uuid::from_u128(1),
            experiment_id: Uuid::from_u128(2),
            from_proposal_id: None,
            config,
            content_hash: String::new(),
            created_by: Uuid::from_u128(3),
            created_at: at,
            updated_by: Uuid::from_u128(3),
            updated_at: at,
            frozen_at: None,
        }
    }

    /// Edit design pre-fills from this, so a stored row missing a defaulted key
    /// (a seed, or the records migration's backfill) still arrives whole.
    #[test]
    fn a_design_fills_in_what_the_stored_design_left_out() {
        let mut stored = serde_json::to_value(design()).unwrap();
        stored.as_object_mut().unwrap().remove("guardrails");
        let v = serde_json::to_value(DesignBody::from(row(stored))).unwrap();
        assert_eq!(v["config"]["guardrails"], serde_json::json!([]));
    }

    /// A row that no longer decodes is still shown as stored, not dropped.
    #[test]
    fn a_design_that_does_not_decode_is_returned_as_stored() {
        let stored = serde_json::json!({ "experiment": "x", "unknown": 1 });
        let v = serde_json::to_value(DesignBody::from(row(stored.clone()))).unwrap();
        assert_eq!(v["config"], stored);
    }
}
