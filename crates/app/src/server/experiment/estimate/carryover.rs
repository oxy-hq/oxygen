//! The carryover window: `design.carryover_days` after the close `off`s.
//!
//! What a campaign leaves behind once it stops (an ad seen this week, a purchase
//! next week). Read with the registered estimator and nothing new: the same pre
//! baseline, a post window moved to start at the off — `washout` becomes the days
//! from the switch to the off, `post_days` the carryover length. Only a design
//! with one switch date reads one (`validate::check_carryover`), so that shift is
//! the same for every treated unit.

use chrono::NaiveDate;
use oxy_airlayer_compat::engine::experiment::{ExperimentDesign, PanelMatrix, estimate};
use oxy_experiment::config::ExperimentConfig;
use oxy_experiment::engine_in::EffectIn;
use oxy_experiment::moment::Moment;
use oxy_experiment::run::LeverState;

use super::mirror::Judged;
use super::plan::FlipDay;

/// `[off, off + carryover_days)` (periods of `design.grain`): from the earliest close `off` that was sent,
/// kept as the moment it went out (an hour-grain close can land mid-day), to the
/// first midnight at or after the window's end, where the panel read stops. `None`
/// when the design reads no carryover, when the levers never went off, and for an
/// aborted run — its `off`s are the abort, and its report stops at them.
pub fn carryover_window(
    cfg: &ExperimentConfig,
    aborted: bool,
    flips: &[FlipDay],
) -> Option<(Moment, NaiveDate)> {
    let days = cfg.design.carryover_days?;
    if aborted {
        return None;
    }
    let off = flips
        .iter()
        .filter(|f| f.state == LeverState::Off && !f.cancelled)
        .map(|f| f.due_at)
        .min()?;
    // Read per day: the window is the days the carryover's hours touch.
    let until = cfg.design.grain.later(off, days)?;
    Some((off, until.ceil_date()))
}

/// The registered estimator over the carryover window. A design that is not one
/// switch date (unreachable once validated) is refused, never approximated.
pub fn carryover_effect(
    cfg: &ExperimentConfig,
    design: &ExperimentDesign,
    panel: &PanelMatrix,
    off: Moment,
    seed: u64,
) -> EffectIn {
    let refused = |why: &str| Judged::<EffectIn>::refused(why.to_string()).view;
    let ExperimentDesign::Waves(a) = design else {
        return refused("a switchback reads no carryover window");
    };
    let switches: std::collections::BTreeSet<i64> =
        a.switch_day.values().flatten().copied().collect();
    let [switch] = switches.into_iter().collect::<Vec<_>>()[..] else {
        return refused("the carryover window needs one switch date for every treated unit");
    };
    let Some(days) = cfg.design.carryover_days else {
        return refused("the design reads no carryover window");
    };
    let mut shifted = a.clone();
    shifted.washout_days = usize::try_from(cfg.design.grain.ordinal_of(off) - switch).unwrap_or(0);
    shifted.post_days = days as usize;
    Judged::<EffectIn>::from_engine(&estimate(panel, &ExperimentDesign::Waves(shifted), seed)).view
}
