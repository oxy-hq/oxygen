//! One `experiment_price` job: settle one `pending` proposal.
//!
//! A refusal is a value (`refused`, with what was tested); a fault is a `failed`
//! row with the reason. Both are rows, so a caller polling the proposal list
//! never waits on a timeout. The queue only ever sees "done" or "failed".

use chrono::{DateTime, Utc};
use entity::experiment_proposals;
use oxy_experiment::timeline::today_in;
use oxy_experiment::{ExperimentConfig, PriorCoefficient};
use oxy_shared::errors::OxyError;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use uuid::Uuid;

use super::candidate::Candidate;
use super::design_store;
use super::lever::check_levers;
use super::panel::{PanelContext, PanelOutcome};
use super::price::{Iterations, PriceComputation, Priced, RunParams, SchedulePlan, price_design};
use super::proposal_store::{self, DoneFields, ProposalStatus};
use super::reads::{Readings, read_ask, warnings as read_warnings};
use super::source::PanelSource;

pub struct PriceJob<'a> {
    pub db: &'a DatabaseConnection,
    pub proposal_id: Uuid,
    pub now: DateTime<Utc>,
    pub iterations: Iterations,
}

/// How a job ended. `error` is set exactly when `status` is `Failed`.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub proposal_id: Uuid,
    pub status: ProposalStatus,
    /// The row was already settled (a redelivered attempt, or a racing one won):
    /// nothing was computed or written.
    pub already_finished: bool,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PriceError {
    #[error("database: {0}")]
    Db(#[from] DbErr),
    #[error("no proposal {0}")]
    NoSuchProposal(Uuid),
    #[error("the lever automations could not be read: {0}")]
    Lever(String),
    #[error("the warehouse could not be read for pricing: {0}")]
    Panel(OxyError),
    #[error("the pricing computation panicked: {0}")]
    Panicked(String),
    #[error("reading or encoding the proposal: {0}")]
    Encode(#[from] serde_json::Error),
}

/// A priced candidate with the pieces the warehouse read added.
struct Done {
    priced: Priced,
    prior: Option<PriorCoefficient>,
    /// Lag, spread and unread-diagnostic warnings; after the pricing core's own.
    extra: Vec<String>,
}

/// What a computation decided, before it is written.
enum Settlement {
    Done(Box<Done>),
    Refused {
        reason: String,
        warnings: Vec<String>,
    },
}

fn refuse(reason: String) -> Result<Settlement, PriceError> {
    Ok(Settlement::Refused {
        reason,
        warnings: Vec::new(),
    })
}

/// The seed for one proposal: the high 64 bits of a v4 UUID (OS entropy) masked
/// to 53 bits, so the JSON number the surface reads survives `JSON.parse`. Drawn
/// when the job starts and recorded with the proposal — not derived from the
/// request, the experiment or the clock, so a caller cannot tune a request until
/// a favourable split appears; every draw is logged instead.
pub(crate) fn fresh_seed() -> u64 {
    Uuid::new_v4().as_u64_pair().0 & ((1u64 << 53) - 1)
}

/// Settle the proposal, whatever happens. Never returns an error: a fault is
/// recorded on the row and reported in [`Finished::error`].
pub async fn run_job(job: PriceJob<'_>, source: &dyn PanelSource) -> Finished {
    match run_price(&job, source).await {
        Ok(finished) => finished,
        Err(e) => record_failure(&job, e).await,
    }
}

async fn record_failure(job: &PriceJob<'_>, e: PriceError) -> Finished {
    let reason = e.to_string();
    tracing::error!(proposal_id = %job.proposal_id, error = %reason, "experiment price job failed");
    // Best effort: if this write also fails the stored row stays `pending`, but
    // the task ends `failed` in the queue, so the read models report the row
    // `failed` (`proposal_store::stopped_jobs`); the task outcome carries the reason.
    if let Err(write) = proposal_store::finish_failed(job.db, job.proposal_id, &reason).await {
        tracing::error!(proposal_id = %job.proposal_id, error = %write, "could not record the failure");
    }
    Finished {
        proposal_id: job.proposal_id,
        status: ProposalStatus::Failed,
        already_finished: false,
        error: Some(reason),
    }
}

async fn run_price(job: &PriceJob<'_>, source: &dyn PanelSource) -> Result<Finished, PriceError> {
    let Some(row) = proposal_store::find_proposal(job.db, job.proposal_id).await? else {
        return Err(PriceError::NoSuchProposal(job.proposal_id));
    };
    // A redelivered attempt reports the row as it stands rather than drawing again.
    if row.status != ProposalStatus::Pending.as_str() {
        return Ok(Finished {
            proposal_id: row.id,
            status: ProposalStatus::parse(&row.status).unwrap_or(ProposalStatus::Failed),
            already_finished: true,
            error: None,
        });
    }
    let settlement = decide(job, &row, source).await?;
    settle(job, settlement).await
}

/// The draft the row was requested for, or why it cannot be used. The draft is
/// edited in place, so one edited after the request is not what was asked to
/// be priced.
async fn load_config(
    db: &DatabaseConnection,
    row: &experiment_proposals::Model,
) -> Result<Result<ExperimentConfig, String>, PriceError> {
    let Some(design_id) = row.design_id else {
        return Ok(Err(format!("proposal {} names no design", row.id)));
    };
    let Some(design) = design_store::design_by_id(db, design_id).await? else {
        return Ok(Err(format!(
            "experiment `{}` has no design {design_id}",
            row.name
        )));
    };
    if row.design_hash.as_ref() != Some(&design.content_hash) {
        return Ok(Err(
            "the draft was edited after this was requested: price it again".to_string(),
        ));
    }
    Ok(design_store::decode(design).map(|d| d.config))
}

/// The revision the measures and views are read at: the promoted one when the
/// job runs. The design is a record; only its measures, views and automations
/// still live in git.
async fn promoted_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, PriceError> {
    Ok(entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await?
        .and_then(|w| w.current_revision_id))
}

/// Both levers must resolve to exactly one automation before anything else is
/// spent (spec, "Lever checks"): a refusal naming each side that did not.
async fn lever_refusal(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    cfg: &ExperimentConfig,
) -> Result<Option<String>, PriceError> {
    let lines = check_levers(db, workspace_id, cfg)
        .await
        .map_err(PriceError::Lever)?;
    Ok((!lines.is_empty()).then(|| {
        format!(
            "the lever cannot move, so there is nothing worth pricing: {}",
            lines.join("; ")
        )
    }))
}

/// Load, check the lever, read the warehouse once, price — everything up to (not
/// including) the write.
async fn decide(
    job: &PriceJob<'_>,
    row: &experiment_proposals::Model,
    source: &dyn PanelSource,
) -> Result<Settlement, PriceError> {
    let candidate: Candidate = serde_json::from_value(row.request.clone())?;
    let cfg = match load_config(job.db, row).await? {
        Ok(cfg) => cfg,
        Err(reason) => return refuse(reason),
    };
    if let Some(reason) = lever_refusal(job.db, row.workspace_id, &cfg).await? {
        return refuse(reason);
    }
    let Some(revision_id) = promoted_revision(job.db, row.workspace_id).await? else {
        return refuse(
            "the workspace has no promoted revision, so its measures cannot be read".to_string(),
        );
    };
    let ctx = PanelContext {
        workspace_id: row.workspace_id,
        revision_id,
        requested_by: row.requested_by,
    };
    let readings = match source
        .read(&ctx, &read_ask(&cfg, &candidate, job.now))
        .await
    {
        Ok(r) => r,
        Err(OxyError::ValidationError(reason)) => return refuse(reason),
        Err(other) => return Err(PriceError::Panel(other)),
    };
    price_readings(cfg, candidate, readings, job).await
}

async fn price_readings(
    cfg: ExperimentConfig,
    candidate: Candidate,
    readings: Readings,
    job: &PriceJob<'_>,
) -> Result<Settlement, PriceError> {
    let extra = read_warnings(&cfg, &readings, today_in(cfg.design.timezone, job.now));
    let Readings {
        panel,
        membership,
        prior,
        ..
    } = readings;
    let panel = match panel {
        PanelOutcome::Panel(p) | PanelOutcome::Thinned { panel: p, .. } => p,
        PanelOutcome::NoHistory { measure, entity } => {
            let reason =
                format!("no history for {measure} over {entity}: the panel query returned 0 rows");
            return Ok(Settlement::Refused {
                reason,
                warnings: extra,
            });
        }
        PanelOutcome::Refused { reason } => {
            return Ok(Settlement::Refused {
                reason,
                warnings: extra,
            });
        }
    };
    let run = RunParams {
        seed: fresh_seed(),
        now: job.now,
        iterations: job.iterations,
        membership,
    };
    // The placebo is CPU-bound: keep it off the runtime's workers.
    let computation =
        tokio::task::spawn_blocking(move || price_design(&cfg, &panel, &candidate, &run))
            .await
            .map_err(|e| PriceError::Panicked(e.to_string()))?;
    Ok(match computation {
        PriceComputation::Priced(priced) => Settlement::Done(Box::new(Done {
            priced,
            prior: prior.frozen(),
            extra,
        })),
        PriceComputation::Refused { reason, problems } => Settlement::Refused {
            reason: refusal_text(reason, &problems),
            warnings: extra,
        },
    })
}

/// The one `refusal` string index C5's `Proposal` carries: the reason, then one
/// line per problem when the file was what failed.
fn refusal_text(reason: String, problems: &[String]) -> String {
    if problems.is_empty() {
        reason
    } else {
        format!("{reason}: {}", problems.join("; "))
    }
}

/// A priced candidate as the columns of its row. A waves plan fills `waves`, a
/// switchback `periods`; the pricing core's warnings come before the reads'.
fn done_fields(done: Done) -> Result<DoneFields, serde_json::Error> {
    let Done {
        priced,
        prior,
        extra,
    } = done;
    let (waves, periods) = match &priced.plan {
        SchedulePlan::Waves(w) => (Some(serde_json::to_value(w)?), None),
        SchedulePlan::Switchback { periods, .. } => (None, Some(serde_json::to_value(periods)?)),
    };
    let mut all_warnings = priced.warnings;
    all_warnings.extend(extra);
    Ok(DoneFields {
        seed: priced.seed,
        waves,
        periods,
        strata: priced
            .strata
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?,
        assign_by: priced.assign_by,
        power: serde_json::to_value(&priced.power)?,
        mde_unit_day: priced.mde_unit_day,
        prior_coefficient: prior.as_ref().map(serde_json::to_value).transpose()?,
        warnings: all_warnings,
    })
}

async fn settle(job: &PriceJob<'_>, settlement: Settlement) -> Result<Finished, PriceError> {
    let (status, changed) = match settlement {
        Settlement::Done(done) => {
            let fields = done_fields(*done)?;
            (
                ProposalStatus::Done,
                proposal_store::finish_done(job.db, job.proposal_id, fields).await?,
            )
        }
        Settlement::Refused { reason, warnings } => (
            ProposalStatus::Refused,
            proposal_store::finish_refused(job.db, job.proposal_id, &reason, &warnings).await?,
        ),
    };
    tracing::info!(proposal_id = %job.proposal_id, status = status.as_str(), changed, "experiment proposal settled");
    Ok(Finished {
        proposal_id: job.proposal_id,
        status,
        already_finished: !changed,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use oxy_airlayer_compat::engine::experiment::PowerResult;
    use oxy_experiment::{LeverState, PriorSource};

    use super::*;
    use crate::server::experiment::snippet::{PeriodPlan, WavePlan};

    fn power() -> PowerResult {
        PowerResult {
            mde: 17.5,
            mde_relative: f64::INFINITY,
            baseline: 155.0,
            null_sd: 4.0,
            iterations: 100,
            distinct_windows: 90,
            independent_stretches: 5,
            refusal: None,
        }
    }

    fn priced(plan: SchedulePlan) -> Priced {
        Priced {
            seed: 8841207,
            alpha: 0.05,
            assign_by: Some("sku.category".to_string()),
            plan,
            strata: Some(vec![vec!["a".to_string()], vec!["b".to_string()]]),
            power: power(),
            mde_unit_day: 3.5,
            warnings: vec!["from pricing".to_string()],
        }
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, d).unwrap()
    }

    #[test]
    fn a_drawn_seed_is_always_a_javascript_safe_integer() {
        for _ in 0..2000 {
            assert!(fresh_seed() < (1u64 << 53));
        }
    }

    #[test]
    fn a_validation_refusal_folds_its_problems_into_one_line() {
        assert_eq!(refusal_text("nothing".to_string(), &[]), "nothing");
        assert_eq!(
            refusal_text(
                "2 problems".to_string(),
                &["a".to_string(), "b".to_string()]
            ),
            "2 problems: a; b"
        );
    }

    #[test]
    fn a_waves_plan_fills_waves_and_leaves_periods_null() {
        let plan = SchedulePlan::Waves(vec![WavePlan {
            switch_on: day(7).into(),
            units: vec!["east".to_string()],
        }]);
        let prior = Some(PriorCoefficient {
            value: 4.2,
            source: PriorSource::Fitted,
        });
        let f = done_fields(Done {
            priced: priced(plan),
            prior,
            extra: vec!["from reads".to_string()],
        })
        .unwrap();
        assert_eq!(f.waves.unwrap()[0]["switch_on"], "2026-09-07");
        assert!(f.periods.is_none());
        assert_eq!(f.strata.unwrap()[1][0], "b");
        assert_eq!(f.assign_by.as_deref(), Some("sku.category"));
        assert_eq!(
            f.prior_coefficient.unwrap(),
            serde_json::json!({"value": 4.2, "source": "fitted"})
        );
        assert_eq!(
            f.warnings,
            ["from pricing", "from reads"],
            "the pricing core's warnings come first"
        );
        assert!(
            f.power["mde_relative"].is_null(),
            "an infinite value is stored as null"
        );
        assert_eq!((f.seed, f.mde_unit_day), (8841207, 3.5));
    }

    #[test]
    fn a_switchback_plan_fills_periods_and_leaves_waves_null() {
        let plan = SchedulePlan::Switchback {
            period_days: 7,
            periods: vec![
                PeriodPlan {
                    from: day(7).into(),
                    state: LeverState::On,
                },
                PeriodPlan {
                    from: day(14).into(),
                    state: LeverState::Off,
                },
            ],
        };
        let f = done_fields(Done {
            priced: priced(plan),
            prior: None,
            extra: vec![],
        })
        .unwrap();
        assert!(f.waves.is_none());
        assert_eq!(
            f.periods.unwrap()[1],
            serde_json::json!({"from": "2026-09-14", "state": "off"})
        );
        assert!(f.prior_coefficient.is_none());
    }
}
