//! Registration: an explicit action. Someone with the right registers an
//! experiment's draft design: it is frozen at the server's clock — the
//! pre-registration proof — and the run is pinned to it for good. Nothing about
//! the design can change after that; a change is a duplicate, a new experiment.
//!
//! [`register`] refuses unless the draft is the one the caller read, registrable, `now` is strictly
//! before its registration deadline (the platform's own clock, so it can only
//! refuse a registration that was in time, never admit a late one), both levers
//! resolve to exactly one automation, and the experiment has no run yet.
//!
//! [`decide`] is the same judgement without the write: the list and
//! `GET /run` ask it what the draft would get.

use chrono::{DateTime, Utc};
use entity::{experiment_runs, experiments};
use oxy_experiment::ExperimentConfig;
use oxy_experiment::timeline::registration_deadline;
use oxy_experiment::validate::is_registrable;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use super::design_store::{self, Design, DesignError};
use super::{lever, store};

#[derive(Debug, Clone, PartialEq)]
pub enum Registration {
    Draft { problems: Vec<String> },
    Refused { reason: String },
    Ready,
}

/// The design alone, at `now`: a draft, past its deadline, or in time.
pub fn judge(c: &ExperimentConfig, now: DateTime<Utc>) -> Registration {
    if let Err(errors) = is_registrable(c) {
        let problems = errors.iter().map(ToString::to_string).collect();
        return Registration::Draft { problems };
    }
    let Some(deadline) = registration_deadline(c) else {
        return Registration::Draft {
            problems: vec!["the design has no waves and no switchback periods".into()],
        };
    };
    if now < deadline {
        return Registration::Ready;
    }
    Registration::Refused {
        reason: format!(
            "it is {now}, which is not before the registration deadline {deadline} (the start \
             of the first switch day — the first switch_on or switchback period — minus \
             anticipation_days, in {tz})",
            tz = c.design.timezone
        ),
    }
}

/// [`judge`], then — only for a design in time — the lever check (spec "Lever
/// checks before the first flip": each `via` resolves, at the promoted revision,
/// to exactly one automation; PR 3's `check_levers`, the one pricing uses). A
/// database fault is an error.
pub async fn decide(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    c: &ExperimentConfig,
    now: DateTime<Utc>,
) -> Result<Registration, DbErr> {
    decide_with(db, workspace_id, c, now, &mut lever::LeverChecks::default()).await
}

/// [`decide`] sharing lever checks across the designs of one request.
pub async fn decide_with(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    c: &ExperimentConfig,
    now: DateTime<Utc>,
    levers: &mut lever::LeverChecks,
) -> Result<Registration, DbErr> {
    let verdict = judge(c, now);
    if verdict != Registration::Ready {
        return Ok(verdict);
    }
    let lines = levers
        .check(db, workspace_id, c)
        .await
        .map_err(DbErr::Custom)?;
    Ok(match lines.into_iter().next() {
        Some(reason) => Registration::Refused { reason },
        None => Registration::Ready,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("experiment `{0}` has no design")]
    NoDesign(String),
    #[error(
        "the design was changed since you read it (you read {given}, it is now {current}); reload and register again"
    )]
    Changed { given: String, current: String },
    #[error("the design is a draft: {}", .0.join("; "))]
    Draft(Vec<String>),
    #[error("registration refused: {0}")]
    Refused(String),
    #[error("experiment `{0}` is already registered; a registered run is never replaced")]
    AlreadyRegistered(String),
    #[error("{0}")]
    Unreadable(String),
    #[error("database: {0}")]
    Db(#[from] DbErr),
}

