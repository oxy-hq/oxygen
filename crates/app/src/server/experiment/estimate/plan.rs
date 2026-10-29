//! Pure decisions the estimate job makes before and after the engine runs.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use oxy_airlayer_compat::engine::experiment::{
    Assignment, DesignShape, DesignSpec, ExperimentDesign, PanelMatrix, SwitchbackSchedule,
};
use oxy_experiment::Moment;
use oxy_experiment::config::ExperimentConfig;
use oxy_experiment::engine_in::EffectIn;
use oxy_experiment::report::{Seeds, SwitchbackFacts};
use oxy_experiment::run::LeverState;
use oxy_experiment::timeline::last_post_window_end_date;

use crate::server::experiment::panel::GroupMembership;

pub const NO_POOL: &str = "the run's control pool was never pinned (no experiment_check ran before it ended), \
                           so no assignment can be built";
pub const NO_MEMBERSHIP: &str = "the run's group membership was never pinned (no experiment_check ran before it \
                                 ended), so the panel cannot be summed into groups";
pub const NO_FLEET: &str = "the run's fleet was never pinned (no experiment_check ran before it ended), so the \
                            switchback panel cannot be restricted to the stores it started with";

/// The estimate uses the registered seed — reproducing the design reproduces the
/// permutation test. The placebo and the ratio get their own, derived, so no two
/// random steps share a stream. All three are recorded in the report.
pub fn derive_seeds(design_seed: u64) -> Seeds {
    Seeds {
        effect: design_seed,
        placebo: mix(design_seed, 0xA1),
        ratio: mix(design_seed, 0xB2),
    }
}

/// Seeds travel through JSON and JavaScript (C2b), so every derived seed is masked to 53 bits.
const SEED_MASK: u64 = (1u64 << 53) - 1;

/// SplitMix64 finaliser over `seed + salt·φ`, masked to 53 bits.
fn mix(seed: u64, salt: u64) -> u64 {
    let mut z = seed.wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) & SEED_MASK
}

/// The estimate reads `to_design`'s waves for its sibling, so this is reached
/// only by a caller handing the engine design over.
const SYNTHETIC_NO_SIBLING: &str =
    "a synthetic control is priced by placebo-in-time over its own fits, not by a sibling placebo";

/// The first day any unit is switched on, or the first switchback period starts.
pub fn earliest_day(d: &ExperimentDesign) -> Option<i64> {
    match d {
        ExperimentDesign::Waves(a) => a.switch_day.values().flatten().copied().min(),
        ExperimentDesign::Switchback(s) => s.periods.iter().map(|p| p.from_day).min(),
        ExperimentDesign::Synthetic(s) => Some(s.start_on),
    }
}

/// Units that never switch and that the panel holds — the controls the estimate
/// could read, which is what the projection multiplies by. A control the panel
/// never received (filtered out of the target, or with no rows) is not one. A
/// switchback treats the whole fleet.
pub fn never_treated(d: &ExperimentDesign, m: &PanelMatrix) -> usize {
    match d {
        ExperimentDesign::Waves(a) => a
            .switch_day
            .iter()
            .filter(|(unit, day)| day.is_none() && m.unit_index(unit).is_some())
            .count(),
        ExperimentDesign::Switchback(_) => 0,
        // Read only by the projection, which a synthetic control does not make.
        ExperimentDesign::Synthetic(_) => 0,
    }
}

/// The sibling's placebo design: the registered design's own shape and windows,
/// **bounded to history before the first switch**. A placebo drawn over
/// post-switch history swallows the real effect, inflates the null and so the
/// substitution floor the report prints (spec § Power). A blocked assignment blocks
/// the placebo the same way; a switchback prices a switchback.
pub fn sibling_design_spec(
    cfg: &ExperimentConfig,
    d: &ExperimentDesign,
    iterations: usize,
) -> Result<DesignSpec, String> {
    let earliest = earliest_day(d).ok_or_else(|| {
        "no unit has a switch day and no period starts, so there is no design to price".to_string()
    })?;
    Ok(match d {
        ExperimentDesign::Waves(a) => waves_spec(cfg, a, earliest, iterations),
        ExperimentDesign::Switchback(s) => switchback_spec(cfg, s, earliest, iterations),
        ExperimentDesign::Synthetic(_) => return Err(SYNTHETIC_NO_SIBLING.to_string()),
    })
}

