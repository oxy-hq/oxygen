//! Pricing a waves candidate: units or groups, strata, a candidate's own α.
//! Pure, like the rest of pricing; every message states only what its own
//! guard tested. `price` re-exports [`price_waves`].

use chrono::{DateTime, Utc};
use oxy_airlayer_compat::engine::experiment::{
    DesignShape, DesignSpec, PanelMatrix, propose_strata, propose_waves,
};
use oxy_experiment::Moment;
use oxy_experiment::{ExperimentConfig, Grain};

use super::candidate::WavesCandidate;
use super::panel::GroupMembership;
use super::panel_groups::mean_group_size;
use super::price::{
    PriceComputation, Priced, RunParams, SchedulePlan, config_problems, deadline_warning,
    first_date_refusal, history_to, priced_power, refused_with_problems,
};
use super::snippet::{WavePlan, plan_waves};

/// A waves candidate's timing warnings.
fn timing_warnings(cfg: &ExperimentConfig, first_day: Moment, now: DateTime<Utc>) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.design.washout_days == 0 && cfg.design.grain == Grain::Day {
        out.push(
            "washout_days is 0: the lever flips as the switch day begins, so a partial first day will land in the post window"
                .to_string(),
        );
    }
    out.extend(deadline_warning(cfg, first_day, now));
    out
}

// ── the waves core ──────────────────────────────────────────────────────────

fn describe(by: Option<&str>) -> String {
    by.map_or_else(
        || "a unit-level design".to_string(),
        |b| format!("`assign_by: {b}`"),
    )
}

/// The draft as this candidate would register it: its grouping and α laid over
/// the file, pricing's own output (`strata`) cleared, stale waves for another
/// grouping dropped (with a warning). Also the warnings about draft keys the
/// pasted snippet would leave behind.
fn waves_config(cfg: &ExperimentConfig, c: &WavesCandidate) -> (ExperimentConfig, Vec<String>) {
    let mut eff = cfg.clone();
    let mut notes = Vec::new();
    eff.design.switchback = None;
    if eff.assign_by != c.assign_by {
        if !eff.design.waves.is_empty() {
            notes.push(format!(
                "the draft's waves and strata were written for {} and were ignored: this candidate is {}",
                describe(eff.assign_by.as_deref()),
                describe(c.assign_by.as_deref())
            ));
            eff.design.waves.clear();
        }
        if let (Some(old), None) = (&eff.assign_by, &c.assign_by) {
            notes.push(format!(
                "the draft sets `assign_by: {old}`, which this unit-level candidate does not use: delete it before pasting this snippet, or the file will be registered as a group design"
            ));
        }
        eff.assign_by = c.assign_by.clone();
    }
    eff.design.strata = None;
    if let Some(alpha) = c.alpha {
        eff.outcome.alpha = alpha;
    }
    (eff, notes)
}

/// `pre_days` and `post_days`, or a refusal naming the ones the draft lacks.
fn windows(eff: &ExperimentConfig) -> Result<(u32, u32), String> {
    match (eff.design.pre_days, eff.design.post_days) {
        (Some(pre), Some(post)) => Ok((pre, post)),
        (pre, post) => {
            let missing: Vec<&str> = [
                (pre.is_none(), "`design.pre_days`"),
                (post.is_none(), "`design.post_days`"),
            ]
            .into_iter()
            .filter_map(|(absent, key)| absent.then_some(key))
            .collect();
            Err(format!(
                "a waves candidate measures pre and post windows, and the draft does not set {}",
                missing.join(" or ")
            ))
        }
    }
}

/// Rank the panel's units by their mean over the last `pre_days` of history and
/// cut the ranking into `blocks` strata; `None` for an unblocked draw.
fn derive_strata(
    panel: &PanelMatrix,
    blocks: Option<usize>,
    pre_days: u32,
    history_to: i64,
) -> Result<Option<Vec<Vec<String>>>, String> {
    blocks
        .map(|b| propose_strata(panel, b, history_to - i64::from(pre_days), history_to))
        .transpose()
        .map_err(|e| format!("no strata could be proposed: {e}"))
}

/// Refuse a proposal that places every unit (or group) on ONE switch date, leaving
/// no control at any time. `validate_against_units` refuses the same shape in a
/// file (`NoControlUnits`). Every unit placed over two or more waves is a fully
/// staggered rollout and prices: the later waves control the earlier ones, and
/// the estimator drops the last ([`fully_staggered_warning`]).
///
/// Runs before `propose_waves`, from the requested sizes: a sum above `n` falls
/// through to the engine's refusal.
fn every_unit_placed(
    cfg: &ExperimentConfig,
    c: &WavesCandidate,
    sizes: &[usize],
    n: usize,
) -> Option<String> {
    (sizes.len() == 1 && sizes.iter().sum::<usize>() == n).then(|| {
        let what = match &c.assign_by {
            Some(by) => format!("groups of `{by}`"),
            None => format!("units of `{}`", cfg.for_each),
        };
        format!(
            "all {n} {what} are placed in one wave, so none is left as a control: with no unit held back the treatment is a function of the calendar and no later analysis can recover the effect. Hold some back, or stagger them over two or more waves"
        )
    })
}

/// Every unit placed over several waves: say that the last wave is not estimated.
fn fully_staggered_warning(sizes: &[usize], n: usize) -> Option<String> {
    (sizes.len() > 1 && sizes.iter().sum::<usize>() == n).then(|| {
        "every unit is placed, so no unit is held back: each wave is measured against the waves not yet switched on, and the last wave, which has none, is not estimated (its units still get the change). Keep the waves further apart than washout + post days, or a later wave is no clean control".to_string()
    })
}

