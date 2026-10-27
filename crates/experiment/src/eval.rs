//! The pure judge behind the ramen-shop evaluation (`ramen_shop_judge` example).
//!
//! It never talks to a workspace: it takes a sealed [`ReportView`] and the lifts the
//! simulator *planted*, and says whether the report's own numbers are consistent with
//! that truth. Every function returns a [`Check`] whose verdict is `Inconclusive`
//! rather than `Pass` whenever the report could not have been wrong (refused,
//! unbounded) — a vacuous pass is exactly what an evaluation must not produce.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::report::{Interval, ReportView};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Planted {
    /// Store → the lift added to the TARGET per prompted store-day. A store that never
    /// offers a drink is present with `0.0` — absent means a typo, not a zero.
    pub per_unit_day: BTreeMap<String, f64>,
    /// Target lift per unit of driver lift.
    pub slope: f64,
    /// Relative distance from `slope` within which a prior is "the same slope".
    pub slope_tolerance: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Units {
        treated: Vec<String>,
    },
    /// Treated group → its members. A group's lift is the SUM of its members'.
    Groups {
        treated: BTreeMap<String, Vec<String>>,
    },
    /// A switchback treats every store on its on-days.
    Fleet,
}

impl Planted {
    /// The estimand the report states, planted. `None` when a named unit has no planted
    /// lift or nothing is treated.
    pub fn truth(&self, scope: &Scope) -> Option<f64> {
        let get = |u: &String| self.per_unit_day.get(u).copied();
        let lifts: Option<Vec<f64>> = match scope {
            Scope::Units { treated } => treated.iter().map(get).collect(),
            Scope::Groups { treated } => treated
                .values()
                .map(|ms| ms.iter().map(get).sum::<Option<f64>>())
                .collect(),
            Scope::Fleet => Some(self.per_unit_day.values().copied().collect()),
        };
        lifts
            .filter(|l| !l.is_empty())
            .map(|l| l.iter().sum::<f64>() / l.len() as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Miss,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub verdict: Verdict,
    pub detail: String,
}

fn check(name: &'static str, verdict: Verdict, detail: impl Into<String>) -> Check {
    Check {
        name,
        verdict,
        detail: detail.into(),
    }
}

fn bounds(i: &Interval) -> Option<(f64, f64)> {
    Some((i.low?, i.high?))
}

/// The interval the report printed covers the planted estimand.
pub fn interval_covers(name: &'static str, report: &ReportView, truth: f64) -> Check {
    if let Some(why) = &report.refusal {
        return check(
            name,
            Verdict::Inconclusive,
            format!("the report was refused: {why}"),
        );
    }
    let Some((lo, hi)) = report.headline.as_ref().and_then(|h| bounds(&h.interval)) else {
        return check(
            name,
            Verdict::Inconclusive,
            "the interval is unbounded (or absent), so coverage is vacuous",
        );
    };
    let detail = format!("planted {truth:.2}; reported interval [{lo:.2}, {hi:.2}]");
    let verdict = if lo <= truth && truth <= hi {
        Verdict::Pass
    } else {
        Verdict::Miss
    };
    check(name, verdict, detail)
}

/// The null run: nothing was planted, so the report must not claim an effect.
pub fn claims_no_effect(report: &ReportView) -> Check {
    const NAME: &str = "null_claims_no_effect";
    if let Some(why) = &report.refusal {
        return check(
            NAME,
            Verdict::Inconclusive,
            format!("the null run was refused: {why}"),
        );
    }
    let Some(h) = &report.headline else {
        return check(NAME, Verdict::Inconclusive, "the report has no headline");
    };
    if h.significant {
        check(
            NAME,
            Verdict::Miss,
            format!(
                "claimed `{}` (estimate {:+.2}) with nothing planted",
                h.verdict, h.estimate
            ),
        )
    } else {
        check(
            NAME,
            Verdict::Pass,
            format!("did not claim an effect: `{}`", h.verdict),
        )
    }
}

/// The store whose staff never offer a drink reads back as landed, so compliance cannot
/// see it. If it drew a treated wave it dilutes the intention-to-treat average, and the
/// interval must cover the DILUTED number. If it drew control, the dilution was not
/// exercised and the run says so rather than passing.
pub fn non_responder_is_itt(
    report: &ReportView,
    planted: &Planted,
    treated: &[String],
    non_responder: &str,
) -> Check {
    const NAME: &str = "non_responder_itt";
    if !treated.iter().any(|u| u == non_responder) {
        return check(
            NAME,
            Verdict::Inconclusive,
            format!("`{non_responder}` drew control in this run: the dilution was not exercised"),
        );
    }
    let with = planted.truth(&Scope::Units {
        treated: treated.to_vec(),
    });
    let without_units: Vec<String> = treated
        .iter()
        .filter(|u| *u != non_responder)
        .cloned()
        .collect();
    let without = planted.truth(&Scope::Units {
        treated: without_units,
    });
    let (Some(diluted), Some(responders)) = (with, without) else {
        return check(
            NAME,
            Verdict::Inconclusive,
            "the planted lifts do not cover every treated store",
        );
    };
    if (responders - diluted).abs() < 1e-9 {
        return check(
            NAME,
            Verdict::Inconclusive,
            "the non-responder's planted lift equals the average: nothing to dilute",
        );
    }
    let mut c = interval_covers(NAME, report, diluted);
    c.detail = format!("{} (responders-only would be {responders:.2})", c.detail);
    c
}

/// The earned slope's interval covers the planted slope.
pub fn slope_covered(report: &ReportView, planted: &Planted) -> Check {
    const NAME: &str = "slope_covered";
    let Some(r) = &report.ratio else {
        return check(NAME, Verdict::Inconclusive, "the report carries no ratio");
    };
    if let Some(why) = &r.refusal {
        return check(
            NAME,
            Verdict::Inconclusive,
            format!("the ratio was refused: {why}"),
        );
    }
    let Some((lo, hi)) = bounds(&r.interval) else {
        return check(
            NAME,
            Verdict::Inconclusive,
            "the slope's set is unbounded, so coverage is vacuous",
        );
    };
    let verdict = if lo <= planted.slope && planted.slope <= hi {
        Verdict::Pass
    } else {
        Verdict::Miss
    };
    check(
        NAME,
        verdict,
        format!(
            "planted slope {:.2}; reported set [{lo:.2}, {hi:.2}]",
            planted.slope
        ),
    )
}

/// Predicted-against-measured where the right answer is known: a prior within
/// `slope_tolerance` of the planted slope should be *agreed with*; one further off
/// should be *contradicted*. A report that says the opposite is a miss to examine —
/// it may be a wide set honestly failing to separate them, not a bug.
pub fn prior_verdict_matches_truth(report: &ReportView, planted: &Planted) -> Check {
    const NAME: &str = "prior_verdict";
    let Some(p) = &report.prediction else {
        let why = report
            .prediction_note
            .clone()
            .unwrap_or_else(|| "no prior_coefficient was frozen".to_string());
        return check(NAME, Verdict::Inconclusive, why);
    };
    let Some(said_agrees) = p.prior_in_interval else {
        return check(
            NAME,
            Verdict::Inconclusive,
            "the earned set is unbounded: the prior is neither confirmed nor contradicted",
        );
    };
    let near =
        (p.prior.value - planted.slope).abs() <= planted.slope_tolerance * planted.slope.abs();
    let detail = format!(
        "prior {:.2} ({:?}) against planted {:.2}: the report says it {}",
        p.prior.value,
        p.prior.source,
        planted.slope,
        if said_agrees { "agrees" } else { "contradicts" }
    );
    check(
        NAME,
        if said_agrees == near {
            Verdict::Pass
        } else {
            Verdict::Miss
        },
        detail,
    )
}