/// The draft of `experiment`, locked for the rest of `txn` and decoded, if no
/// run exists and it is still the one the caller read (`expected_hash`).
pub(super) async fn lock_draft(
    txn: &DatabaseTransaction,
    experiment: &experiments::Model,
    expected_hash: Option<&str>,
) -> Result<Design, RegisterError> {
    let name = &experiment.name;
    let (_, row) = design_store::lock_design(txn, experiment.workspace_id, name)
        .await
        .map_err(|e| match e {
            DesignError::Db(e) => RegisterError::Db(e),
            _ => RegisterError::NoDesign(name.clone()),
        })?;
    if row.frozen_at.is_some() || store::run_of(txn, experiment.id).await?.is_some() {
        return Err(RegisterError::AlreadyRegistered(name.clone()));
    }
    if let Some(given) = expected_hash.filter(|h| *h != row.content_hash) {
        return Err(RegisterError::Changed {
            given: given.to_string(),
            current: row.content_hash,
        });
    }
    design_store::decode(row).map_err(RegisterError::Unreadable)
}

/// Register `experiment`'s draft — the one read at `expected_hash`, or whatever
/// it is when `None` — as its one run, by `user` at `now`, and freeze it at
/// `now`. One transaction: an edit racing this lands before (and is what gets
/// registered and checked) or is refused as frozen.
pub async fn register(
    db: &DatabaseConnection,
    experiment: &experiments::Model,
    expected_hash: Option<&str>,
    user: Uuid,
    now: DateTime<Utc>,
) -> Result<experiment_runs::Model, RegisterError> {
    let txn = db.begin().await?;
    let design = lock_draft(&txn, experiment, expected_hash).await?;
    match decide(&txn, experiment.workspace_id, &design.config, now).await? {
        Registration::Draft { problems } => return Err(RegisterError::Draft(problems)),
        Registration::Refused { reason } => return Err(RegisterError::Refused(reason)),
        Registration::Ready => {}
    }
    let run_id = Uuid::new_v4();
    if !insert_run(&txn, experiment, run_id, design.row.id, user, now).await? {
        return Err(RegisterError::AlreadyRegistered(experiment.name.clone()));
    }
    design_store::freeze(&txn, design.row, now).await?;
    txn.commit().await?;
    tracing::info!(
        target: "experiment", workspace_id = %experiment.workspace_id,
        experiment = %experiment.name, registered_by = %user, "experiment registered"
    );
    store::run_by_id(db, run_id)
        .await?
        .ok_or_else(|| RegisterError::Db(DbErr::RecordNotFound(format!("run {run_id}"))))
}

async fn insert_run(
    txn: &DatabaseTransaction,
    experiment: &experiments::Model,
    run_id: Uuid,
    design_id: Uuid,
    user: Uuid,
    now: DateTime<Utc>,
) -> Result<bool, DbErr> {
    Ok(txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO experiment_runs (id, workspace_id, experiment_id, design_id, \
               registered_by, registered_at, state, next_check_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, 'registered', $6, $6, $6) \
             ON CONFLICT (experiment_id) DO NOTHING",
            [
                run_id.into(),
                experiment.workspace_id.into(),
                experiment.id.into(),
                design_id.into(),
                user.into(),
                now.fixed_offset().into(),
            ],
        ))
        .await?
        .rows_affected()
        == 1)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use oxy_experiment::timeline::registration_deadline;

    use super::*;
    use crate::server::experiment::test_design::{design, design_with};

    #[test]
    fn a_design_registered_before_the_deadline_is_ready() {
        let c = design();
        let now = registration_deadline(&c).unwrap() - Duration::seconds(1);
        assert_eq!(judge(&c, now), Registration::Ready);
    }

    #[test]
    fn a_design_registered_at_the_deadline_is_refused_and_names_its_guard() {
        let c = design();
        let deadline = registration_deadline(&c).unwrap();
        let Registration::Refused { reason } = judge(&c, deadline) else {
            panic!("strictly before, so the deadline itself is late");
        };
        assert!(
            reason.contains("not before the registration deadline"),
            "{reason}"
        );
        assert!(reason.contains("Australia/Sydney"), "{reason}");
    }

    #[test]
    fn a_draft_stays_a_draft() {
        let draft = design_with("  seed: 8841207\n", "");
        assert!(matches!(
            judge(&draft, Utc::now()),
            Registration::Draft { .. }
        ));
    }
}
