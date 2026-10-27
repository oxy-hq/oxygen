//! Compliance: every unit as assigned. The estimate is intention-to-treat and
//! never sees this; the record exists so the report can say how diluted the
//! attempt was and restate the design's floor accordingly.

use std::collections::BTreeMap;

use chrono::TimeZone;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::config::{ExperimentConfig, LeverState};
use crate::grain::Grain;
use crate::moment::Moment;

/// How read-back samples the lever: one reading per local day, or per local hour
/// on an hour-grain design (whose switches can fall inside a day). The grace is in
/// days either way: it is how long the lever takes to land. Hours are local to
/// the design's timezone, the one the warehouse buckets them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sampling {
    Daily,
    Hourly(Tz),
}

impl Sampling {
    pub fn of(c: &ExperimentConfig) -> Self {
        if c.design.grain == Grain::Hour {
            Sampling::Hourly(c.design.timezone)
        } else {
            Sampling::Daily
        }
    }

    /// The reading after `m`.
    pub fn next(self, m: Moment) -> Moment {
        let hours = match self {
            Sampling::Daily => 24,
            Sampling::Hourly(_) => 1,
        };
        m.plus_hours(hours).unwrap_or(m)
    }

    /// The reading `m` falls in: its day's midnight, or its hour.
    pub fn floor(self, m: Moment) -> Moment {
        match self {
            Sampling::Daily => Moment::midnight(m.date()),
            Sampling::Hourly(_) => Grain::Hour
                .start_at(Grain::Hour.ordinal_of(m))
                .map_or(m, Moment),
        }
    }

    /// The last reading inside the grace after `from`: `grace_days` days on.
    fn grace_end(self, from: Moment, grace_days: u32) -> Moment {
        from.plus_hours(24 * i64::from(grace_days)).unwrap_or(from)
    }

    /// Every reading in `[from, until)`: an hour a spring-forward night skips
    /// never happened, so the warehouse has no reading for it to miss.
    fn span(self, from: Moment, until: Moment) -> impl Iterator<Item = Moment> {
        std::iter::successors(Some(from), move |m| Some(self.next(*m)))
            .take_while(move |m| *m < until)
            .filter(move |m| match self {
                Sampling::Daily => true,
                Sampling::Hourly(tz) => tz.from_local_datetime(&m.at()).earliest().is_some(),
            })
    }
}

/// Treated units: `Landed` | `NeverLanded` | `Failed`.
/// Controls: `Clean` | `LeakedToControl` — every control appears.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FlipOutcome {
    /// `when_on` first held on this reading (a day, or an hour on an hour-grain
    /// design), within `grace_days` of the switch.
    Landed { on_day: Moment },
    /// Grace is over, every day of it was seen, and `when_on` never held — or,
    /// in the final record, the flip was never decided or never sent.
    NeverLanded,
    /// The flip itself failed (missing lever, refused request, failed lever run).
    Failed { reason: String },
    /// A control the lever never reached.
    Clean,
    /// A control where `when_off` first failed on this reading.
    LeakedToControl { on_day: Moment },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComplianceSummary {
    pub c_treated: f64,
    pub c_control_leak: f64,
    /// `mde_at_proposal / (c_treated − c_control_leak)`; `None` when that gap
    /// is not positive or there was no promise to restate.
    pub restated_mde: Option<f64>,
    pub below_floor: bool,
    /// Entries on the treated side / the control side. `n_treated == 0` means
    /// compliance was NOT MEASURED (the report and the surface say so — never
    /// "0%", never `below_floor`).
    pub n_treated: usize,
    pub n_control: usize,
}

/// Each entry is treated or control by its variant, never by its key: a
/// switchback has no waves, yet every one of its units is treated.
pub fn summarize(c: &ExperimentConfig, outcomes: &[(String, FlipOutcome)]) -> ComplianceSummary {
    let (mut t_n, mut t_landed, mut c_n, mut c_leaked) = (0usize, 0usize, 0usize, 0usize);
    for (_, outcome) in outcomes {
        if is_treated(outcome) {
            t_n += 1;
            t_landed += usize::from(matches!(outcome, FlipOutcome::Landed { .. }));
        } else {
            c_n += 1;
            c_leaked += usize::from(matches!(outcome, FlipOutcome::LeakedToControl { .. }));
        }
    }
    let c_treated = share(t_landed, t_n);
    let c_control_leak = share(c_leaked, c_n);
    let gap = c_treated - c_control_leak;
    ComplianceSummary {
        c_treated,
        c_control_leak,
        // Nothing treated was recorded: not measured, so no restatement and no floor verdict.
        restated_mde: c
            .outcome
            .mde_at_proposal
            .filter(|_| t_n > 0 && gap > 0.0)
            .map(|m| m / gap),
        below_floor: t_n > 0 && c_treated < c.design.compliance_floor,
        n_treated: t_n,
        n_control: c_n,
    }
}