fn waves_spec(
    eff: &ExperimentConfig,
    c: &WavesCandidate,
    waves: &[WavePlan],
    n_units: usize,
    run: &RunParams,
) -> DesignSpec {
    let placed: usize = waves.iter().map(|w| w.units.len()).sum();
    let (shape, iterations) = if let [only] = waves {
        (
            DesignShape::CommonDate {
                n_treated: only.units.len(),
                n_control: n_units - placed,
            },
            // Read by permutation, a common date is priced as one wave is.
            if eff.design.one_date_test.is_welch() {
                run.iterations.common_date
            } else {
                run.iterations.staggered
            },
        )
    } else {
        (
            DesignShape::Staggered {
                wave_sizes: waves.iter().map(|w| w.units.len()).collect(),
                spacing_days: usize::try_from(c.spacing_days).unwrap_or(0),
                n_never_treated: n_units - placed,
            },
            run.iterations.staggered,
        )
    };
    DesignSpec {
        shape,
        pre_days: eff.design.pre_days.unwrap_or_default() as usize,
        post_days: eff.design.post_days.unwrap_or_default() as usize,
        anticipation_days: eff.design.anticipation_days as usize,
        washout_days: eff.design.washout_days as usize,
        coverage_floor: eff.design.coverage_floor,
        alpha: eff.outcome.alpha,
        family: eff.outcome.family as usize,
        power: eff.outcome.power,
        iterations,
        history_to: Some(history_to(eff, run)),
        blocks: c.blocks.unwrap_or(0),
        one_date_test: crate::server::experiment::assignment::engine_one_date_test(
            eff.design.one_date_test,
        ),
    }
}

/// `power.mde` on the comparable scale: a unit design's is already per
/// unit-day; a group design's is divided by the mean group size (index C2b),
/// with the warning that says what that assumes.
fn unit_day(
    mde: f64,
    c: &WavesCandidate,
    membership: Option<&GroupMembership>,
    groups: &[String],
) -> Result<(f64, Option<String>), String> {
    match (&c.assign_by, membership) {
        (None, _) => Ok((mde, None)),
        (Some(by), Some(m)) => match mean_group_size(m, groups) {
            Some(size) if size > 0.0 => Ok((
                mde / size,
                Some(format!(
                    "mde_unit_day divides the group-level minimum detectable effect by the mean group size ({size:.1} members of `{by}` groups): it assumes the effect spreads evenly over a group's members"
                )),
            )),
            _ => Err(format!(
                "no group of `{by}` in the panel has members in the membership, so its minimum detectable effect cannot be put on a per-unit scale"
            )),
        },
        (Some(by), None) => Err(format!(
            "`assign_by: {by}` was requested but no group membership was loaded"
        )),
    }
}

/// Price one waves candidate: lay it over the draft, validate, draw (blocked if
/// asked), refuse the unidentified design, run the placebo.
pub fn price_waves(
    cfg: &ExperimentConfig,
    panel: &PanelMatrix,
    c: &WavesCandidate,
    run: &RunParams,
) -> PriceComputation {
    let units = panel.units();
    if c.pairs && units.len() < 4 {
        return PriceComputation::refused(format!(
            "matched pairs need at least 4 units (or groups) to make 2 pairs; the panel holds {}",
            units.len()
        ));
    }
    let resolved = c.resolved(units.len());
    let c = &resolved;
    let (eff, mut warnings) = waves_config(cfg, c);
    let problems = config_problems(&eff, units);
    if !problems.is_empty() {
        return refused_with_problems(problems);
    }
    let (pre, _post) = match windows(&eff) {
        Ok(w) => w,
        Err(reason) => return PriceComputation::refused(reason),
    };
    let strata = match derive_strata(panel, c.blocks, pre, history_to(&eff, run)) {
        Ok(s) => s,
        Err(reason) => return PriceComputation::refused(reason),
    };
    if let Some(reason) = every_unit_placed(&eff, c, &c.wave_sizes, units.len()) {
        return PriceComputation::refused(reason);
    }
    if let Some(reason) = first_date_refusal(&eff, c.first_switch_on) {
        return PriceComputation::refused(reason);
    }
    let first_day = eff.design.grain.ordinal_of(c.first_switch_on);
    // The engine refuses `spacing_days < 1` even for one wave, where it is unused.
    let spacing = if c.wave_sizes.len() == 1 {
        1
    } else {
        c.spacing_days
    };
    let proposed = match propose_waves(
        units,
        &c.wave_sizes,
        first_day,
        spacing,
        strata.as_deref(),
        run.seed,
    ) {
        Ok(w) => w,
        Err(e) => return PriceComputation::refused(format!("no waves could be proposed: {e}")),
    };
    let waves = match plan_waves(eff.design.grain, &proposed) {
        Ok(w) => w,
        Err(e) => {
            return PriceComputation::refused(format!(
                "the proposed waves could not be dated: {e}"
            ));
        }
    };
    let spec = waves_spec(&eff, c, &waves, units.len(), run);
    let power = match priced_power(panel, &spec, run.seed) {
        Ok(p) => p,
        Err(reason) => return PriceComputation::refused(reason),
    };
    let (mde_unit_day, scale_note) = match unit_day(power.mde, c, run.membership.as_ref(), units) {
        Ok(v) => v,
        Err(reason) => return PriceComputation::refused(reason),
    };
    warnings.extend(scale_note);
    warnings.extend(timing_warnings(&eff, c.first_switch_on, run.now));
    warnings.extend(fully_staggered_warning(&c.wave_sizes, units.len()));
    PriceComputation::Priced(Priced {
        seed: run.seed,
        alpha: eff.outcome.alpha,
        assign_by: c.assign_by.clone(),
        plan: SchedulePlan::Waves(waves),
        strata,
        power,
        mde_unit_day,
        warnings,
    })
}
