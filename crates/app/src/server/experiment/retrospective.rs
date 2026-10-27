//! A retrospective run: the analysis of a design whose dates are all past, read from
//! the warehouse after the fact.
//!
//! It is the same `experiment_runs` row a registration makes, with three differences.
//! It is created `closing`, because its last window is already history. It moves no
//! lever — [`create`] never resolves `via`, and the tick steps it with
//! `step_retrospective`, which emits no flip — so it is never checked; the estimate pins
//! its pool from the design's own pre window (`estimate::retro`). And its report is
//! labelled not pre-registered: pre-registration is what lets a plain test stand without
//! a multiplicity correction (spec, "Pre-registration is what removes the multiplicity
//! correction"), so a retrospective result must never read like a registered one.
//!
//! One run per experiment still holds: a retrospective run and a registered run exclude
//! each other.

use chrono::{DateTime, Utc};
use entity::{experiment_runs, experiments};
use oxy_experiment::ExperimentConfig;
use oxy_experiment::timeline::{last_post_window_end_date, today_in};
use oxy_experiment::validate::is_retrospective_ready;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::design_store;
use super::register::{RegisterError, lock_draft};
use super::store;

#[derive(Debug, Clone, PartialEq)]
pub enum Retrospective {
    Draft { problems: Vec<String> },
    Refused { reason: String },
    Ready,
}

/// The design alone, at `now`: a draft, a window not yet past, or ready. Its last day
/// must have ended in `design.timezone` — a window still open is a registration.
pub fn judge(c: &ExperimentConfig, now: DateTime<Utc>) -> Retrospective {
    if let Err(errors) = is_retrospective_ready(c) {
        let problems = errors.iter().map(ToString::to_string).collect();
        return Retrospective::Draft { problems };
    }
    let Some(end) = last_post_window_end_date(c) else {
        return Retrospective::Draft {
            problems: vec!["the design has no post window".into()],
        };
    };
    let today = today_in(c.design.timezone, now);
    if today >= end {
        return Retrospective::Ready;
    }
    Retrospective::Refused {
        reason: format!(
            "the design's last window runs until {end} (exclusive) in {tz} and today is \
             {today}: a retrospective reads only days that are over",
            tz = c.design.timezone
        ),
    }
}

/// Create the retrospective run of `experiment`'s draft — the one read at
/// `expected_hash`, or whatever it is when `None` — by `user` at `now`, and freeze
/// the design: a run is pinned to it either way. Due at once, so the next tick
/// enqueues its estimate.
pub async fn create(
    db: &DatabaseConnection,
    experiment: &experiments::Model,
    expected_hash: Option<&str>,
    user: Uuid,
    now: DateTime<Utc>,
) -> Result<experiment_runs::Model, RegisterError> {
    let txn = db.begin().await?;
    // A refused retrospective leaves no result: read the design again instead.
    let (_, row) = design_store::lock_design(&txn, experiment.workspace_id, &experiment.name)
        .await
        .map_err(|e| match e {
            design_store::DesignError::Db(e) => RegisterError::Db(e),
            _ => RegisterError::NoDesign(experiment.name.clone()),
        })?;
    super::reopen::reopen_refused(&txn, experiment.id, &row).await?;
    let design = lock_draft(&txn, experiment, expected_hash).await?;
    match judge(&design.config, now) {
        Retrospective::Draft { problems } => return Err(RegisterError::Draft(problems)),
        Retrospective::Refused { reason } => return Err(RegisterError::Refused(reason)),
        Retrospective::Ready => {}
    }
    let run_id = Uuid::new_v4();
    let today = today_in(design.config.design.timezone, now);
    if !insert(&txn, experiment, run_id, design.row.id, user, now, today).await? {
        return Err(RegisterError::AlreadyRegistered(experiment.name.clone()));
    }
    design_store::freeze(&txn, design.row, now).await?;
    txn.commit().await?;
    tracing::info!(
        target: "experiment", workspace_id = %experiment.workspace_id,
        experiment = %experiment.name, created_by = %user, "retrospective run created"
    );
    store::run_by_id(db, run_id)
        .await?
        .ok_or_else(|| RegisterError::Db(DbErr::RecordNotFound(format!("run {run_id}"))))
}

async fn insert(
    db: &impl ConnectionTrait,
    experiment: &experiments::Model,
    run_id: Uuid,
    design_id: Uuid,
    user: Uuid,
    now: DateTime<Utc>,
    today: chrono::NaiveDate,
) -> Result<bool, DbErr> {
    Ok(db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO experiment_runs (id, workspace_id, experiment_id, design_id, \
               registered_by, registered_at, state, closing_since, retrospective, \
               next_check_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, 'closing', $7, true, $6, $6, $6) \
             ON CONFLICT (experiment_id) DO NOTHING",
            [
                run_id.into(),
                experiment.workspace_id.into(),
                experiment.id.into(),
                design_id.into(),
                user.into(),
                now.fixed_offset().into(),
                today.into(),
            ],
        ))
        .await?
        .rows_affected()
        == 1)
}
