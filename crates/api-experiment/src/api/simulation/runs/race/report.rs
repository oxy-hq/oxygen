//! The shape a race answers with, and the assembly that produces it.
//!
//! Pure: every database concern is resolved by the time a [`RunCurve`] reaches
//! here, so the horizon rule — the thing most likely to be got wrong, and the
//! thing that quietly corrupts the statistic when it is — is assertable
//! without a database. The reasoning behind that rule is in the parent
//! module's docs.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use serde::{Serialize, Serializer};

use oxy_simulation::PolicyKind;
use oxy_simulation::race::{ArmProfits, ArmSummary, Inference, NoInference, PairedComparison};

use super::super::super::ApiError;
use super::{LoadedCurves, RaceOptions, RunCurve};

/// What `GET /simulations/{name}/race` answers with.
#[derive(Debug, Serialize)]
pub struct ProfitRace {
    pub simulation: String,
    /// The arm every challenger was compared against. `None` only when the
    /// world has no runs at all.
    pub baseline: Option<String>,
    /// The period index every arm was scored at. **Read this before reading a
    /// margin** — a race at period 2 of a 40-period world is a different claim
    /// from one at period 40. `None` when no run recorded a single period.
    pub horizon: Option<i32>,
    /// True when the caller chose the horizon, false when it is the deepest
    /// period every recorded replicate reached.
    pub horizon_pinned: bool,
    /// Every arm with a run, and what each of its draws contributed.
    pub arms: Vec<ArmCoverage>,
    /// One per challenger, in [`PolicyKind::ALL`] order.
    pub comparisons: Vec<RaceComparison>,
    /// Older runs of an `(arm, seed)` that a newer terminal run replaced.
    pub superseded_runs: usize,
    /// Runs of this world still `queued` or `running`, and therefore not
    /// scored. Visible for the same reason `superseded_runs` is: a race read
    /// while three of its arms are mid-flight is a different claim from one
    /// read when they finished, and the response is the only thing that can
    /// say so.
    pub in_flight_runs: usize,
    /// How many comparisons this response ran. Every `p_value` below is
    /// **per-comparison and uncorrected**: at α = 0.05 a family of four has a
    /// family-wise error near 18%, so a surface that ranks arms should say
    /// per-comparison in its copy or correct for this number.
    pub family_size: usize,
}

/// One arm, and which of its draws the horizon admitted.
#[derive(Debug, Serialize)]
pub struct ArmCoverage {
    pub arm: String,
    /// Every world this arm has a scorable run of, one entry per **world** —
    /// ordered by seed, then by spec, then by replicate. Ordered by the pairing
    /// key rather than the label because the replicate number is no longer
    /// unique within an arm once a base seed has moved, and the seed is no
    /// longer unique once a spec has been edited under it — so two arms' lists
    /// are only comparable row-for-row under the key they actually pair on.
    pub replicates: Vec<ReplicateReach>,
    /// Draws with a `cumulative_profit` row at the horizon.
    pub scored: usize,
    /// Draws without one — dropped from the pairing, counted here.
    pub short: usize,
}

/// One draw of the world under one arm.
#[derive(Debug, Serialize)]
pub struct ReplicateReach {
    /// The label the run was queued with — what a reader recognises, and what
    /// `GET /simulations/runs` shows. **Not** the pairing key, and not unique
    /// within an arm once a base seed has moved.
    pub replicate: i32,
    /// The world this draw is. Two rows across arms are one world exactly when
    /// this matches, which is why it is here and not left for the reader to
    /// recompute from a base seed they cannot see.
    ///
    /// Stored `i64` because Postgres has no unsigned type, and put on the wire
    /// as the `u64` it was declared as — the same undoing of the cast that
    /// [`simulation_runs::Model`](entity::simulation_runs::Model) and
    /// `EnqueuedRun` already do. The whole point of this field is that a reader
    /// can match a coverage row against the run listing; spelt two different
    /// ways it fails at exactly the seeds where the spelling differs.
    #[serde(serialize_with = "seed_as_u64")]
    pub seed: i64,
    /// The rest of the world's identity: a short digest of the spec snapshot
    /// this run stored. Two rows across arms are one world exactly when the
    /// seed AND this match.
    ///
    /// On the wire because the seed alone stopped being sufficient the moment a
    /// spec could be edited under it, and a reader looking at two arms that did
    /// not pair on an apparently shared seed has otherwise nothing to look at.
    /// An opaque token, not an identifier: it is stable within one response and
    /// nothing should store it or look it up. See `race::spec_fingerprint`.
    pub world: String,
    /// The deepest period this run recorded. `0` for a run that recorded none.
    pub reach: i32,
    /// Whether this draw contributed a profit at the horizon.
    pub scored: bool,
}

