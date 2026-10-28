//! The report's fixed rules. Every function here is pure and each rule owns
//! one small function so its test can name it. This file assembles the view and
//! holds the headline sections; the rest live by section: [`super::design`],
//! [`super::prior`], [`super::spillover`] and [`super::validity`].

use crate::config::{Assignment, Direction, measure_label};
use crate::engine_in::EffectIn;

use super::design::{assumptions, cell, design_kind, estimand, projection, treated_total};
use super::prior::{is_directional, prediction, ratio_view, verdict, write_back_view};
use super::spillover::{carryover, substitution};
use super::types::*;
use super::validity::{checks, compliance, lever_note, proposals};
use super::waves::{PROPORTIONAL_ESTIMAND, in_report_scale, is_proportional, signed_pct, waves};

pub fn build_report(i: ReportInput<'_>) -> ReportView {
    // On a proportional scale every number below is already a share.
    let scaled = in_report_scale(i.config, i.target);
    let i = ReportInput {
        target: &scaled,
        ..i
    };
    let proportional = is_proportional(i.config);
    let finite_estimate = i.target.estimate.filter(|e| e.is_finite());
    let refusal = i.target.refusal.clone().or_else(|| {
        finite_estimate.is_none().then(|| {
            "the estimator returned a non-finite estimate and recorded no refusal".to_string()
        })
    });
    // Numbers exist only when nothing refused. Every numeric section keys off this.
    let est = if refusal.is_none() {
        finite_estimate
    } else {
        None
    };
    let kind = design_kind(i.config);
    let alpha = i.config.outcome.alpha;
    let substitution = substitution(&i, kind);
    let status = if i.aborted {
        ReportStatus::Aborted {
            reason: i.abort_reason.map(str::to_string),
            levers_kept: i.levers_kept,
        }
    } else if i.retrospective {
        ReportStatus::Retrospective
    } else {
        ReportStatus::Registered
    };
    let (prediction, prediction_note) = prediction(&i);
    ReportView {
        label: label(&status, i.measured_until),
        status,
        experiment: i.config.experiment.clone(),
        hypothesis: i.config.hypothesis.clone(),
        target: target_label(i.config),
        unit: i.config.for_each.clone(),
        design: kind,
        grain: i.config.design.grain,
        estimand: match &i.config.outcome.denominator {
            Some(_) => RATIO_ESTIMAND.to_string(),
            None if proportional => PROPORTIONAL_ESTIMAND.to_string(),
            None => estimand(kind, i.config.design.grain),
        },
        directional: is_directional(alpha),
        alpha,
        assumptions: assumptions(kind),
        headline: est.map(|e| headline(&i, e, &substitution.statement, kind)),
        // A share summed over unit-days is not a quantity: no totals for a ratio,
        // nor for a proportional change.
        treated_total: est
            .filter(|_| i.config.outcome.denominator.is_none() && !proportional)
            .and_then(|e| treated_total(&i, e, kind)),
        projection: est
            .filter(|_| i.config.outcome.denominator.is_none() && !proportional)
            .and_then(|e| projection(&i, e, kind)),
        direction: direction(&i, est),
        // A promise priced in the measure's units says nothing about a share.
        promise: est.filter(|_| !proportional).and_then(|e| promise(&i, e)),
        compliance: compliance(&i),
        checks: checks(i.target),
        proposals: proposals(
            i.proposal_count,
            i.config.design.assignment == Assignment::Observed,
            i.hand_assigned,
        ),
        ratio: i.ratio.map(ratio_view),
        prediction,
        prediction_note,
        write_back: i.write_back.map(write_back_view),
        lever_note: lever_note(i.distinct_lever_revisions),
        carryover: carryover(&i, est),
        seeds: i.seeds,
        mde_note: mde_note(&i),
        share: i.config.outcome.denominator.is_some(),
        proportional,
        waves: waves(i.config, i.target, est),
        refusal,
        substitution,
    }
}

pub(super) fn num(x: f64) -> String {
    format!("{x:.2}")
}

pub(super) fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

pub(super) fn finite(x: Option<f64>) -> Option<f64> {
    x.filter(|v| v.is_finite())
}

pub(super) fn interval(low: Option<f64>, high: Option<f64>) -> Interval {
    Interval {
        low: finite(low),
        high: finite(high),
    }
}

pub(super) fn summary(e: &EffectIn) -> EffectSummary {
    EffectSummary {
        estimate: finite(e.estimate),
        interval: interval(e.ci_low, e.ci_high),
        significant: e.significant,
        refusal: e.refusal.clone(),
    }
}

/// Pre-registration is what lets a plain two-sided test stand without a multiplicity
/// correction (spec, "Pre-registration is what removes the multiplicity correction"). A
/// design written after its data existed had no such commitment, so its label says so first.
const RETROSPECTIVE: &str = "RETROSPECTIVE — not pre-registered: the design was written after \
                             its data existed, so this is an exploratory reading, not a registered \
                             test; its interval and p-value carry no multiplicity correction";

