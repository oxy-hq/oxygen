//! The registered alpha's label, the World Model's prediction against the measurement, and
//! the ratio and coefficient write-back the measurement proposes to it.

use crate::config::{PriorCoefficient, PriorSource};
use crate::engine_in::RatioIn;
use crate::patch::{CoefficientEdit, PatchRefusal};

use super::build::{finite, interval, num, summary};
use super::types::*;

/// The spec's line between a plain and a directional experiment (`internal-docs/experiments.md`
/// § Directional experiments). A rule of the report, not a tunable: the registered alpha is a
/// config field and is read from there.
pub const DIRECTIONAL_ABOVE_ALPHA: f64 = 0.05;

pub fn is_directional(alpha: f64) -> bool {
    alpha > DIRECTIONAL_ABOVE_ALPHA
}

/// What the headline calls the result. A directional result that clears its bar is never plain
/// "significant"; a result that does not clear it is not directional either.
pub fn verdict(significant: bool, alpha: f64) -> String {
    match (significant, is_directional(alpha)) {
        (true, true) => format!("directional at α = {alpha}"),
        (true, false) => format!("significant at α = {alpha}"),
        (false, _) => format!("not distinguishable from zero at α = {alpha}"),
    }
}

fn source_word(s: PriorSource) -> &'static str {
    match s {
        PriorSource::Declared => "declared",
        PriorSource::Fitted => "fitted",
    }
}

/// The engine reached the Anderson–Rubin inversion (a finite coefficient over a measured first
/// stage) and the set runs to infinity on at least one side. airlayer's `estimate_ratio` words
/// that as a `refusal` (unbounded side, weak first stage, not one interval), yet it is a
/// reported interval, not a refusal: a genuine refusal never produces a finite coefficient.
pub(super) fn ar_unbounded(r: &RatioIn) -> bool {
    finite(r.coefficient).is_some()
        && r.first_stage.refusal.is_none()
        && finite(r.first_stage.estimate).is_some()
        && (finite(r.ci_low).is_none() || finite(r.ci_high).is_none())
}

/// `(comparison, why-not)`. Nothing frozen is nothing to say; a frozen prior that cannot be
/// compared is always explained.
pub fn prediction(i: &ReportInput<'_>) -> (Option<PredictionView>, Option<String>) {
    let Some(prior) = &i.config.outcome.prior_coefficient else {
        return (None, None);
    };
    let unavailable = |why: &str| {
        let note = format!(
            "the World Model's {} prior {} was not compared: {why}",
            source_word(prior.source),
            num(prior.value)
        );
        (None, Some(note))
    };
    if i.aborted {
        return unavailable(
            "the run was aborted, and a truncated measurement is not a test of the slope",
        );
    }
    let Some(ratio) = i.ratio else {
        return unavailable("no driver measure was registered, so no first stage was measured");
    };
    if let Some(why) = ratio
        .refusal
        .as_ref()
        .filter(|_| !ar_unbounded(ratio))
        .or(ratio.first_stage.refusal.as_ref())
    {
        return unavailable(&format!("the first stage was refused: {why}"));
    }
    if let Some(why) = &i.target.refusal {
        return unavailable(&format!("the target estimate was refused: {why}"));
    }
    let measured = finite(i.target.estimate).filter(|_| i.target.refusal.is_none());
    let (Some(first_stage), Some(measured)) = (finite(ratio.first_stage.estimate), measured) else {
        return unavailable(
            "the lever's effect on the driver or on the target is not a finite number",
        );
    };
    // Either side unbounded → no verdict: under a weak first stage the set can be a complement or
    // the whole line, and a one-sided bound does not establish membership (index C2b).
    let in_set = match (finite(ratio.ci_low), finite(ratio.ci_high)) {
        (Some(lo), Some(hi)) => Some(lo <= prior.value && prior.value <= hi),
        _ => None,
    };
    let view = PredictionView {
        predicted: prior.value * first_stage,
        measured,
        measured_interval: interval(i.target.ci_low, i.target.ci_high),
        prior: PriorView {
            value: prior.value,
            source: prior.source,
        },
        prior_in_interval: in_set,
        statement: statement(prior, in_set, ratio),
    };
    (Some(view), None)
}

fn statement(prior: &PriorCoefficient, in_set: Option<bool>, ratio: &RatioIn) -> String {
    let (source, value) = (source_word(prior.source), num(prior.value));
    let set = match (finite(ratio.ci_low), finite(ratio.ci_high)) {
        (Some(lo), Some(hi)) => format!("[{}, {}]", num(lo), num(hi)),
        _ => "(unbounded)".to_string(),
    };
    match in_set {
        Some(true) => format!(
            "the experiment agrees with the World Model's slope: the {source} prior {value} lies inside the \
             Anderson–Rubin set {set} for the earned coefficient"
        ),
        Some(false) => {
            let base = format!(
                "the experiment contradicts the World Model's slope: the {source} prior {value} lies outside the \
                 Anderson–Rubin set {set} for the earned coefficient"
            );
            match prior.source {
                PriorSource::Fitted => format!(
                    "{base}. A fitted slope contradicted by an intervention is observational history meeting a \
                     deliberate change — the comparison most worth recording."
                ),
                PriorSource::Declared => base,
            }
        }
        None => format!(
            "the {source} prior {value} can be neither confirmed nor contradicted: the earned coefficient's \
             Anderson–Rubin set is unbounded (a weak first stage)"
        ),
    }
}

pub(super) fn ratio_view(r: &RatioIn) -> RatioView {
    RatioView {
        coefficient: finite(r.coefficient),
        interval: interval(r.ci_low, r.ci_high),
        unbounded: ar_unbounded(r),
        first_stage: summary(&r.first_stage),
        refusal: r.refusal.clone(),
    }
}

pub(super) fn write_back_view(w: &Result<CoefficientEdit, PatchRefusal>) -> WriteBackView {
    match w {
        Ok(e) => WriteBackView::Proposed {
            coefficient: e.coefficient,
            target: e.target_measure.clone(),
            driver: e.driver_measure.clone(),
            file_path: e.file_path.clone(),
            refs_entry: e.refs_entry.clone(),
            replaces: e.replaces.clone(),
        },
        Err(r) => WriteBackView::Refused {
            reason: r.to_string(),
        },
    }
}