fn waves_spec(
    cfg: &ExperimentConfig,
    a: &Assignment,
    earliest: i64,
    iterations: usize,
) -> DesignSpec {
    let mut waves: BTreeMap<i64, usize> = BTreeMap::new();
    for day in a.switch_day.values().flatten() {
        *waves.entry(*day).or_default() += 1;
    }
    let n_treated: usize = waves.values().sum();
    let n_never = a.switch_day.values().filter(|d| d.is_none()).count();
    let days: Vec<i64> = waves.keys().copied().collect();
    let shape = if waves.len() == 1 {
        DesignShape::CommonDate {
            n_treated,
            n_control: n_never,
        }
    } else {
        DesignShape::Staggered {
            wave_sizes: waves.values().copied().collect(),
            // Uneven registered spacing: the tightest gap is the conservative one.
            spacing_days: days
                .windows(2)
                .map(|w| (w[1] - w[0]) as usize)
                .min()
                .unwrap_or(0),
            n_never_treated: n_never,
        }
    };
    DesignSpec {
        shape,
        pre_days: a.pre_days,
        post_days: a.post_days,
        anticipation_days: a.anticipation_days,
        washout_days: a.washout_days,
        coverage_floor: a.coverage_floor,
        alpha: a.alpha,
        family: a.family,
        power: cfg.outcome.power,
        iterations,
        history_to: Some(earliest - a.anticipation_days as i64),
        blocks: a.strata.as_ref().map_or(0, Vec::len),
        one_date_test: a.one_date_test,
    }
}

/// `pre_days`, `post_days`, `anticipation_days` and `blocks` are ignored — and must be 0 —
/// for a switchback (C1), so there is no anticipation to subtract from the bound.
fn switchback_spec(
    cfg: &ExperimentConfig,
    s: &SwitchbackSchedule,
    earliest: i64,
    iterations: usize,
) -> DesignSpec {
    DesignSpec {
        shape: DesignShape::Switchback {
            period_days: s.period_days,
            pairs: s.periods.len() / 2,
        },
        pre_days: 0,
        post_days: 0,
        anticipation_days: 0,
        washout_days: s.washout_days,
        coverage_floor: s.coverage_floor,
        alpha: s.alpha,
        family: s.family,
        power: cfg.outcome.power,
        iterations,
        history_to: Some(earliest),
        blocks: 0,
        one_date_test: oxy_airlayer_compat::engine::experiment::OneDateTest::Welch,
    }
}

/// A switchback's totals basis: the fleet from the panel and the on-days from the engine's own
/// `EffectResult.retained_on_days` (C1) — exact, the days the engine actually kept. `Ok(None)`
/// for a design that is not a switchback, and for an engine refusal (which already says why
/// there is no estimate). A non-refused switchback effect with no `retained_on_days` breaks
/// C1's contract: that is a section refusal, never a total reconstructed from the calendar.
pub fn switchback_facts(
    cfg: &ExperimentConfig,
    effect: &EffectIn,
    fleet_units: usize,
) -> Result<Option<SwitchbackFacts>, String> {
    if cfg.design.switchback.is_none() || effect.refusal.is_some() {
        return Ok(None);
    }
    match effect.retained_on_days {
        Some(on_days) => Ok(Some(SwitchbackFacts {
            fleet_units,
            on_days,
        })),
        None => Err(
            "the engine returned no `retained_on_days` for this switchback, so its treated total \
                     cannot be computed"
                .to_string(),
        ),
    }
}

/// The assignment's universe: every wave's units plus the run's pinned control
/// pool, sorted and de-duplicated. Under `assign_by` both are group names. Nothing
/// here asks the warehouse which units exist *now* — the pool was pinned at
/// registration. A switchback has neither, and the result is empty.
pub fn assignment_units(cfg: &ExperimentConfig, pool: &[String]) -> Vec<String> {
    let mut units: Vec<String> = cfg
        .design
        .waves
        .iter()
        .flat_map(|w| w.units.iter().cloned())
        .chain(pool.iter().cloned())
        .collect();
    units.sort();
    units.dedup();
    units
}

/// The units every panel read is restricted to (`PanelRequest.restrict_to`): a switchback's
/// pinned fleet; a unit-level waves design's assignment universe ([`assignment_units`]), so a
/// unit that appears later is in neither arm — and never wipes earlier days from the panel,
/// which keeps only the days every unit has. `None` under `assign_by`: the pinned membership
/// already decides which rows are summed, and the two never combine.
pub fn read_units(
    cfg: &ExperimentConfig,
    pool: &[String],
    fleet: Option<&[String]>,
) -> Option<Vec<String>> {
    if cfg.design.switchback.is_some() {
        return fleet.map(<[String]>::to_vec);
    }
    cfg.assign_by.is_none().then(|| assignment_units(cfg, pool))
}

