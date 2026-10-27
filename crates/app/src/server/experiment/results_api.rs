//! The sealed report, once there is one (`GET /experiments/{name}/result`).
//!
//! **There is no effect endpoint before `reported`**, enforced here rather than
//! hidden in the UI (spec § "Results and the surface"). The gate reads
//! `experiment_runs.state`, *never* the existence of a results row: the estimate
//! job writes the row a tick before the state advances, and a row that exists is
//! not a licence to read it. An unrecognised state is sealed.
//!
//! `FleetOk`: two persisted-data reads, no working copy. The HTTP handler is
//! `api::experiment::get_result`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset};
use entity::{experiment_results, experiment_runs};
use oxy_experiment::patch::CoefficientEdit;
use oxy_experiment::run::RunState;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::store::run_by_name;

const NO_WRITE_BACK: &str = "no coefficient was proposed for this run";

/// Why a result carries no coefficient patch: the reason the report stored, else a fixed line.
pub(crate) fn no_patch_reason(report: &Value) -> String {
    report
        .pointer("/write_back/reason")
        .and_then(Value::as_str)
        .unwrap_or(NO_WRITE_BACK)
        .to_string()
}

#[derive(Debug, Serialize)]
pub struct WriteBackAvailability {
    pub available: bool,
    pub reason: Option<String>,
    pub edit: Option<CoefficientEdit>,
}

#[derive(Debug, Serialize)]
pub struct ResultResponse {
    pub experiment: String,
    pub run_id: Uuid,
    pub state: String,
    /// The design version the run executed.
    pub design_id: Uuid,
    pub aborted: bool,
    pub created_at: DateTime<FixedOffset>,
    pub report: Value,
    pub effect: Value,
    pub ratio: Option<Value>,
    /// As stored; `n_treated == 0` is *not measured*, never 0%.
    pub compliance: Value,
    pub write_back: WriteBackAvailability,
    /// Lifted from the stored `report`, which stays the source of truth (see [`lift`]).
    pub directional: bool,
    pub alpha: f64,
    /// The `experiment_results.prediction` column — equal to `report.prediction`.
    pub prediction: Option<Value>,
    pub assumptions: Vec<String>,
    /// The sealed day-by-day series; `None` on a result written before it existed.
    pub series: Option<Value>,
}

#[derive(Debug)]
pub enum ResultError {
    NoSuchRun,
    /// The run is not `reported`/`aborted` (or its state is unrecognised).
    Sealed {
        state: String,
    },
    /// Terminal, but the estimate job has not written the row yet.
    Pending {
        state: String,
    },
    Db(String),
    /// A stored report that lacks a field every report carries.
    Corrupt(String),
}

impl ResultError {
    fn db<E: std::fmt::Display>(e: E) -> Self {
        ResultError::Db(e.to_string())
    }
}

impl IntoResponse for ResultError {
    fn into_response(self) -> Response {
        match self {
            ResultError::NoSuchRun => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no such experiment run" })),
            )
                .into_response(),
            ResultError::Sealed { state } => (
                StatusCode::CONFLICT,
                Json(json!({ "sealed": true, "state": state })),
            )
                .into_response(),
            ResultError::Pending { state } => (
                StatusCode::CONFLICT,
                Json(json!({ "sealed": false, "pending": true, "state": state })),
            )
                .into_response(),
            ResultError::Db(e) | ResultError::Corrupt(e) => {
                tracing::error!(error = %e, "experiments: result read failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "result read failed" })),
                )
                    .into_response()
            }
        }
    }
}

/// Never reads `experiment_results` before the state check has passed.
pub async fn read_result(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<ResultResponse, ResultError> {
    let run = run_by_name(db, workspace_id, name)
        .await
        .map_err(ResultError::db)?
        .ok_or(ResultError::NoSuchRun)?;
    if !matches!(
        RunState::parse(&run.state),
        Some(RunState::Reported | RunState::Aborted)
    ) {
        return Err(ResultError::Sealed { state: run.state });
    }
    let row = experiment_results::Entity::find()
        .filter(experiment_results::Column::RunId.eq(run.id))
        .one(db)
        .await
        .map_err(ResultError::db)?
        .ok_or_else(|| ResultError::Pending {
            state: run.state.clone(),
        })?;
    respond(name, run, row)
}

fn respond(
    name: &str,
    run: experiment_runs::Model,
    row: experiment_results::Model,
) -> Result<ResultResponse, ResultError> {
    let lifted = lift(&row.report)?;
    Ok(ResultResponse {
        experiment: name.to_string(),
        run_id: run.id,
        state: run.state,
        design_id: run.design_id,
        aborted: row.aborted,
        created_at: row.created_at,
        write_back: availability(&row),
        report: row.report,
        effect: row.effect,
        ratio: row.ratio,
        compliance: row.compliance,
        directional: lifted.directional,
        alpha: lifted.alpha,
        assumptions: lifted.assumptions,
        prediction: row.prediction,
        series: row.series,
    })
}

struct Lifted {
    directional: bool,
    alpha: f64,
    assumptions: Vec<String>,
}

/// `directional`, `alpha` and `assumptions` live in the stored report so a re-read years
/// later renders the same page; the body lifts them so a client need not parse `report`.
/// A report missing one is corrupt — never defaulted, because a guessed alpha would
/// label a directional result "significant".
fn lift(report: &Value) -> Result<Lifted, ResultError> {
    let missing = |what: &str| ResultError::Corrupt(format!("the stored report carries no {what}"));
    Ok(Lifted {
        alpha: report
            .get("alpha")
            .and_then(Value::as_f64)
            .ok_or_else(|| missing("alpha"))?,
        directional: report
            .get("directional")
            .and_then(Value::as_bool)
            .ok_or_else(|| missing("directional flag"))?,
        assumptions: report
            .get("assumptions")
            .and_then(Value::as_array)
            .ok_or_else(|| missing("assumptions list"))?
            .iter()
            .filter_map(|a| a.as_str().map(str::to_string))
            .collect(),
    })
}

fn availability(row: &experiment_results::Model) -> WriteBackAvailability {
    let edit = row.coefficient_patch.as_deref().and_then(|p| {
        serde_json::from_str::<CoefficientEdit>(p)
            .map_err(|e| {
                tracing::warn!(run_id = %row.run_id, error = %e, "experiments: the stored coefficient patch does not parse");
            })
            .ok()
    });
    let reason = if edit.is_some() {
        None
    } else {
        Some(no_patch_reason(&row.report))
    };
    WriteBackAvailability {
        available: edit.is_some(),
        reason,
        edit,
    }
}