/// The bit-cast `simulation_runs::Column::Seed` stores, undone. Duplicated
/// rather than shared with the entity because the entity's copy is a private
/// serde helper, and lifting it into a public API to save four lines would make
/// a storage detail part of that crate's surface.
fn seed_as_u64<S: Serializer>(seed: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(*seed as u64)
}

/// One challenger against the baseline, on the worlds they both scored.
#[derive(Debug, Serialize)]
pub struct RaceComparison {
    pub treatment: ArmScore,
    pub baseline: ArmScore,
    /// Worlds both arms scored at the horizon. The sample size of the test.
    pub n_pairs: usize,
    /// Worlds one arm scored and the other did not — counted by seed, like
    /// everything else a race pairs on.
    pub dropped_unpaired: usize,
    /// Pairs discarded because a profit, or their difference, was not finite.
    /// Always an upstream bug; counted so it is visible instead of poisoning
    /// the mean.
    pub dropped_nonfinite: usize,
    /// `mean(treatment − baseline)`. Positive means the treatment earned more.
    pub mean_difference: Option<f64>,
    /// `None` whenever `withheld` is set.
    pub test: Option<PairedTestResult>,
    /// Why there is no test: `disjoint_worlds`, `no_pairs`, `single_pair`,
    /// `identical_arms` or `constant_difference`. `None` when there is one.
    pub withheld: Option<String>,
}

/// One arm over the paired subset only — **not** over everything it ran. An
/// arm's mean across five worlds and another's across three are not comparable
/// numbers, which is the whole reason a race pairs.
#[derive(Debug, Serialize)]
pub struct ArmScore {
    pub arm: String,
    pub n: usize,
    pub mean: Option<f64>,
    /// Bessel-corrected. `None` when `n < 2`.
    pub sd: Option<f64>,
}

/// The paired t-test, when there was one to run.
#[derive(Debug, Serialize)]
pub struct PairedTestResult {
    pub std_error: f64,
    pub t: f64,
    /// `n_pairs − 1`, where `n_pairs` counts worlds, not runs.
    pub dof: usize,
    /// Two-sided, against H₀ of a zero mean difference. Per-comparison — see
    /// [`ProfitRace::family_size`].
    pub p_value: f64,
    pub confidence: f64,
    /// On the **mean difference**, not on either arm.
    pub interval_low: f64,
    pub interval_high: f64,
}

// ── assembly ─────────────────────────────────────────────────────────────────

/// Turn one world's curves into a race. Pure — every database concern is
/// already resolved, so the horizon rule is assertable without one.
pub fn assemble(
    simulation: &str,
    loaded: LoadedCurves,
    options: RaceOptions,
) -> Result<ProfitRace, ApiError> {
    let LoadedCurves {
        curves,
        superseded_runs,
        in_flight_runs,
    } = loaded;

    let horizon = options.horizon.or_else(|| common_horizon(&curves));
    let worlds = world_keys(&curves);
    let by_arm = group_by_arm(curves);
    let ran = |arm: PolicyKind| by_arm.iter().any(|(a, _)| *a == arm);

    let baseline = match options.baseline {
        Some(pinned) => {
            if !ran(pinned) {
                return Err((
                    StatusCode::NOT_FOUND,
                    format!(
                        "'{simulation}' has no runs of the '{}' arm to race against",
                        pinned.as_str()
                    ),
                ));
            }
            Some(pinned)
        }
        // The first arm present in `ALL` order — the null, else what a customer
        // does today, and so on down to the ceiling.
        None => PolicyKind::ALL.into_iter().find(|arm| ran(*arm)),
    };

    let arms: Vec<ArmCoverage> = by_arm
        .iter()
        .map(|(arm, runs)| coverage(*arm, runs, horizon))
        .collect();

    let profits: Vec<(PolicyKind, ArmProfits)> = by_arm
        .iter()
        .map(|(arm, runs)| (*arm, score_at(*arm, runs, horizon, &worlds)))
        .collect();

    let comparisons = match baseline
        .and_then(|arm| profits.iter().find(|(a, _)| *a == arm))
        .map(|(_, base)| base)
    {
        Some(base) => profits
            .iter()
            .filter(|(arm, _)| Some(*arm) != baseline)
            .map(|(_, challenger)| {
                comparison(
                    oxy_simulation::race::compare(challenger, base),
                    // The pre-pairing counts, which the comparison itself no
                    // longer carries: its two `ArmScore`s summarise the paired
                    // subset, so both read `n: 0` exactly when the interesting
                    // question — did these arms run *anything*, or just nothing
                    // in common — has become unanswerable from the result.
                    BothScored {
                        treatment: challenger.len(),
                        baseline: base.len(),
                    },
                )
            })
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };

    Ok(ProfitRace {
        simulation: simulation.to_string(),
        baseline: baseline.map(|arm| arm.as_str().to_string()),
        horizon,
        horizon_pinned: options.horizon.is_some(),
        arms,
        family_size: comparisons.len(),
        comparisons,
        superseded_runs,
        in_flight_runs,
    })
}

