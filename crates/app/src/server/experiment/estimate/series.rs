//! The period-by-period series the report charts (a day, week or month per point,
//! `design.grain`): the target measure across the whole run,
//! from the first pre window to the last post day, read from the **same** panel the
//! engine estimated from — so the chart is a snapshot of what was measured, sealed
//! with the result, never re-queried later.
//!
//! Waves: per day, the unweighted mean over the treated units and over the never-treated
//! controls, and how many treated units had the lever on. A switchback: per day, the
//! fleet mean, whether the period was on, and whether the day sat in a washout.
//! Days the panel dropped (a day any unit is missing) are absent, exactly as the engine
//! never saw them.

use chrono::{NaiveDate, NaiveDateTime};
use oxy_airlayer_compat::engine::experiment::{
    Assignment, ExperimentDesign, PanelMatrix, SwitchbackSchedule,
};
use oxy_experiment::Grain;
use serde::Serialize;

/// A point's or a window edge's first moment: `YYYY-MM-DD`, or `YYYY-MM-DDTHH:00`
/// (local) at hour grain, so ISO strings still order and compare as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    pub at: NaiveDateTime,
    pub hourly: bool,
}

impl Moment {
    pub fn date(&self) -> NaiveDate {
        self.at.date()
    }
}

impl PartialEq<NaiveDate> for Moment {
    fn eq(&self, other: &NaiveDate) -> bool {
        !self.hourly && self.at.date() == *other
    }
}

impl Serialize for Moment {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let f = if self.hourly {
            "%Y-%m-%dT%H:00"
        } else {
            "%Y-%m-%d"
        };
        s.collect_str(&self.at.format(f))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DailySeries {
    pub measure: String,
    /// The period each point covers: every `date` is the first day of one.
    pub grain: Grain,
    #[serde(flatten)]
    pub shape: SeriesShape,
    /// An aborted run's cut (exclusive): no day on or after it was read.
    pub measured_until: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "design", rename_all = "snake_case")]
pub enum SeriesShape {
    Waves {
        treated_units: usize,
        control_units: usize,
        waves: Vec<WaveWindow>,
        days: Vec<WaveDay>,
    },
    Switchback {
        fleet_units: usize,
        washout_days: usize,
        periods: Vec<PeriodSpan>,
        days: Vec<SwitchbackDay>,
    },
}

/// One wave's switch date and its registered windows, both half-open (`to` exclusive).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WaveWindow {
    pub switch_on: Moment,
    pub units: usize,
    pub pre_from: Moment,
    pub pre_to: Moment,
    pub post_from: Moment,
    pub post_to: Moment,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WaveDay {
    pub date: Moment,
    /// `None` when the panel holds no unit of that arm.
    pub treated: Option<f64>,
    pub control: Option<f64>,
    pub on_units: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PeriodSpan {
    pub from: Moment,
    pub to: Moment,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SwitchbackDay {
    pub date: Moment,
    pub fleet: f64,
    pub on: bool,
    pub washout: bool,
}

pub fn daily_series(
    measure: &str,
    grain: Grain,
    design: &ExperimentDesign,
    m: &PanelMatrix,
) -> DailySeries {
    let date = |ord: i64| Moment {
        at: grain
            .start_at(ord)
            .expect("a panel ordinal is a period of the calendar"),
        hourly: grain == Grain::Hour,
    };
    let shape = match design {
        ExperimentDesign::Waves(a) => waves(a, m, &date),
        ExperimentDesign::Switchback(s) => switchback(s, m, &date),
    };
    DailySeries {
        measure: measure.to_string(),
        grain,
        shape,
        measured_until: None,
    }
}

/// The panel's retained days in `[from, to)`, as `(ordinal, column)`: a calendar day the
/// panel dropped has no column and is skipped.
fn retained(m: &PanelMatrix, from: i64, to: i64) -> Vec<(i64, usize)> {
    (from..to)
        .filter_map(|d| {
            let (lo, hi) = m.ordinal_range(d, d + 1);
            (hi > lo).then_some((d, lo))
        })
        .collect()
}

/// Unweighted mean of `rows` on one panel column; `None` with no row.
fn mean(m: &PanelMatrix, rows: &[usize], col: usize) -> Option<f64> {
    (!rows.is_empty()).then(|| rows.iter().map(|r| m.get(*r, col)).sum::<f64>() / rows.len() as f64)
}

fn waves(a: &Assignment, m: &PanelMatrix, date: &dyn Fn(i64) -> Moment) -> SeriesShape {
    // Only units the panel holds: one it dropped contributes to neither arm.
    let mut treated: Vec<(usize, i64)> = Vec::new();
    let mut control: Vec<usize> = Vec::new();
    for (unit, switch) in &a.switch_day {
        let Some(row) = m.unit_index(unit) else {
            continue;
        };
        match switch {
            Some(day) => treated.push((row, *day)),
            None => control.push(row),
        }
    }
    // Row order, not the map's: the means are then summed in one fixed order and the
    // sealed series is reproducible to the last bit.
    treated.sort_unstable();
    control.sort_unstable();
    let mut switches: Vec<i64> = treated.iter().map(|(_, d)| *d).collect();
    switches.sort_unstable();
    switches.dedup();
    let pre_start = |s: i64| s - (a.anticipation_days + a.pre_days) as i64;
    let post_end = |s: i64| s + (a.washout_days + a.post_days) as i64;
    let windows: Vec<WaveWindow> = switches
        .iter()
        .map(|&s| WaveWindow {
            switch_on: date(s),
            units: treated.iter().filter(|(_, d)| *d == s).count(),
            pre_from: date(pre_start(s)),
            pre_to: date(s - a.anticipation_days as i64),
            post_from: date(s + a.washout_days as i64),
            post_to: date(post_end(s)),
        })
        .collect();
    let rows: Vec<usize> = treated.iter().map(|(r, _)| *r).collect();
    let days = match (switches.first(), switches.last()) {
        (Some(&first), Some(&last)) => retained(m, pre_start(first), post_end(last))
            .into_iter()
            .map(|(ord, col)| WaveDay {
                date: date(ord),
                treated: mean(m, &rows, col),
                control: mean(m, &control, col),
                on_units: treated.iter().filter(|(_, s)| *s <= ord).count(),
            })
            .collect(),
        _ => Vec::new(),
    };
    SeriesShape::Waves {
        treated_units: treated.len(),
        control_units: control.len(),
        waves: windows,
        days,
    }
}

fn switchback(
    s: &SwitchbackSchedule,
    m: &PanelMatrix,
    date: &dyn Fn(i64) -> Moment,
) -> SeriesShape {
    let len = s.period_days as i64;
    let mut periods: Vec<(i64, bool)> = s.periods.iter().map(|p| (p.from_day, p.on)).collect();
    periods.sort_unstable();
    let fleet: Vec<usize> = (0..m.n_units()).collect();
    let days = match (periods.first(), periods.last()) {
        (Some(&(first, _)), Some(&(last, _))) => retained(m, first, last + len)
            .into_iter()
            .filter_map(|(ord, col)| {
                let &(from, on) = periods.iter().rev().find(|(f, _)| *f <= ord)?;
                Some(SwitchbackDay {
                    date: date(ord),
                    fleet: mean(m, &fleet, col)?,
                    on,
                    washout: ord < from + s.washout_days as i64,
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    SeriesShape::Switchback {
        fleet_units: m.n_units(),
        washout_days: s.washout_days,
        periods: periods
            .iter()
            .map(|&(from, on)| PeriodSpan {
                from: date(from),
                to: date(from + len),
                on,
            })
            .collect(),
        days,
    }
}
