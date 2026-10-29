//! A synthetic control read from the target panel: the effect the headline
//! states, and each treated unit's fit the report draws.
//!
//! The spec comes from `assignment::to_engine_design` over the pinned pool. What
//! the panel lacks is reconciled here first: a treated unit with no rows is
//! dropped and listed on a reading of past data (the strict path lets the engine
//! refuse it), a donor with no rows simply cannot be a donor and is listed, and
//! an excluded unit the panel never read is out of the pool already.

use oxy_airlayer_compat::engine::experiment::{
    DroppedUnit, PanelMatrix, SyntheticSpec, estimate_synthetic,
};
use oxy_experiment::engine_in::{EffectIn, SyntheticIn};
use serde_json::{Value, json};

use super::mirror::Judged;
use super::target_estimate::Read;

pub struct SyntheticRead {
    pub effect: Judged<EffectIn>,
    /// `None` when the estimate refused: there are no fits to draw.
    pub fits: Option<Judged<SyntheticIn>>,
}

pub fn estimate_synthetic_target(
    m: &PanelMatrix,
    spec: &SyntheticSpec,
    seed: u64,
    read: Read<'_>,
) -> SyntheticRead {
    let refused = |why: String| SyntheticRead {
        effect: Judged::refused(why),
        fits: None,
    };
    if read.denominator.is_some() {
        return refused("a synthetic control reads one measure: it cannot read a ratio".into());
    }
    let logged;
    let m = if read.proportional {
        match m.log1p() {
            Ok(l) => {
                logged = l;
                &logged
            }
            Err(why) => return refused(why),
        }
    } else {
        m
    };
    // A lenient panel keeps a unit's gaps; a fit needs every unit on every day it
    // reads, so it reads the days every unit holds, as pricing does.
    let dense = m.complete_days();
    let m = dense.as_ref();
    let (spec, mut dropped) = reconcile(m, spec, read.lenient);
    let r = estimate_synthetic(m, &spec, seed);
    let mut effect = r.effect.clone();
    // As `estimate_target`: keep the engine's list, the builder's reasons first.
    effect
        .dropped_units
        .retain(|d| !read.dropped.iter().any(|b| b.unit == d.unit));
    effect
        .dropped_units
        .splice(0..0, read.dropped.iter().cloned());
    for d in dropped.drain(..) {
        if !effect.dropped_units.iter().any(|k| k.unit == d.unit) {
            effect.dropped_units.push(d);
        }
    }
    let mut judged = Judged::<EffectIn>::from_engine(&effect);
    if effect.refusal.is_some() {
        return SyntheticRead {
            effect: judged,
            fits: None,
        };
    }
    let fits = json!({
        "fits": r.fits,
        "donors": r.donors,
        "placebo_sets": r.placebo_sets,
        "exact": r.exact,
    });
    if let Value::Object(raw) = &mut judged.raw {
        raw.insert("synthetic".into(), fits.clone());
    }
    SyntheticRead {
        effect: judged,
        fits: Some(Judged::from_engine(&fits)),
    }
}

/// `spec` restricted to the units `m` holds, and the units that cost.
fn reconcile(
    m: &PanelMatrix,
    spec: &SyntheticSpec,
    lenient: bool,
) -> (SyntheticSpec, Vec<DroppedUnit>) {
    let held = |u: &String| m.unit_index(u).is_some();
    let mut out = spec.clone();
    let mut dropped = Vec::new();
    if lenient {
        out.treated.retain(|u| {
            held(u) || {
                dropped.push(DroppedUnit {
                    unit: u.clone(),
                    reason: "a treated unit with no rows in the panel".into(),
                });
                false
            }
        });
    }
    if let Some(donors) = out.donors.as_mut() {
        donors.retain(|u| {
            held(u) || {
                dropped.push(DroppedUnit {
                    unit: u.clone(),
                    reason: "a donor unit with no rows in the panel".into(),
                });
                false
            }
        });
    }
    out.exclude.retain(held);
    (out, dropped)
}
