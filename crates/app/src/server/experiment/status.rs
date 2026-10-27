//! `GET /experiments/{name}/run`. Operations only: the effect stays sealed
//! until `reported`, and nothing here computes one. The abort request lives in
//! [`super::abort`] and the flip tallies in [`super::status_flips`]; both are
//! re-exported here.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use entity::{experiment_runs, experiments};
use oxy_experiment::ExperimentConfig;
use oxy_experiment::Moment;
use oxy_experiment::compliance::ComplianceSummary;
use oxy_experiment::run::{AbortLevers, RunState, switch_moments, units_on};
use oxy_experiment::timeline::local_hour_in;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr};
use serde::Serialize;
use uuid::Uuid;

pub use super::abort::{AbortOutcome, OwnerAbort, request_abort};
use super::design_store;
use super::jobs::{CHECK_TASK_INTERVAL_SECS, EstimateState, estimate_state};
use super::panel::GroupMembership;
pub use super::record::compliance_outcomes;
use super::register::{self, Registration};
use super::retrospective::{self, Retrospective};
pub use super::status_flips::{
    FailedFlip, FlipCounts, PeriodProgress, compliance, lever_revisions, periods,
};
use super::status_flips::{counts, failed};
use super::store::{self, GuardrailStatus};
pub use super::summary::ExperimentStatus;
use super::summary::next_switch;
use super::tick::flip_facts;
use crate::server::router::recovery::DEFAULT_INPROC_GLOBAL_WORKER_INTERVAL_SECS;

