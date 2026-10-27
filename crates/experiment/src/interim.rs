//! The interim reading: the registered estimator over the data so far, on a run
//! that is still live (or closing). Never sealed, never a coefficient, never a lever.
//!
//! The estimator reads whole windows: a post window cut short by "today" holds less
//! than its coverage floor and refuses. So the interim reads the design **trimmed to
//! what has finished** ([`interim_config`]): a waves design keeps the waves with at least
//! one whole post-window day before the cut, with every post window shortened to the
//! shortest one read so far; a switchback keeps its finished on/off pairs. The report is
//! then relabelled ([`as_interim`]): no totals that extrapolate, no write-back, and a
//! label that says the interval is the registered fixed-horizon one.

use chrono::NaiveDate;

use crate::grain::Grain;
use crate::moment::Moment;

use crate::config::ExperimentConfig;
use crate::report::{ReportStatus, ReportView};
use crate::timeline::last_post_window_end;

/// The design an interim reads, trimmed to the days that have finished before `cut`.
#[derive(Debug, Clone, PartialEq)]
pub struct InterimWindow {
    pub config: ExperimentConfig,
    /// The last day read, inclusive.
    pub through: NaiveDate,
    /// Waves (or switchback pairs) that had not finished a whole post day and are left out.
    pub left_out: usize,
}

/// `cut` is exclusive: the first moment **not** read (now, in the design's timezone —
/// a period still in progress is never read). It moves back to the start of the
/// period it falls in: the hour at hour grain, the day, week or month otherwise.
pub fn interim_config(c: &ExperimentConfig, cut: Moment) -> Result<InterimWindow, String> {
    let mut config = c.clone();
    let grain = c.design.grain;
    let cut = match grain {
        Grain::Hour => grain.start_at(grain.ordinal_of(cut)).map_or(cut, Moment),
        _ => Moment::midnight(grain.floor(cut.date())),
    };
    let left_out = if config.design.switchback.is_some() {
        trim_periods(&mut config, cut)?
    } else {
        trim_waves(&mut config, cut)?
    };
    let end = last_post_window_end(&config)
        .ok_or_else(|| "the design names no wave or switchback period".to_string())?;
    Ok(InterimWindow {
        config,
        through: last_inside(grain, end).date(),
        left_out,
    })
}

/// The last period inside a window that ends (exclusively) at `end`: its last
/// hour at hour grain, its last day otherwise.
fn last_inside(grain: Grain, end: Moment) -> Moment {
    match grain {
        Grain::Hour => end.plus_hours(-1).unwrap_or(end),
        _ => Moment::midnight(end.date().pred_opt().unwrap_or(end.date())),
    }
}

/// `m` moved `n` periods of `grain` later.
fn plus(grain: Grain, m: Moment, n: u32) -> Moment {
    grain.later(m, n).unwrap_or(m)
}

/// Keep the waves with a whole post day before `cut`; shorten every post window to the
/// shortest one read. A left-out wave's units leave the strata: they are in no arm.
fn trim_waves(c: &mut ExperimentConfig, cut: Moment) -> Result<usize, String> {
    let post = c
        .design
        .post_days
        .ok_or_else(|| "the design has no post window".to_string())?;
    let (grain, washout) = (c.design.grain, c.design.washout_days);
    // Whole periods read since the post window opened: ordinals, so hours count too.
    let read = |switch_on: Moment| {
        grain.ordinal_of(cut) - (grain.ordinal_of(switch_on) + i64::from(washout))
    };
    let (kept, out): (Vec<_>, Vec<_>) = c
        .design
        .waves
        .drain(..)
        .partition(|w| read(w.switch_on) >= 1);
    let Some(shortest) = kept.iter().map(|w| read(w.switch_on)).min() else {
        let opens = out
            .iter()
            .map(|w| plus(grain, w.switch_on, washout))
            .min()
            .ok_or_else(|| "the design names no wave".to_string())?;
        return Err(format!(
            "no wave has a whole post-window {} yet: the first post window opens on {opens}, \
             so an interim can read from {}",
            grain.as_str(),
            plus(grain, opens, 1)
        ));
    };
    c.design.post_days = Some(post.min(u32::try_from(shortest).unwrap_or(u32::MAX)));
    let gone: Vec<String> = out
        .iter()
        .flat_map(|w| w.units.iter())
        .filter(|u| !kept.iter().any(|k| k.units.contains(u)))
        .cloned()
        .collect();
    if let Some(strata) = c.design.strata.as_mut() {
        for s in strata.iter_mut() {
            s.retain(|u| !gone.contains(u));
        }
        strata.retain(|s| !s.is_empty());
    }
    c.design.waves = kept;
    Ok(out.len())
}

/// Keep a switchback's finished on/off pairs; a pair still running is not read.
fn trim_periods(c: &mut ExperimentConfig, cut: Moment) -> Result<usize, String> {
    let sb = c
        .design
        .switchback
        .as_mut()
        .ok_or_else(|| "the design has no switchback".to_string())?;
    let (grain, days) = (c.design.grain, sb.period_days);
    let finished = sb
        .periods
        .iter()
        .take_while(|p| plus(grain, p.from, days) <= cut)
        .count();
    let pairs = sb.periods.len() / 2;
    let kept = finished / 2;
    if kept == 0 {
        let first = sb
            .periods
            .get(1)
            .map(|p| plus(grain, p.from, days))
            .ok_or_else(|| "the switchback names no on/off pair".to_string())?;
        return Err(format!(
            "no on/off pair has finished yet: the first finishes on {}, so an interim can read \
             from {first}",
            last_inside(grain, first)
        ));
    }
    sb.periods.truncate(kept * 2);
    Ok(pairs - kept)
}

/// Relabel a report built over an [`InterimWindow`] as an interim reading. What would
/// extrapolate a partial run (the treated total, the projection), propose a coefficient
/// or set the estimate against a prior is dropped: an interim informs, it decides nothing.
pub fn as_interim(mut view: ReportView, through: NaiveDate) -> ReportView {
    view.status = ReportStatus::Interim { through };
    view.label = label(through);
    view.treated_total = None;
    view.projection = None;
    view.write_back = None;
    view.prediction = None;
    view.prediction_note = None;
    view
}

pub(crate) fn label(through: NaiveDate) -> String {
    format!(
        "INTERIM — not the final result: the registered estimator over the days read through \
         {through}, while the run is still in progress. Its interval is the registered \
         fixed-horizon one, so looking before the run ends overstates its confidence"
    )
}