fn share(n: usize, of: usize) -> f64 {
    if of == 0 { 0.0 } else { n as f64 / of as f64 }
}

fn is_treated(o: &FlipOutcome) -> bool {
    matches!(
        o,
        FlipOutcome::Landed { .. } | FlipOutcome::NeverLanded | FlipOutcome::Failed { .. }
    )
}

/// C4's record key for a switchback unit in one period: `"<unit>@<period from>"`.
pub fn period_key(unit: &str, from: Moment) -> String {
    format!("{unit}@{from}")
}

/// A part complied when it is in the state its side asked for: `Landed`
/// (treated) or `Clean` (control).
fn complied(o: &FlipOutcome) -> bool {
    matches!(o, FlipOutcome::Landed { .. } | FlipOutcome::Clean)
}

/// The share of decided parts that complied — a group's "share of its members
/// that landed". Display only: `summarize` pools members itself, and nothing
/// is thresholded on this. `None` with none decided.
pub fn share_complied(parts: &[FlipOutcome]) -> Option<f64> {
    let n = parts.iter().filter(|o| complied(o)).count();
    (!parts.is_empty()).then(|| n as f64 / parts.len() as f64)
}

/// One switchback period at one unit: `[from, until)` in `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodSpan {
    pub from: Moment,
    pub until: Moment,
    pub state: LeverState,
}

/// A switchback unit's outcome in one period, from the expression matching
/// the period (`when_on` for on, `when_off` for off) per day — only days the
/// warehouse returned. An on period is a treated flip: `classify_treated`
/// over its grace window. An off period is a control for its own span: from
/// the last grace day to the period's end `when_off` must hold, and the first
/// day it does not is the leak (a late-landing off leaks on that last grace
/// day). `Clean` only once the period is over and every one of those days was
/// seen: a day not yet loaded leaves it undecided, never leaked.
pub fn classify_period(
    p: &PeriodSpan,
    grace_days: u32,
    sampling: Sampling,
    now: Moment,
    held: &BTreeMap<Moment, bool>,
) -> Option<FlipOutcome> {
    if p.state == LeverState::On {
        return classify_treated(p.from, grace_days, sampling, now, held);
    }
    let settled = sampling.grace_end(p.from, grace_days);
    if let Some((at, _)) = held.range(settled..p.until).find(|(_, h)| !**h) {
        return Some(FlipOutcome::LeakedToControl { on_day: *at });
    }
    let every_reading_seen = sampling
        .span(settled, p.until)
        .all(|m| held.contains_key(&m));
    (now >= p.until && every_reading_seen).then_some(FlipOutcome::Clean)
}

/// A treated unit's outcome from `when_on` per reading (`held`, only readings the
/// warehouse returned). `Landed` on the first held reading in
/// `[switch_on, switch_on + grace_days]`; `NeverLanded` only once that window
/// is over AND every reading of it was seen; otherwise undecided (`None`) — a day
/// the warehouse has not loaded is not a day the promo was off. `now` is the
/// reading in progress (today, or this hour), which is never read.
pub fn classify_treated(
    switch_on: Moment,
    grace_days: u32,
    sampling: Sampling,
    now: Moment,
    held: &BTreeMap<Moment, bool>,
) -> Option<FlipOutcome> {
    let last = sampling.grace_end(switch_on, grace_days);
    if let Some((at, _)) = held.range(switch_on..=last).find(|(_, h)| **h) {
        return Some(FlipOutcome::Landed { on_day: *at });
    }
    let after = sampling.next(last);
    let every_reading_seen = sampling
        .span(switch_on, after)
        .all(|m| held.contains_key(&m));
    (now >= after && every_reading_seen).then_some(FlipOutcome::NeverLanded)
}

/// The first reading `when_off` failed at a control (`held_off` maps reading → held).
pub fn first_leak(held_off: &BTreeMap<Moment, bool>) -> Option<Moment> {
    held_off.iter().find(|(_, held)| !**held).map(|(at, _)| *at)
}

#[cfg(test)]
mod tests;
