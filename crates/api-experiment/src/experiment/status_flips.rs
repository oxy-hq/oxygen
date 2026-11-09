//! The run's flips as the status reports them: counts, failures, the lever
//! revisions they resolved at, a switchback's per-period progress and the
//! compliance so far. Pure over `experiment_flips` rows; `status` re-exports
//! the public items.

use entity::experiment_flips;
use oxy_experiment::ExperimentConfig;
use oxy_experiment::Moment;
use oxy_experiment::compliance::{ComplianceSummary, FlipOutcome, summarize};
use oxy_experiment::run::LeverState;
use serde::Serialize;
use uuid::Uuid;

use super::panel::GroupMembership;
use super::record::compliance_outcomes;
use super::store::Observations;

#[derive(Debug, Serialize)]
pub struct PeriodProgress {
    pub index: usize,
    pub from: Moment,
    pub state: LeverState,
    pub flips: FlipCounts,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct FlipCounts {
    pub pending: usize,
    pub sent: usize,
    pub confirmed: usize,
    pub unconfirmed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

#[derive(Debug, Serialize)]
pub struct FailedFlip {
    pub unit: String,
    pub state: String,
    pub reason: String,
}

/// A switchback's periods with the flips due at each boundary (a terminal
/// `off` belongs to none). `None` for a waves design.
pub fn periods(
    c: &ExperimentConfig,
    flips: &[experiment_flips::Model],
) -> Option<Vec<PeriodProgress>> {
    let sb = c.design.switchback.as_ref()?;
    let of = |from: Moment, state: LeverState| {
        flips.iter().filter(move |f| {
            Moment(f.due_at) == from && LeverState::parse(&f.target_state) == Some(state)
        })
    };
    let progress = sb
        .periods
        .iter()
        .enumerate()
        .map(|(index, p)| PeriodProgress {
            index,
            from: p.from,
            state: p.state,
            flips: counts(of(p.from, p.state)),
        });
    Some(progress.collect())
}

pub(super) fn counts<'a>(
    flips: impl IntoIterator<Item = &'a experiment_flips::Model>,
) -> FlipCounts {
    let mut n = FlipCounts::default();
    for f in flips {
        n.sent += usize::from(f.task_id.is_some());
        match f.status.as_str() {
            "pending" => n.pending += 1,
            "confirmed" => n.confirmed += 1,
            "unconfirmed" => n.unconfirmed += 1,
            "failed" => n.failed += 1,
            "cancelled" => n.cancelled += 1,
            _ => {}
        }
    }
    n
}

/// The promoted revisions the flips' levers resolved at, distinct, in the order
/// the flips were recorded (`store::flips_of` orders by `created_at`) — which is
/// dispatch order, except an `off` that waited for its `on`. More than one means
/// the lever automation changed mid-run. PR 5's report reads it through this
/// function over `store::flips_of(db, run.id)`.
pub fn lever_revisions(flips: &[experiment_flips::Model]) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = Vec::new();
    for revision in flips.iter().filter_map(|f| f.lever_revision_id) {
        if !out.contains(&revision) {
            out.push(revision);
        }
    }
    out
}

pub(super) fn failed(flips: &[experiment_flips::Model]) -> Vec<FailedFlip> {
    flips
        .iter()
        .filter(|f| f.status == "failed")
        .map(|f| FailedFlip {
            unit: f.unit.clone(),
            state: f.target_state.clone(),
            reason: f.reason.clone().unwrap_or_default(),
        })
        .collect()
}

/// Compliance so far; `None` until a treated design unit is decided.
pub fn compliance(
    c: &ExperimentConfig,
    flips: &[experiment_flips::Model],
    pool: Option<&[String]>,
    membership: Option<&GroupMembership>,
    fleet: Option<&[String]>,
    obs: &Observations,
) -> Option<ComplianceSummary> {
    let outcomes = compliance_outcomes(c, flips, pool, membership, fleet, obs, false);
    let treated = |o: &FlipOutcome| {
        matches!(
            o,
            FlipOutcome::Landed { .. } | FlipOutcome::NeverLanded | FlipOutcome::Failed { .. }
        )
    };
    outcomes
        .iter()
        .any(|(_, o)| treated(o))
        .then(|| summarize(c, &outcomes))
}