/// Every world any arm drew, mapped to the dense key `ArmProfits` pairs on.
///
/// `ArmProfits::observe` takes a `u32` and a seed is a `u64` bit-cast into an
/// `i64`, so the seed cannot be the key directly. It does not need to be: the
/// key is only ever compared for equality inside `pair_up`, so any injective
/// map preserves the pairing exactly. Built once across **all** arms — building
/// it per arm would number each arm's worlds `0..n` independently and pair the
/// treatment's first world against the baseline's first, which is precisely the
/// positional-zip error `ArmProfits` is keyed to prevent.
///
/// `BTreeMap` so the numbering is by ascending key and therefore stable across
/// requests; a race whose keys moved between two identical reads would be fine
/// statistically and impossible to debug.
///
/// The map's own key is `(seed, spec fingerprint)`, in that order: the
/// fingerprint alone is the world's identity — the snapshot is seed-substituted,
/// so it already subsumes the seed — but ordering on a digest would number the
/// worlds by hash, and the numbering is what a reader follows down the coverage
/// list. Leading with the seed keeps the reading order and costs nothing, since
/// the pair is compared only for equality.
fn world_keys(curves: &[RunCurve]) -> BTreeMap<(i64, String), u32> {
    let mut worlds: Vec<(i64, String)> = curves
        .iter()
        .map(|c| (c.seed, c.spec_fingerprint.clone()))
        .collect();
    worlds.sort_unstable();
    worlds.dedup();
    worlds
        .into_iter()
        .enumerate()
        .map(|(index, world)| (world, index as u32))
        .collect()
}

/// The deepest period EVERY recorded replicate reached.
///
/// `min` over each run's last period, and runs that recorded nothing are
/// excluded rather than counted as zero — a run with no rows would otherwise
/// force a horizon of 0, which no row has, and every arm would score empty.
/// Those runs are still counted, as `short` in their arm's coverage.
fn common_horizon(curves: &[RunCurve]) -> Option<i32> {
    curves
        .iter()
        .filter_map(|c| c.by_period.keys().next_back().copied())
        .min()
}

/// Grouped in [`PolicyKind::ALL`] order, and by seed within an arm, so the
/// response is stable across requests and every list in it reads in the order a
/// race does: the null, what a customer does today, the product, the candidate
/// fix, the ceiling.
///
/// By world rather than by replicate because the replicate is no longer unique
/// within an arm — a base seed moved between two queueings leaves two draws
/// labelled `#0` — so sorting on it alone would not be a total order. Nor is
/// the seed one: a spec edited under a fixed seed leaves two worlds at the same
/// seed. The spec fingerprint separates those, and the replicate breaks the
/// (impossible, but free to state) remaining tie.
///
/// A `Vec` rather than a map because `PolicyKind` is deliberately neither `Ord`
/// nor `Hash` — and adding either to satisfy this file would put the arms in
/// whatever order the derive picked, which is not a decision this module gets
/// to make. Five arms make the linear scan free.
fn group_by_arm(curves: Vec<RunCurve>) -> Vec<(PolicyKind, Vec<RunCurve>)> {
    let mut by_arm = Vec::new();
    for arm in PolicyKind::ALL {
        let mut runs: Vec<RunCurve> = curves.iter().filter(|c| c.policy == arm).cloned().collect();
        if runs.is_empty() {
            continue;
        }
        runs.sort_by_key(|c| (c.seed, c.spec_fingerprint.clone(), c.replicate));
        by_arm.push((arm, runs));
    }
    by_arm
}