impl From<RunState> for ExperimentStatus {
    fn from(s: RunState) -> Self {
        match s {
            RunState::Registered => Self::Registered,
            RunState::Live => Self::Live,
            RunState::Closing => Self::Closing,
            RunState::Reported => Self::Reported,
            RunState::Aborted => Self::Aborted,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunStatusBody {
    pub name: String,
    pub status: ExperimentStatus,
    pub registration_refusal: Option<String>,
    /// No run yet, and the latest version's dates are all past: it can be analysed
    /// retrospectively (`POST /experiments/{name}/retrospective`).
    pub retrospective_ready: bool,
    /// No run yet, and the latest version is complete but its last window is not over.
    pub retrospective_refusal: Option<String>,
    /// Distinct `experiment_flips.lever_revision_id`s in recording order.
    pub lever_revisions: Vec<Uuid>,
    pub run: Option<RunDetail>,
}

#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub state: RunState,
    /// The design the run executes, frozen at `registered_at`.
    pub design_id: Uuid,
    pub registered_at: DateTime<Utc>,
    pub registered_by: Uuid,
    /// Created from past data: no lever moved, reported as not pre-registered.
    pub retrospective: bool,
    pub next_switch_on: Option<Moment>,
    pub flips: FlipCounts,
    pub failed_flips: Vec<FailedFlip>,
    pub compliance: Option<ComplianceSummary>,
    pub coverage: Coverage,
    pub guardrails: Vec<GuardrailStatus>,
    pub guardrail_breach: Option<String>,
    pub abort_reason: Option<String>,
    /// What the abort does (or did) with the levers: the owner's choice while it is
    /// requested, what it did once the run is aborted. `None` until one is requested.
    pub abort_levers: Option<AbortLevers>,
    /// The day the run became `aborted`; its report's measurement ends there.
    pub aborted_on: Option<NaiveDate>,
    /// The units whose lever is on now: what an abort that turns levers off switches.
    pub units_on: Vec<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub problems: Vec<String>,
    pub next_check_at: DateTime<Utc>,
    pub not_advancing: bool,
    pub design: DesignKind,
    /// A switchback's periods with each one's flip progress; `None` for waves.
    pub periods: Option<Vec<PeriodProgress>>,
    pub assign_by: Option<String>,
    /// Every group → its members, as the first check pinned them.
    pub group_membership: Option<GroupMembership>,
    /// The estimate job: `none`, `running` (queued or running), `done`, or
    /// `failed` with its `task_status`, the agentic run's error if any, when it
    /// failed and how many of `jobs::MAX_ESTIMATE_ATTEMPTS` runs were used.
    pub estimate: EstimateState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignKind {
    Waves,
    Switchback,
}

#[derive(Debug, Serialize)]
pub struct Coverage {
    pub last_window: Option<f64>,
    pub floor: f64,
}

pub fn not_advancing(state: RunState, next_check_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    let stale = Duration::seconds(2 * DEFAULT_INPROC_GLOBAL_WORKER_INTERVAL_SECS as i64);
    !state.is_terminal() && next_check_at < now - stale
}

/// The next wave date — or switchback period boundary — not yet past.
pub fn next_switch_on(c: &ExperimentConfig, state: RunState, now: Moment) -> Option<Moment> {
    if !matches!(state, RunState::Registered | RunState::Live) {
        return None;
    }
    next_switch(c, now)
}

/// What the first check pinned, as the status sees it.
pub struct Pins<'a> {
    pub pool: Option<&'a [String]>,
    pub membership: Option<&'a GroupMembership>,
    pub fleet: Option<&'a [String]>,
}

/// What a check could not do yet that the status should say: a run past its
/// first switch with its fleet (switchback), group membership (`assign_by`) or
/// control pool (otherwise) not pinned has read nothing of the control arm and
/// sent nothing that depends on it. Only while the run can still pin it.
fn waiting(c: &ExperimentConfig, state: RunState, pins: &Pins<'_>, now: Moment) -> Vec<String> {
    let begun = switch_moments(c).first().is_some_and(|m| *m <= now);
    if state.is_terminal() || !begun {
        return Vec::new();
    }
    let problem = if c.design.switchback.is_some() {
        pins.fleet.is_none().then_some(
            "the fleet is not pinned yet: no flip goes out until the first check pins it",
        )
    } else if c.assign_by.is_some() {
        pins.membership.is_none().then_some(
            "the group membership is not pinned yet: no group flip goes out until the first \
             check pins it",
        )
    } else {
        pins.pool.is_none().then_some(
            "the control pool is not pinned yet: no control is read until a check pins it",
        )
    };
    problem.map(str::to_string).into_iter().collect()
}

/// A live run whose last check is older than two check intervals: the job is
/// failing, or not running at all. A run never checked is measured from
/// registration.
fn stale_check(
    state: RunState,
    checked_at: Option<DateTime<Utc>>,
    registered_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<String> {
    let limit = Duration::seconds(2 * CHECK_TASK_INTERVAL_SECS);
    let since = checked_at.unwrap_or(registered_at);
    (state == RunState::Live && since < now - limit).then(|| match checked_at {
        Some(at) => format!("the last check completed at {at}; checks are due hourly"),
        None => "no check has completed since registration; checks are due hourly".to_string(),
    })
}

pub async fn run_status(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    now: DateTime<Utc>,
) -> Result<Option<RunStatusBody>, DbErr> {
    let Some(experiment) = design_store::experiment_by_name(db, workspace_id, name).await? else {
        return Ok(None);
    };
    Ok(Some(match store::run_of(db, experiment.id).await? {
        Some(run) => registered(db, &run, now).await?,
        None => unregistered(db, &experiment, now).await?,
    }))
}

/// What registering the draft would get, at `now`.
pub async fn registration_of(
    db: &impl ConnectionTrait,
    experiment: &experiments::Model,
    now: DateTime<Utc>,
) -> Result<Registration, DbErr> {
    let Some(latest) = design_store::design_of(db, experiment.id).await? else {
        return Ok(Registration::Draft {
            problems: vec!["the experiment has no design".into()],
        });
    };
    match design_store::decode(latest) {
        Ok(d) => register::decide(db, experiment.workspace_id, &d.config, now).await,
        Err(problem) => Ok(Registration::Draft {
            problems: vec![problem],
        }),
    }
}

/// What a retrospective of the draft would get, at `now`.
async fn retrospective_of(
    db: &impl ConnectionTrait,
    experiment: &experiments::Model,
    now: DateTime<Utc>,
) -> Result<Retrospective, DbErr> {
    let Some(latest) = design_store::design_of(db, experiment.id).await? else {
        return Ok(Retrospective::Draft {
            problems: vec!["the experiment has no design".into()],
        });
    };
    Ok(match design_store::decode(latest) {
        Ok(d) => retrospective::judge(&d.config, now),
        Err(problem) => Retrospective::Draft {
            problems: vec![problem],
        },
    })
}

async fn unregistered(
    db: &DatabaseConnection,
    experiment: &experiments::Model,
    now: DateTime<Utc>,
) -> Result<RunStatusBody, DbErr> {
    let (status, registration_refusal) = match registration_of(db, experiment, now).await? {
        Registration::Draft { .. } => (ExperimentStatus::Draft, None),
        Registration::Refused { reason } => (ExperimentStatus::RegistrationRefused, Some(reason)),
        Registration::Ready => (ExperimentStatus::Registrable, None),
    };
    let (retrospective_ready, retrospective_refusal) =
        match retrospective_of(db, experiment, now).await? {
            Retrospective::Draft { .. } => (false, None),
            Retrospective::Refused { reason } => (false, Some(reason)),
            Retrospective::Ready => (true, None),
        };
    Ok(RunStatusBody {
        name: experiment.name.clone(),
        status,
        registration_refusal,
        retrospective_ready,
        retrospective_refusal,
        lever_revisions: Vec::new(),
        run: None,
    })
}

async fn registered(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    now: DateTime<Utc>,
) -> Result<RunStatusBody, DbErr> {
    let state = RunState::parse(&run.state)
        .ok_or_else(|| DbErr::Custom(format!("unknown run state {:?}", run.state)))?;
    let design = store::pinned(db, run).await.map_err(DbErr::Custom)?;
    let c = design.config;
    let flips = store::flips_of(db, run.id).await?;
    let obs = store::observations(run);
    let membership = store::group_membership(run);
    let (local, next_check_at) = (
        local_hour_in(c.design.timezone, now),
        run.next_check_at.with_timezone(&Utc),
    );
    let pool = store::control_pool(run);
    let fleet = store::fleet(run);
    let detail = RunDetail {
        state,
        design_id: run.design_id,
        registered_at: run.registered_at.with_timezone(&Utc),
        registered_by: run.registered_by,
        retrospective: run.retrospective,
        next_switch_on: next_switch_on(&c, state, local),
        flips: counts(&flips),
        failed_flips: failed(&flips),
        compliance: compliance(
            &c,
            &flips,
            pool.as_deref(),
            membership.as_ref(),
            fleet.as_deref(),
            &obs,
        ),
        coverage: Coverage {
            last_window: obs.coverage,
            floor: c.design.coverage_floor,
        },
        guardrails: obs.guardrails.clone(),
        guardrail_breach: run.guardrail_breach.clone(),
        abort_reason: run.abort_reason.clone(),
        abort_levers: store::abort_levers(run),
        aborted_on: run.aborted_on,
        units_on: units_on(&c, &flip_facts(&flips)),
        checked_at: obs.checked_at,
        problems: obs
            .problems
            .iter()
            .cloned()
            .chain(
                waiting(
                    &c,
                    state,
                    &Pins {
                        pool: pool.as_deref(),
                        membership: membership.as_ref(),
                        fleet: fleet.as_deref(),
                    },
                    local,
                )
                .into_iter()
                // A retrospective run is never checked: its estimate pins the pool.
                .filter(|_| !run.retrospective),
            )
            .chain(stale_check(
                state,
                obs.checked_at,
                run.registered_at.with_timezone(&Utc),
                now,
            ))
            .collect(),
        next_check_at,
        not_advancing: not_advancing(state, next_check_at, now),
        design: if c.design.switchback.is_some() {
            DesignKind::Switchback
        } else {
            DesignKind::Waves
        },
        periods: periods(&c, &flips),
        assign_by: c.assign_by.clone(),
        group_membership: membership,
        estimate: estimate_state(db, run).await?,
    };
    Ok(RunStatusBody {
        name: c.experiment.clone(),
        status: state.into(),
        registration_refusal: None,
        retrospective_ready: false,
        retrospective_refusal: None,
        lever_revisions: lever_revisions(&flips),
        run: Some(detail),
    })
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use entity::experiment_flips;
    use oxy_experiment::run::LeverState;

    use super::*;
    use crate::server::experiment::test_design::{design, switchback};

    #[test]
    fn a_run_two_driver_intervals_stale_is_not_advancing_unless_terminal() {
        let now = Utc::now();
        let interval = Duration::seconds(DEFAULT_INPROC_GLOBAL_WORKER_INTERVAL_SECS as i64);
        assert!(!not_advancing(RunState::Live, now - interval, now));
        assert!(!not_advancing(RunState::Live, now - interval * 2, now));
        assert!(not_advancing(
            RunState::Live,
            now - interval * 2 - Duration::seconds(1),
            now
        ));
        assert!(!not_advancing(
            RunState::Aborted,
            now - Duration::days(3),
            now
        ));
    }

    #[test]
    fn the_next_switch_is_the_first_switch_day_not_yet_past_while_running() {
        let c = design();
        let d = |s: &str| s.parse::<NaiveDate>().unwrap();
        assert_eq!(
            next_switch_on(&c, RunState::Registered, d("2026-02-20").into()),
            Some(d("2026-03-02")).map(oxy_experiment::Moment::from)
        );
        assert_eq!(
            next_switch_on(&c, RunState::Live, d("2026-03-02").into()),
            Some(d("2026-03-02")).map(oxy_experiment::Moment::from)
        );
        assert_eq!(
            next_switch_on(&c, RunState::Live, d("2026-03-03").into()),
            Some(d("2026-03-16")).map(oxy_experiment::Moment::from)
        );
        assert_eq!(
            next_switch_on(&c, RunState::Live, d("2026-03-17").into()),
            None
        );
        assert_eq!(
            next_switch_on(&c, RunState::Closing, d("2026-02-20").into()),
            None
        );
        assert_eq!(
            next_switch_on(&switchback(), RunState::Live, d("2026-03-10").into()),
            Some(d("2026-03-16")).map(oxy_experiment::Moment::from),
            "a switchback's next period boundary"
        );
    }

    fn pins<'a>(
        pool: Option<&'a [String]>,
        membership: Option<&'a GroupMembership>,
        fleet: Option<&'a [String]>,
    ) -> Pins<'a> {
        Pins {
            pool,
            membership,
            fleet,
        }
    }

    #[test]
    fn an_unpinned_fleet_is_reported_only_while_the_run_can_still_pin_it() {
        let c = switchback();
        let today = "2026-03-10".parse::<NaiveDate>().unwrap();
        let none = pins(None, None, None);
        assert_eq!(waiting(&c, RunState::Live, &none, today.into()).len(), 1);
        assert!(
            waiting(&c, RunState::Aborted, &none, today.into()).is_empty(),
            "aborted before the first check"
        );
        assert!(waiting(&c, RunState::Reported, &none, today.into()).is_empty());
        let fleet = ["bondi".to_string()];
        assert!(
            waiting(
                &c,
                RunState::Live,
                &pins(None, None, Some(&fleet)),
                today.into()
            )
            .is_empty()
        );
    }

    #[test]
    fn an_unpinned_control_pool_is_reported_past_the_first_switch() {
        let c = design();
        let d = |s: &str| s.parse::<NaiveDate>().unwrap();
        let none = pins(None, None, None);
        let pool = ["glebe".to_string()];
        assert!(
            waiting(&c, RunState::Live, &none, d("2026-03-01").into()).is_empty(),
            "before the first switch there is nothing to read yet"
        );
        let found = waiting(&c, RunState::Live, &none, d("2026-03-02").into());
        assert!(found[0].contains("control pool"), "{found:?}");
        assert!(
            waiting(
                &c,
                RunState::Live,
                &pins(Some(&pool), None, None),
                d("2026-03-02").into()
            )
            .is_empty()
        );
        assert!(waiting(&c, RunState::Aborted, &none, d("2026-03-02").into()).is_empty());
        assert!(waiting(&c, RunState::Reported, &none, d("2026-03-02").into()).is_empty());
    }

    #[test]
    fn unpinned_group_membership_is_reported_under_assign_by() {
        let mut c = design();
        c.assign_by = Some("region".into());
        let today = "2026-03-02".parse::<NaiveDate>().unwrap();
        let pool = ["glebe".to_string()];
        let members = GroupMembership::default();
        let found = waiting(
            &c,
            RunState::Live,
            &pins(Some(&pool), None, None),
            today.into(),
        );
        assert!(found[0].contains("group membership"), "{found:?}");
        assert!(
            waiting(
                &c,
                RunState::Live,
                &pins(Some(&pool), Some(&members), None),
                today.into()
            )
            .is_empty()
        );
        assert!(waiting(&c, RunState::Closing, &pins(None, None, None), today.into()).len() == 1);
        assert!(waiting(&c, RunState::Aborted, &pins(None, None, None), today.into()).is_empty());
    }

    #[test]
    fn a_live_run_whose_check_is_two_intervals_old_is_stale() {
        let now = Utc::now();
        let hour = Duration::seconds(CHECK_TASK_INTERVAL_SECS);
        let registered = now - Duration::days(10);
        let at = |checked| stale_check(RunState::Live, checked, registered, now);
        assert!(at(Some(now - hour * 2)).is_none());
        assert!(at(Some(now - hour * 2 - Duration::seconds(1))).is_some());
        assert!(at(None).is_some(), "never checked, registered long ago");
        assert!(
            stale_check(RunState::Live, None, now - hour, now).is_none(),
            "just registered"
        );
        for state in [RunState::Aborted, RunState::Reported, RunState::Registered] {
            assert!(stale_check(state, Some(now - hour * 9), registered, now).is_none());
        }
    }

    #[test]
    fn a_switchbacks_periods_count_their_own_flips() {
        let row = |unit: &str, state: &str, due: &str, status: &str| experiment_flips::Model {
            id: Uuid::new_v4(),
            run_id: Uuid::nil(),
            unit: unit.into(),
            target_state: state.into(),
            due_at: due.parse::<oxy_experiment::Moment>().unwrap().at(),
            wave: 0,
            task_id: (status != "pending").then(Uuid::nil),
            agentic_run_id: None,
            lever_revision_id: None,
            status: status.into(),
            confirmed_on: None,
            reason: None,
            created_at: Utc::now().fixed_offset(),
        };
        let flips = [
            row("bondi", "on", "2026-03-02", "confirmed"),
            row("glebe", "on", "2026-03-02", "dispatched"),
            row("bondi", "off", "2026-03-09", "pending"),
            row("bondi", "off", "2026-03-11", "pending"), // a terminal off: no period's
        ];
        let p = periods(&switchback(), &flips).expect("a switchback has periods");
        assert_eq!(p.len(), 4);
        assert_eq!((p[0].flips.sent, p[0].flips.confirmed), (2, 1));
        assert_eq!((p[1].state, p[1].flips.pending), (LeverState::Off, 1));
        assert!(
            periods(&design(), &flips).is_none(),
            "a waves design has none"
        );
    }
}
