//! The run state machine as a pure function of (design, state, facts).
//!
//! Nothing here reads a file: `RunFacts` carries dates, recorded flips, the
//! pinned fleet, guardrail and abort facts, and the estimate's completion —
//! never a design edit. The host always passes the design pinned at
//! registration, so the platform actuates only on dates and guardrails.

use std::collections::BTreeMap;

use crate::moment::Moment;
use serde::{Deserialize, Serialize};

pub use crate::config::LeverState;
use crate::config::{ExperimentConfig, OnClose, SwitchbackConfig};
use crate::timeline::last_post_window_end;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Registered,
    Live,
    Closing,
    Reported,
    Aborted,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Registered => "registered",
            RunState::Live => "live",
            RunState::Closing => "closing",
            RunState::Reported => "reported",
            RunState::Aborted => "aborted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Registered,
            Self::Live,
            Self::Closing,
            Self::Reported,
            Self::Aborted,
        ]
        .into_iter()
        .find(|state| state.as_str() == s)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, RunState::Reported | RunState::Aborted)
    }
}

/// The `experiment_flips.target_state` strings. The enum is PR 2's (its
/// switchback periods need it); this impl sits beside the only code that
/// writes the column.
impl LeverState {
    pub fn as_str(self) -> &'static str {
        match self {
            LeverState::On => "on",
            LeverState::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "on" => Some(LeverState::On),
            "off" => Some(LeverState::Off),
            _ => None,
        }
    }
}

/// What an abort does with the levers: turn every treated unit off (the default), or
/// leave each one as it is and stop managing it. The owner chooses; a guardrail abort
/// always turns them off. Stored as `experiment_runs.abort_levers`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortLevers {
    #[default]
    Off,
    Keep,
}

