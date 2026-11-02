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
    Assignment, ExperimentDesign, PanelMatrix, SwitchbackSchedule, SyntheticSpec,
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
    /// The carryover window `[from, to)` whose days follow the post window, read on its
    /// own panel; absent when the design reads none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carryover: Option<CarryoverSpan>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CarryoverSpan {
    pub from: Moment,
    pub to: Moment,
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
        ExperimentDesign::Synthetic(s) => waves(&as_waves(s), m, &date),
    };
    DailySeries {
        measure: measure.to_string(),
        grain,
        shape,
        measured_until: None,
        carryover: None,
    }
}

/// `s` with the carryover window's days appended: the retained days of its own panel `m`
/// after the series' last day and before `until`, lever off, and the window `[off,
/// until)` stated. The days already charted keep the registered panel's values. A
/// switchback reads no carryover, so it is returned as it was.
pub fn with_carryover(
    mut s: DailySeries,
    design: &ExperimentDesign,
    m: &PanelMatrix,
    off: oxy_experiment::Moment,
    until: NaiveDate,
) -> DailySeries {
    let ExperimentDesign::Waves(a) = design else {
        return s;
    };
    let grain = s.grain;
    let SeriesShape::Waves { days, .. } = &mut s.shape else {
        return s;
    };
    let ord = |d: NaiveDate| grain.ordinal_of(d.into());
    let date = |o: i64| Moment {
        at: grain
            .start_at(o)
            .expect("a panel ordinal is a period of the calendar"),
        hourly: grain == Grain::Hour,
    };
    let off = grain.ordinal_of(off);
    // The period after the last charted one, from its moment: at hour grain its date
    // alone would restart the count at that day's midnight.
    let after = days.last().map_or(off, |w| grain.ordinal_at(w.date.at) + 1);
    let (treated, control) = arms(a, m);
    let rows: Vec<usize> = treated.iter().map(|(r, _)| *r).collect();
    days.extend(
        retained(m, after, ord(until))
            .into_iter()
            .map(|(o, col)| WaveDay {
                date: date(o),
                treated: mean(m, &rows, col),
                control: mean(m, &control, col),
                // A close later than the post window leaves days still on before the off.
                on_units: if o < off { treated.len() } else { 0 },
            }),
    );
    s.carryover = Some(CarryoverSpan {
        from: date(off),
        to: date(ord(until)),
    });
    s
}

/// A synthetic control's units as one wave: treated on `start_on`, donors never.
/// The series charts arms by mean, which is what a reader compares the fits to.
fn as_waves(s: &SyntheticSpec) -> Assignment {
    let treated = s.treated.iter().map(|u| (u.clone(), Some(s.start_on)));
    let donors = s.donors.iter().flatten().filter(|u| !s.exclude.contains(u));
    Assignment {
        switch_day: treated.chain(donors.map(|u| (u.clone(), None))).collect(),
        strata: None,
        pre_days: s.pre_days,
        post_days: s.post_days,
        anticipation_days: s.anticipation_days,
        washout_days: s.washout_days,
        coverage_floor: s.coverage_floor,
        alpha: s.alpha,
        family: s.family,
        one_date_test: oxy_airlayer_compat::engine::experiment::OneDateTest::Welch,
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

/// Unweighted mean of `rows` on one panel column, over the units that hold that day
/// (a past-data panel keeps one unit's gaps to itself); `None` when none does.
fn mean(m: &PanelMatrix, rows: &[usize], col: usize) -> Option<f64> {
    let held: Vec<f64> = rows
        .iter()
        .filter(|r| m.is_present(**r, col))
        .map(|r| m.get(*r, col))
        .collect();
    (!held.is_empty()).then(|| held.iter().sum::<f64>() / held.len() as f64)
}

/// The panel rows of each arm, treated with its switch day. Only units the panel holds:
/// one it dropped contributes to neither arm. Row order, not the map's: the means are
/// then summed in one fixed order and the sealed series is reproducible to the last bit.
fn arms(a: &Assignment, m: &PanelMatrix) -> (Vec<(usize, i64)>, Vec<usize>) {
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
    treated.sort_unstable();
    control.sort_unstable();
    (treated, control)
}

fn waves(a: &Assignment, m: &PanelMatrix, date: &dyn Fn(i64) -> Moment) -> SeriesShape {
    let (treated, control) = arms(a, m);
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
