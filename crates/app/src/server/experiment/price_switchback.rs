//! Pricing a switchback: a seeded paired schedule put through the placebo.
//! Pure, like the rest of pricing; every message states only what its guard tested.

use oxy_airlayer_compat::engine::experiment::{
    DesignShape, DesignSpec, PanelMatrix, propose_switchback,
};
use oxy_experiment::{ExperimentConfig, Grain, SwitchbackConfig};

use super::candidate::SwitchbackCandidate;
use super::price::{
    PriceComputation, Priced, RunParams, SchedulePlan, config_problems, deadline_warning,
    first_date_refusal, history_to, priced_power, refused_with_problems,
};
use super::snippet::plan_periods;

/// The keys of the draft a switchback refuses (`SwitchbackForbids`, PR 2).
fn forbidden_keys(cfg: &ExperimentConfig) -> Vec<&'static str> {
    [
        (!cfg.design.waves.is_empty(), "design.waves"),
        (cfg.design.strata.is_some(), "design.strata"),
        (cfg.assign_by.is_some(), "assign_by"),
        (cfg.design.pre_days.is_some(), "design.pre_days"),
        (cfg.design.post_days.is_some(), "design.post_days"),
        (
            cfg.design.anticipation_days != 0,
            "design.anticipation_days (non-zero)",
        ),
    ]
    .into_iter()
    .filter_map(|(present, key)| present.then_some(key))
    .collect()
}

/// The draft as this candidate would register it: a switchback of the candidate's
/// period, without the keys a switchback refuses. The warning names those keys,
/// because pasting this proposal's snippet into the draft would otherwise fail compile.
fn switchback_config(
    cfg: &ExperimentConfig,
    c: &SwitchbackCandidate,
) -> (ExperimentConfig, Vec<String>) {
    let mut notes = Vec::new();
    let keys = forbidden_keys(cfg);
    if !keys.is_empty() {
        let named: Vec<String> = keys.iter().map(|k| format!("`{k}`")).collect();
        notes.push(format!(
            "the draft carries {}, which a switchback does not take: delete {} before pasting this snippet, or the file will not compile",
            named.join(", "),
            if keys.len() == 1 { "it" } else { "them" }
        ));
    }
    let mut eff = cfg.clone();
    eff.design.waves.clear();
    eff.design.strata = None;
    eff.assign_by = None;
    eff.design.pre_days = None;
    eff.design.post_days = None;
    eff.design.anticipation_days = 0;
    eff.design.switchback = Some(SwitchbackConfig {
        period_days: c.period_days,
        periods: Vec::new(),
    });
    if let Some(alpha) = c.alpha {
        eff.outcome.alpha = alpha;
    }
    (eff, notes)
}

fn switchback_spec(eff: &ExperimentConfig, c: &SwitchbackCandidate, run: &RunParams) -> DesignSpec {
    DesignSpec {
        shape: DesignShape::Switchback {
            period_days: c.period_days as usize,
            pairs: c.pairs,
        },
        pre_days: 0,
        post_days: 0,
        anticipation_days: 0,
        washout_days: eff.design.washout_days as usize,
        coverage_floor: eff.design.coverage_floor,
        alpha: eff.outcome.alpha,
        family: eff.outcome.family as usize,
        power: eff.outcome.power,
        iterations: run.iterations.switchback,
        history_to: Some(history_to(eff, run)),
        blocks: 0,
    }
}

fn switchback_warnings(
    eff: &ExperimentConfig,
    c: &SwitchbackCandidate,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<String> {
    let mut out = Vec::new();
    let grain = eff.design.grain;
    if eff.design.washout_days == 0 && grain == Grain::Day {
        out.push(
            "washout_days is 0: the lever flips as each period begins, so a partial first day will land in every period"
                .to_string(),
        );
    }
    if grain == Grain::Day && c.period_days % 7 != 0 {
        out.push(format!(
            "periods of {} days are not whole weeks: each period holds some weekdays more often than others and carries weekday swings into the null; whole weeks (7 or 14 days) hold every weekday equally",
            c.period_days
        ));
    }
    out.push(
        "a switchback measures the short-run effect of on against off under alternation, not the effect of leaving the lever on; it assumes carryover ends within washout_days and no pull-forward (a shopper who buys in an on period does not skip the next off one)"
            .to_string(),
    );
    out.extend(deadline_warning(eff, c.first_period_on, now));
    out
}

/// Price one switchback candidate: lay it over the draft, validate, propose the
/// paired schedule, run the placebo.
pub fn price_switchback(
    cfg: &ExperimentConfig,
    panel: &PanelMatrix,
    c: &SwitchbackCandidate,
    run: &RunParams,
) -> PriceComputation {
    let (eff, mut warnings) = switchback_config(cfg, c);
    let problems = config_problems(&eff, panel.units());
    if !problems.is_empty() {
        return refused_with_problems(problems);
    }
    if let Some(reason) = first_date_refusal(&eff, c.first_period_on) {
        return PriceComputation::refused(reason);
    }
    let grain = eff.design.grain;
    let proposed = propose_switchback(
        grain.ordinal_of(c.first_period_on),
        i64::from(c.period_days),
        c.pairs,
        run.seed,
    );
    let periods = match plan_periods(grain, &proposed) {
        Ok(p) => p,
        Err(e) => {
            return PriceComputation::refused(format!(
                "the proposed periods could not be dated: {e}"
            ));
        }
    };
    let power = match priced_power(panel, &switchback_spec(&eff, c, run), run.seed) {
        Ok(p) => p,
        Err(reason) => return PriceComputation::refused(reason),
    };
    warnings.extend(switchback_warnings(&eff, c, run.now));
    PriceComputation::Priced(Priced {
        seed: run.seed,
        alpha: eff.outcome.alpha,
        assign_by: None,
        plan: SchedulePlan::Switchback {
            period_days: c.period_days,
            periods,
        },
        strata: None,
        mde_unit_day: power.mde,
        power,
        warnings,
    })
}
