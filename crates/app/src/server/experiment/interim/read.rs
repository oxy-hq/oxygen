//! `GET` and `POST /experiments/{name}/interim`: the latest interim reading of a run in
//! progress, and a request for a fresh one. A run that is not `live`/`closing` answers 409:
//! before it nothing was treated, after it the sealed report is the reading.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use entity::{experiment_interims, experiment_runs};
use oxy_experiment::run::RunState;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::in_progress;
use super::store::{enqueue_interim, in_flight, interim_of};
use crate::server::experiment::store::run_by_name;

#[derive(Debug, Serialize)]
pub struct InterimResponse {
    pub experiment: String,
    pub run_id: Uuid,
    pub state: String,
    pub design_id: Uuid,
    /// A reading is being computed.
    pub running: bool,
    pub requested_at: Option<DateTime<FixedOffset>>,
    /// Why the last attempt produced no reading. The previous reading, if any, stays.
    pub error: Option<String>,
    pub reading: Option<InterimReading>,
}

/// The fields a sealed result carries that a summary card reads, from the interim row.
#[derive(Debug, Serialize)]
pub struct InterimReading {
    pub computed_at: DateTime<FixedOffset>,
    /// The last day read, inclusive.
    pub through: NaiveDate,
    pub report: Value,
    pub effect: Value,
    pub compliance: Value,
    pub series: Option<Value>,
    pub alpha: Option<f64>,
    pub directional: Option<bool>,
}

#[derive(Debug)]
pub enum InterimError {
    NoSuchRun,
    NotInProgress { state: String },
    Db(String),
}

impl IntoResponse for InterimError {
    fn into_response(self) -> Response {
        match self {
            InterimError::NoSuchRun => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no such experiment run" })),
            )
                .into_response(),
            InterimError::NotInProgress { state } => (
                StatusCode::CONFLICT,
                Json(json!({
                    "in_progress": false,
                    "state": state,
                    "error": "an interim reads only a live or closing run",
                })),
            )
                .into_response(),
            InterimError::Db(e) => {
                tracing::error!(error = %e, "experiments: interim read failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "interim read failed" })),
                )
                    .into_response()
            }
        }
    }
}

fn db_err<E: std::fmt::Display>(e: E) -> InterimError {
    InterimError::Db(e.to_string())
}

async fn run_in_progress(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<experiment_runs::Model, InterimError> {
    let run = run_by_name(db, workspace_id, name)
        .await
        .map_err(db_err)?
        .ok_or(InterimError::NoSuchRun)?;
    if run.retrospective || !RunState::parse(&run.state).is_some_and(|s| in_progress(&s)) {
        return Err(InterimError::NotInProgress { state: run.state });
    }
    Ok(run)
}

pub async fn read_interim(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    now: DateTime<Utc>,
) -> Result<InterimResponse, InterimError> {
    let run = run_in_progress(db, workspace_id, name).await?;
    let row = interim_of(db, run.id).await.map_err(db_err)?;
    Ok(respond(name, run, row, now))
}

/// Enqueue a fresh reading unless one is already being computed, then answer as the read.
pub async fn request_interim(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    now: DateTime<Utc>,
) -> Result<InterimResponse, InterimError> {
    let run = run_in_progress(db, workspace_id, name).await?;
    enqueue_interim(db, &run, now, Duration::zero())
        .await
        .map_err(db_err)?;
    let row = interim_of(db, run.id).await.map_err(db_err)?;
    Ok(respond(name, run, row, now))
}

fn respond(
    name: &str,
    run: experiment_runs::Model,
    row: Option<experiment_interims::Model>,
    now: DateTime<Utc>,
) -> InterimResponse {
    let running = row.as_ref().is_some_and(|r| in_flight(r, now));
    let (requested_at, error) = row
        .as_ref()
        .map_or((None, None), |r| (r.requested_at, r.error.clone()));
    InterimResponse {
        experiment: name.to_string(),
        run_id: run.id,
        state: run.state,
        design_id: run.design_id,
        running,
        requested_at,
        error,
        reading: row.and_then(reading),
    }
}

fn reading(row: experiment_interims::Model) -> Option<InterimReading> {
    let report = row.report?;
    Some(InterimReading {
        computed_at: row.computed_at?,
        through: row.through?,
        alpha: report.get("alpha").and_then(Value::as_f64),
        directional: report.get("directional").and_then(Value::as_bool),
        effect: row.effect.unwrap_or(Value::Null),
        compliance: row.compliance.unwrap_or(Value::Null),
        series: row.series,
        report,
    })
}