fn coverage(arm: PolicyKind, runs: &[RunCurve], horizon: Option<i32>) -> ArmCoverage {
    let replicates: Vec<ReplicateReach> = runs
        .iter()
        .map(|run| ReplicateReach {
            replicate: run.replicate,
            seed: run.seed,
            world: run.spec_fingerprint.clone(),
            reach: run.by_period.keys().next_back().copied().unwrap_or(0),
            scored: horizon.is_some_and(|h| run.by_period.contains_key(&h)),
        })
        .collect();
    let scored = replicates.iter().filter(|r| r.scored).count();
    ArmCoverage {
        arm: arm.as_str().to_string(),
        short: replicates.len() - scored,
        scored,
        replicates,
    }
}

/// One arm's profits at the horizon, keyed by world.
///
/// A draw with no row at the horizon is simply absent, which is what makes it a
/// drop rather than a truncation: `race::compare` will not pair it, and
/// [`coverage`] has already counted it.
fn score_at(
    arm: PolicyKind,
    runs: &[RunCurve],
    horizon: Option<i32>,
    worlds: &BTreeMap<(i64, String), u32>,
) -> ArmProfits {
    let mut profits = ArmProfits::new(arm);
    let Some(horizon) = horizon else {
        return profits;
    };
    for run in runs {
        let Some(profit) = run.by_period.get(&horizon) else {
            continue;
        };
        // Infallible by construction — `worlds` was built from these same
        // curves. Handled rather than unwrapped because the alternative to a
        // warn here is a panic in a read handler over a bookkeeping slip.
        let Some(world) = worlds.get(&(run.seed, run.spec_fingerprint.clone())) else {
            tracing::warn!(
                arm = arm.as_str(),
                seed = run.seed,
                spec = %run.spec_fingerprint,
                "simulation race: a curve's world is missing from the pairing key map"
            );
            continue;
        };
        profits.observe(*world, *profit);
    }
    profits
}

/// How many worlds each arm scored before pairing — what separates "these arms
/// ran different worlds" from "one of them has nothing".
#[derive(Debug, Clone, Copy)]
struct BothScored {
    treatment: usize,
    baseline: usize,
}

fn comparison(paired: PairedComparison, scored: BothScored) -> RaceComparison {
    let (test, withheld) = match paired.inference {
        Inference::Tested(t) => (
            Some(PairedTestResult {
                std_error: t.std_error,
                t: t.t,
                dof: t.dof,
                p_value: t.p_value,
                confidence: t.confidence,
                interval_low: t.interval.0,
                interval_high: t.interval.1,
            }),
            None,
        ),
        Inference::Withheld(why) => (None, Some(withheld_reason(why, scored).to_string())),
    };
    RaceComparison {
        treatment: arm_score(paired.treatment),
        baseline: arm_score(paired.baseline),
        n_pairs: paired.n_pairs,
        dropped_unpaired: paired.dropped_unpaired,
        dropped_nonfinite: paired.dropped_nonfinite,
        mean_difference: paired.mean_difference,
        test,
        withheld,
    }
}

fn arm_score(summary: ArmSummary) -> ArmScore {
    ArmScore {
        arm: summary.arm.as_str().to_string(),
        n: summary.n,
        mean: summary.mean,
        sd: summary.sd,
    }
}

/// The wire spelling of a withheld inference. `snake_case` to match every other
/// enum this API puts on the wire, and exhaustive so a new `NoInference`
/// variant fails the build here rather than reaching a panel as a shrug.
///
/// `NoPairs` splits in two here, because the one variant covers two situations
/// a reader must not confuse. An arm with nothing scored is `no_pairs` — come
/// back when the runs finish. Two arms that each scored worlds and share none
/// is `disjoint_worlds`: nothing is pending, nothing will arrive, and these
/// runs will *never* be comparable because they were run against different
/// worlds. Left as one word, that case renders as an empty margin and reads as
/// "no difference", which is the opposite of what it means.
fn withheld_reason(why: NoInference, scored: BothScored) -> &'static str {
    match why {
        NoInference::NoPairs if scored.treatment > 0 && scored.baseline > 0 => "disjoint_worlds",
        NoInference::NoPairs => "no_pairs",
        NoInference::SinglePair => "single_pair",
        NoInference::IdenticalArms => "identical_arms",
        NoInference::ConstantDifference => "constant_difference",
    }
}