/// What the run must have pinned before an honest assignment exists, and the reason when
/// it did not: the control pool for a waves design, plus the group membership under
/// `assign_by`. A switchback has no control arm, so no pool and no membership; it pins the
/// **fleet** instead, and without it the period means would include any store that opened
/// mid-run.
pub fn missing_pin(
    cfg: &ExperimentConfig,
    pool: Option<&[String]>,
    membership: Option<&GroupMembership>,
    fleet: Option<&[String]>,
) -> Option<&'static str> {
    if cfg.design.switchback.is_some() {
        return fleet.is_none().then_some(NO_FLEET);
    }
    if pool.is_none() {
        return Some(NO_POOL);
    }
    (cfg.assign_by.is_some() && membership.is_none()).then_some(NO_MEMBERSHIP)
}

/// The token after `@` in the `refs` entry: the first 12 hex digits of the
/// registered design's content hash. A registered design is frozen, so it names
/// the design the run executed forever.
pub fn design_ref(content_hash: &str) -> String {
    content_hash.chars().take(12).collect()
}

/// One recorded `experiment_flips` row, as the abort cut reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlipDay {
    pub state: LeverState,
    pub due_at: Moment,
    /// The wave index, or the switchback period's index.
    pub wave: usize,
    pub cancelled: bool,
    /// The row stands for a switch that began: not cancelled, or cancelled because its period
    /// had ended before dispatch (`send::PERIOD_ENDED`) — PR 4's own rule (`record::period_entries`,
    /// `check::periods`). A row cancelled by the abort is a switch the run abandoned.
    pub reached: bool,
}

/// Where an aborted run's measurement stops: the **exclusive** end of every panel read, so
/// no day after the levers went off is read as treated post-period. `None` — read to the
/// registered end — for a run that was not aborted, and when the cut would not end before it.
///
/// The abort day is the earliest `due_at` of the abort's `off` flips (PR 4's `abort_offs`
/// dates them the day the abort was decided). The levers went off during that day, so it is
/// a partial treated day and is excluded. A switchback's own period `off`s are not the abort:
/// they sit at an `off` period's `from` and carry that period's index, where an abort off
/// carries the index of the `on` it turns off. A cancelled flip was never sent and decides
/// nothing.
///
/// With no abort off (nothing was on to turn off: aborted before any `on` went out, or a
/// switchback aborted in an `off` period) the run row carries no abort date, so the cut is the
/// first switch day the run never reached — no flip was recorded for it, so the levers never
/// moved from there on and nothing after it was treated as registered. A switch whose `on`s
/// were recorded but never sent (failed, not cancelled) counts as reached, so the cut moves
/// later: what that reads is untreated days diluting the effect, never post-abort treatment.
pub fn measured_until(
    cfg: &ExperimentConfig,
    aborted: bool,
    flips: &[FlipDay],
) -> Option<NaiveDate> {
    if !aborted {
        return None;
    }
    // Panels are bounded by dates: an intraday cut keeps only the whole days before it.
    let cut = abort_day(cfg, flips)
        .or_else(|| first_unreached_switch(cfg, flips))?
        .date();
    let end = last_post_window_end_date(cfg)?;
    (cut < end).then_some(cut)
}

/// Where an abort that kept its levers stops the measurement: the day it was aborted
/// (`experiment_runs.aborted_on`), exclusive like [`measured_until`]. No `off` was sent,
/// so the flips cannot place the cut; the levers stayed on, so nothing from that day on is
/// read as the registered treatment. `None` when the cut would not end before the
/// registered end.
pub fn kept_levers_cut(cfg: &ExperimentConfig, aborted_on: NaiveDate) -> Option<NaiveDate> {
    let end = last_post_window_end_date(cfg)?;
    (aborted_on < end).then_some(aborted_on)
}

fn abort_day(cfg: &ExperimentConfig, flips: &[FlipDay]) -> Option<Moment> {
    let scheduled_off = |f: &FlipDay| {
        cfg.design.switchback.as_ref().is_some_and(|sb| {
            sb.periods
                .get(f.wave)
                .is_some_and(|p| p.state == LeverState::Off && p.from == f.due_at)
        })
    };
    flips
        .iter()
        .filter(|f| f.state == LeverState::Off && !f.cancelled && !scheduled_off(f))
        .map(|f| f.due_at)
        .min()
}

fn first_unreached_switch(cfg: &ExperimentConfig, flips: &[FlipDay]) -> Option<Moment> {
    oxy_experiment::run::switch_moments(cfg)
        .into_iter()
        .find(|day| !flips.iter().any(|f| f.reached && f.due_at == *day))
}

/// The exclusive end of every panel read: the registered end ([`last_post_window_end`]), or
/// the abort cut when it is earlier. `None` when the design has no post window.
pub fn history_end(cfg: &ExperimentConfig, cut: Option<NaiveDate>) -> Option<NaiveDate> {
    let end = last_post_window_end_date(cfg)?;
    Some(cut.map_or(end, |c| c.min(end)))
}