fn label(status: &ReportStatus, measured_until: Option<chrono::NaiveDate>) -> String {
    let (aborted, levers_kept) = match status {
        ReportStatus::Registered => return "Registered result".to_string(),
        ReportStatus::Retrospective => return RETROSPECTIVE.to_string(),
        ReportStatus::Interim { through } => return crate::interim::label(*through),
        ReportStatus::Aborted {
            reason: Some(r),
            levers_kept,
        } => (
            format!("ABORTED RUN — not the registered result ({r})"),
            *levers_kept,
        ),
        ReportStatus::Aborted {
            reason: None,
            levers_kept,
        } => (
            "ABORTED RUN — not the registered result".to_string(),
            *levers_kept,
        ),
    };
    match (measured_until, levers_kept) {
        // Exclusive: the levers went off during that day, so it is not read either.
        (Some(d), false) => format!(
            "{aborted}; the measurement stops before {d}, where the abort ended the run's switches; \
             that day and later are not read"
        ),
        (Some(d), true) => format!(
            "{aborted}; the measurement stops before {d}, the day the run was aborted; its levers \
             were left as they were, so that day and later are not read"
        ),
        (None, true) => format!("{aborted}; its levers were left as they were"),
        (None, false) => aborted,
    }
}

/// Gross estimate ± CI half-width, then the floor. Never net. A directional result says so in
/// the first words — never plain "significant".
fn headline(i: &ReportInput<'_>, est: f64, substitution: &str, kind: DesignKind) -> Headline {
    let t = i.target;
    let half_width = match (finite(t.ci_low), finite(t.ci_high)) {
        (Some(l), Some(h)) => Some((h - l) / 2.0),
        _ => None,
    };
    let proportional = is_proportional(i.config);
    // A share's interval is not symmetric about it: state both ends.
    let spread = match (proportional, finite(t.ci_low), finite(t.ci_high)) {
        (true, Some(l), Some(h)) => format!("{} to {}", signed_pct(l), signed_pct(h)),
        (_, _, _) => half_width.map_or_else(
            || "interval unbounded".to_string(),
            |h| format!("±{}", num(h)),
        ),
    };
    let half_width = half_width.filter(|_| !proportional);
    let alpha = i.config.outcome.alpha;
    let verdict = verdict(t.significant, alpha);
    // A ratio is one number per unit and window, not per unit-day.
    let per = match &i.config.outcome.denominator {
        Some(_) => "unit".to_string(),
        None => cell(kind, i.config.design.grain),
    };
    let figure = if proportional {
        signed_pct(est)
    } else {
        format!("{est:+.2}")
    };
    let base = format!(
        "gross {figure} per {per} on {} ({spread}); {substitution}",
        target_label(i.config)
    );
    let text = if t.significant && is_directional(alpha) {
        format!("{verdict}: {base}")
    } else {
        base
    };
    Headline {
        estimate: est,
        half_width,
        interval: interval(t.ci_low, t.ci_high),
        p_value: finite(t.p_value),
        significant: t.significant,
        verdict,
        text,
    }
}

fn direction(i: &ReportInput<'_>, est: Option<f64>) -> DirectionView {
    let predicted_up = matches!(i.config.outcome.direction, Direction::Increase);
    let observed = match est {
        Some(e) if e > 0.0 => "increase",
        Some(e) if e < 0.0 => "decrease",
        _ => "none",
    };
    let message = match (predicted_up, observed) {
        (true, "decrease") => Some("you predicted up, it went down".to_string()),
        (false, "increase") => Some("you predicted down, it went up".to_string()),
        _ => None,
    };
    DirectionView {
        predicted: if predicted_up { "increase" } else { "decrease" }.to_string(),
        observed: observed.to_string(),
        mismatch: message.is_some(),
        message,
    }
}

fn promise(i: &ReportInput<'_>, est: f64) -> Option<PromiseView> {
    let m = i.config.outcome.mde_at_proposal?;
    let abs = est.abs();
    let below = abs < m;
    let verb = if below { "smaller than" } else { "at least" };
    Some(PromiseView {
        mde_at_proposal: m,
        abs_estimate: abs,
        below_promise: below,
        message: format!(
            "the measured effect ({}) is {verb} the smallest effect this design promised to read ({})",
            num(abs),
            num(m)
        ),
    })
}

/// No promise to compare with: say so, and why when pricing gave a reason.
fn mde_note(i: &ReportInput<'_>) -> Option<String> {
    if i.config.outcome.mde_at_proposal.is_some() {
        return None;
    }
    let base = "MDE unavailable: no smallest detectable effect was priced for this design";
    Some(match i.mde_unavailable {
        Some(why) => format!("{base} (pricing: {why})"),
        None => base.to_string(),
    })
}

const RATIO_ESTIMAND: &str = "mean change in each unit's ratio of sums (post window \u{2212} pre \
                              window), unweighted across units";

/// The target as the report names it: `target ÷ denominator` for a ratio.
fn target_label(c: &crate::config::ExperimentConfig) -> String {
    let target = measure_label(&c.outcome.target, &c.outcome.target_filter);
    match &c.outcome.denominator {
        Some(d) => format!("{target} \u{f7} {d}"),
        None => target,
    }
}