impl AbortLevers {
    pub fn as_str(self) -> &'static str {
        match self {
            AbortLevers::Off => "off",
            AbortLevers::Keep => "keep",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Off, Self::Keep]
            .into_iter()
            .find(|l| l.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunFacts {
    /// Now in `design.timezone`, floored to the hour: every switch is a whole hour
    /// (a midnight below hour grain), so a flip is due once this reaches it.
    pub now: Moment,
    /// Flip rows already recorded, dispatched or not.
    pub flips: Vec<FlipFact>,
    /// A switchback's fleet — every unit it flips — as the first check pinned
    /// it (`experiment_runs.fleet`). `None` until pinned. A waves design never
    /// reads it: its treated units are in the file.
    pub pinned_units: Option<Vec<String>>,
    pub last_window_coverage: Option<f64>,
    /// When the run entered `closing`, in `design.timezone`.
    pub closing_since: Option<Moment>,
    pub guardrail_breach: Option<String>,
    /// The owner's reason.
    pub abort_requested: Option<String>,
    /// Until the run is aborted, what the owner asked for with the reason; from then
    /// on what the abort did ([`abort_levers`], recorded by the host on the way in).
    pub abort_levers: AbortLevers,
    pub estimate_done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlipFact {
    pub unit: String,
    pub state: LeverState,
    /// The moment the flip was due: with `unit` and `state`, its identity.
    pub due_at: Moment,
    /// Its lever job was enqueued (`experiment_flips.task_id` is set).
    pub dispatched: bool,
    /// It was cancelled and never sent (`experiment_flips.status = 'cancelled'`):
    /// it records no lever state, so it never stands in for another flip.
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunAction {
    /// One per unit per wave; under a switchback one per pinned unit per
    /// period boundary, `wave` being the period's index. `due_at` is the
    /// wave's `switch_on`, the period's `from`, or — for a terminal `off`
    /// (abort, close) — the day it was decided. `per: wave` batching is the
    /// host's (PR 4 dispatch), not the state machine's.
    DispatchFlip {
        unit: String,
        state: LeverState,
        wave: usize,
        due_at: Moment,
    },
    EnqueueEstimate,
    NotifyOwner {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub next: RunState,
    pub actions: Vec<RunAction>,
}

/// How long `closing` waits for the last post window to reach
/// `coverage_floor` before estimating on what arrived. A platform constant,
/// not a key: a design cannot buy itself a longer wait.
pub const CLOSING_MAX_WAIT_DAYS: i64 = 14;

pub fn step(c: &ExperimentConfig, state: RunState, f: &RunFacts) -> Transition {
    match state {
        RunState::Registered | RunState::Live => step_active(c, state, f),
        RunState::Closing => step_closing(c, f),
        RunState::Reported => to(RunState::Reported, Vec::new()),
        RunState::Aborted => step_aborted(c, f),
    }
}

/// An aborted run is estimated too (its report is labelled aborted). Enqueued
/// from here, never on the step into `aborted`: the job could be claimed before
/// the host writes the new state, and the executor refuses a `live` run. The
/// host's enqueue is a CAS, so emitting it every step is harmless.
///
/// An abort that kept the levers emits no `off` here either, even if a check that was
/// already running records a breach afterwards: the run no longer manages its units.
fn step_aborted(c: &ExperimentConfig, f: &RunFacts) -> Transition {
    let mut actions = match f.abort_levers {
        AbortLevers::Off => abort_offs(c, f),
        AbortLevers::Keep => Vec::new(),
    };
    if !f.estimate_done {
        actions.push(RunAction::EnqueueEstimate);
    }
    to(RunState::Aborted, actions)
}

/// Every day the design switches something, ascending: the waves'
/// `switch_on` dates, or the switchback's period starts.
pub fn switch_moments(c: &ExperimentConfig) -> Vec<Moment> {
    let mut days: Vec<Moment> = match &c.design.switchback {
        Some(sb) => sb.periods.iter().map(|p| p.from).collect(),
        None => c.design.waves.iter().map(|w| w.switch_on).collect(),
    };
    days.sort();
    days.dedup();
    days
}

fn to(next: RunState, actions: Vec<RunAction>) -> Transition {
    Transition { next, actions }
}

fn step_active(c: &ExperimentConfig, state: RunState, f: &RunFacts) -> Transition {
    if let Some(reason) = abort_reason(f) {
        let mut actions = match abort_levers(f) {
            AbortLevers::Off => abort_offs(c, f),
            AbortLevers::Keep => Vec::new(),
        };
        actions.push(RunAction::NotifyOwner { reason });
        return to(RunState::Aborted, actions);
    }
    // `last_post_window_end` is exclusive: the run closes ON that date.
    if state == RunState::Live && last_post_window_end(c).is_some_and(|end| f.now >= end) {
        // The levers go off on entering `closing`: the estimate may never
        // finish, and a lever left on silently is worse than a late report.
        return to(RunState::Closing, close_offs(c, f));
    }
    let started = switch_moments(c).first().is_some_and(|day| *day <= f.now);
    let next = if state == RunState::Live || started {
        RunState::Live
    } else {
        RunState::Registered
    };
    to(next, scheduled_flips(c, f))
}

fn abort_reason(f: &RunFacts) -> Option<String> {
    if let Some(breach) = &f.guardrail_breach {
        return Some(format!("guardrail breached: {breach}"));
    }
    let kept = match f.abort_levers {
        AbortLevers::Off => "",
        AbortLevers::Keep => ", levers left as they were",
    };
    f.abort_requested
        .as_ref()
        .map(|reason| format!("aborted on request{kept}: {reason}"))
}

/// What an abort decided now does with the levers — the host records it on the step into
/// `aborted`. A guardrail breach always turns them off, whatever the owner asked.
pub fn abort_levers(f: &RunFacts) -> AbortLevers {
    if f.guardrail_breach.is_some() {
        AbortLevers::Off
    } else {
        f.abort_levers
    }
}

/// The units whose lever is on now (its latest `on` was sent and no `off` followed):
/// what an abort that turns the levers off would switch.
pub fn units_on(c: &ExperimentConfig, flips: &[FlipFact]) -> Vec<String> {
    on_now(c, flips)
        .into_iter()
        .map(|(_, unit, _)| unit.to_string())
        .collect()
}

/// `closing` keeps emitting any `on_close: off` still owed (`needs_off` makes
/// it idempotent: an off that was never recorded — a crash between the decision
/// and the write — is re-emitted here. A recorded `off` counts as done even if
/// its send later failed, so a failed one is not retried by this step).
/// A retrospective run's step: the design was written after its data existed, so the
/// run moved no lever and owes no `off`, and its last window is history, so it waits on
/// no coverage. It is created `closing`; it estimates at once and is reported when the
/// estimate is done. Whatever state it is in, it never emits a flip.
pub fn step_retrospective(state: RunState, f: &RunFacts) -> Transition {
    match state {
        RunState::Closing if f.estimate_done => to(RunState::Reported, Vec::new()),
        RunState::Closing => to(RunState::Closing, vec![RunAction::EnqueueEstimate]),
        other => to(other, Vec::new()),
    }
}

fn step_closing(c: &ExperimentConfig, f: &RunFacts) -> Transition {
    let mut actions = close_offs(c, f);
    if f.estimate_done {
        return to(RunState::Reported, actions);
    }
    // With a carryover window the levers went off at close and the days after are
    // read too: nothing is estimated until the window's last day has arrived, and
    // the max wait runs from there.
    let carryover = c.design.carryover_days.unwrap_or(0);
    let since = f
        .closing_since
        .map_or(f.now, |d| c.design.grain.later(d, carryover).unwrap_or(d));
    if f.now < since {
        return to(RunState::Closing, actions);
    }
    let floor = c.design.coverage_floor;
    if f.last_window_coverage.is_some_and(|cov| cov >= floor) {
        actions.push(RunAction::EnqueueEstimate);
        return to(RunState::Closing, actions);
    }
    if (f.now.at() - since.at()).num_days() < CLOSING_MAX_WAIT_DAYS {
        return to(RunState::Closing, actions);
    }
    // Stable wording (no counts that move daily), so the host sends it once.
    let reason = format!(
        "the last post window's coverage stayed below coverage_floor {floor:.2} for \
         {CLOSING_MAX_WAIT_DAYS} days in closing; estimating on the data present"
    );
    actions.push(RunAction::EnqueueEstimate);
    actions.push(RunAction::NotifyOwner { reason });
    to(RunState::Closing, actions)
}

fn flip(unit: &str, state: LeverState, wave: usize, due_at: Moment) -> RunAction {
    RunAction::DispatchFlip {
        unit: unit.to_string(),
        state,
        wave,
        due_at,
    }
}

fn scheduled_flips(c: &ExperimentConfig, f: &RunFacts) -> Vec<RunAction> {
    match &c.design.switchback {
        Some(sb) => period_flips(sb, f),
        None => wave_on_flips(c, f),
    }
}

/// `(wave index, unit, switch_on)` for every treated unit of a waves design,
/// in design order.
fn treated(c: &ExperimentConfig) -> impl Iterator<Item = (usize, &str, Moment)> {
    c.design
        .waves
        .iter()
        .enumerate()
        .flat_map(|(i, w)| w.units.iter().map(move |u| (i, u.as_str(), w.switch_on)))
}

fn wave_on_flips(c: &ExperimentConfig, f: &RunFacts) -> Vec<RunAction> {
    let has_on = |unit: &str| {
        f.flips
            .iter()
            .any(|x| x.unit == unit && x.state == LeverState::On)
    };
    treated(c)
        .filter(|(_, _, day)| *day <= f.now)
        .filter(|(_, unit, _)| !has_on(unit))
        .map(|(wave, unit, day)| flip(unit, LeverState::On, wave, day))
        .collect()
}

/// Every pinned unit at every period boundary reached, once — `due_at` is
/// what tells a unit's two `on`s apart. Nothing until the fleet is pinned.
fn period_flips(sb: &SwitchbackConfig, f: &RunFacts) -> Vec<RunAction> {
    let Some(units) = &f.pinned_units else {
        return Vec::new();
    };
    let recorded = |unit: &str, state: LeverState, day: Moment| {
        f.flips
            .iter()
            .any(|x| x.unit == unit && x.state == state && x.due_at == day)
    };
    let mut out = Vec::new();
    for (k, p) in sb
        .periods
        .iter()
        .enumerate()
        .filter(|(_, p)| p.from <= f.now)
    {
        for unit in units.iter().filter(|u| !recorded(u, p.state, p.from)) {
            out.push(flip(unit, p.state, k, p.from));
        }
    }
    out
}

/// Each unit the lever may have reached, with the wave and day of its latest
/// `on`: a waves design's treated units at their wave (recorded or not — a
/// close still turns off a unit whose `on` failed); a switchback's units at
/// the latest `on` that was sent for them, in unit order.
fn last_ons<'a>(c: &'a ExperimentConfig, flips: &'a [FlipFact]) -> Vec<(usize, &'a str, Moment)> {
    let Some(sb) = &c.design.switchback else {
        return treated(c).collect();
    };
    let mut latest: BTreeMap<&str, Moment> = BTreeMap::new();
    // An `on` that never left (cancelled, or failed before it was sent)
    // never turned the lever on: it must not hide an earlier one that did.
    for x in flips
        .iter()
        .filter(|x| x.state == LeverState::On && x.dispatched && !x.cancelled)
    {
        let day = latest.entry(x.unit.as_str()).or_insert(x.due_at);
        *day = (*day).max(x.due_at);
    }
    latest
        .into_iter()
        .map(|(unit, day)| {
            (
                sb.periods.iter().position(|p| p.from == day).unwrap_or(0),
                unit,
                day,
            )
        })
        .collect()
}

/// No `off` recorded for `unit` on or after `since`, its last `on`'s day. A
/// terminal `off` is dated the day it was decided, so "already off" is this,
/// never a match on one date. A cancelled `off` was never sent: it counts for
/// nothing.
fn needs_off(flips: &[FlipFact], unit: &str, since: Moment) -> bool {
    !flips
        .iter()
        .any(|x| x.unit == unit && x.state == LeverState::Off && !x.cancelled && x.due_at >= since)
}

fn abort_offs(c: &ExperimentConfig, f: &RunFacts) -> Vec<RunAction> {
    on_now(c, &f.flips)
        .into_iter()
        .map(|(wave, unit, _)| flip(unit, LeverState::Off, wave, f.now))
        .collect()
}

/// Each unit whose latest `on` was sent and not followed by an `off`, with that `on`'s
/// wave (or period) and day.
fn on_now<'a>(c: &'a ExperimentConfig, flips: &'a [FlipFact]) -> Vec<(usize, &'a str, Moment)> {
    let sent = |unit: &str, day: Moment| {
        flips
            .iter()
            .any(|x| x.unit == unit && x.state == LeverState::On && x.due_at == day && x.dispatched)
    };
    last_ons(c, flips)
        .into_iter()
        .filter(|(_, unit, day)| sent(unit, *day) && needs_off(flips, unit, *day))
        .collect()
}

fn close_offs(c: &ExperimentConfig, f: &RunFacts) -> Vec<RunAction> {
    if matches!(c.on_close(), OnClose::Keep) {
        return Vec::new();
    }
    last_ons(c, &f.flips)
        .into_iter()
        .filter(|(_, unit, day)| needs_off(&f.flips, unit, *day))
        .map(|(wave, unit, _)| flip(unit, LeverState::Off, wave, f.now))
        .collect()
}

#[cfg(test)]
mod tests;
