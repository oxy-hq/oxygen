//! The horizon rule, the pairing key and the deduplication, without a database.
//!
//! All three are pure — a curve is `(arm, seed, replicate, period → profit)` by
//! the time it reaches [`assemble`] — and all three are the parts that corrupt
//! the statistic silently when they are wrong.
//! `crates/app/tests/platform/simulation_routes.rs` covers the same rules end to
//! end against real rows; these assert them one at a time, including the shapes
//! that are awkward to seed.

use std::collections::BTreeMap;

use oxy_simulation::PolicyKind;
use uuid::Uuid;

use super::{
    LoadedCurves, RaceOptions, RaceRunRow, RunCurve, ScoredRun, SetAside, assemble,
    latest_run_per_draw, spec_fingerprint,
};

/// A draw on the base-0 seed ladder, where seed `k` IS replicate `k`.
///
/// That is the only base for which pairing on the seed and pairing on the
/// replicate agree, which is exactly what makes it the right default here: the
/// cases below are about the horizon, the baseline and the coverage, and they
/// state the world identity only when it is the thing under test. The cases
/// where the two keys disagree call [`curve_at`] and say so.
fn curve(arm: PolicyKind, replicate: i32, profits: &[(i32, f64)]) -> RunCurve {
    curve_at(arm, replicate, replicate as i64, profits)
}

/// A draw whose world is stated independently of its label.
///
/// The fingerprint is derived from the seed, so "same seed" still means "same
/// world" for every case that is not about the spec — which is the situation
/// the fan-out actually produces, since the arms of one race share a
/// `.simulation.yml` and differ only in the seed substituted into it. The
/// cases where a seed is shared by two DIFFERENT specs call
/// [`curve_in_world`] and say so.
fn curve_at(arm: PolicyKind, replicate: i32, seed: i64, profits: &[(i32, f64)]) -> RunCurve {
    curve_in_world(
        arm,
        replicate,
        seed,
        &format!("spec-of-seed-{seed}"),
        profits,
    )
}

/// A draw whose spec is stated independently of its seed — the two halves of a
/// world identity, pulled apart.
fn curve_in_world(
    arm: PolicyKind,
    replicate: i32,
    seed: i64,
    spec_tag: &str,
    profits: &[(i32, f64)],
) -> RunCurve {
    RunCurve {
        policy: arm,
        seed,
        spec_fingerprint: spec_tag.to_string(),
        replicate,
        by_period: profits.iter().copied().collect::<BTreeMap<_, _>>(),
    }
}

fn loaded(curves: Vec<RunCurve>) -> LoadedCurves {
    LoadedCurves {
        curves,
        superseded_runs: 0,
        in_flight_runs: 0,
    }
}

/// A `simulation_runs` row that has stopped, which is the only kind a race
/// reads.
fn row(run_id: Uuid, policy: &str, replicate: i32, seed: i64) -> RaceRunRow {
    RaceRunRow {
        run_id,
        policy: policy.to_string(),
        replicate,
        seed,
        spec_fingerprint: format!("spec-of-seed-{seed}"),
        status: "done".to_string(),
    }
}

/// A row whose spec is stated independently of its seed.
fn row_in_world(
    run_id: Uuid,
    policy: &str,
    replicate: i32,
    seed: i64,
    spec_tag: &str,
) -> RaceRunRow {
    RaceRunRow {
        spec_fingerprint: spec_tag.to_string(),
        ..row(run_id, policy, replicate, seed)
    }
}

fn in_flight(run_id: Uuid, policy: &str, replicate: i32, seed: i64, status: &str) -> RaceRunRow {
    RaceRunRow {
        status: status.to_string(),
        ..row(run_id, policy, replicate, seed)
    }
}

fn kept(run_id: Uuid, policy: PolicyKind, replicate: i32, seed: i64) -> ScoredRun {
    ScoredRun {
        run_id,
        policy,
        replicate,
        seed,
        spec_fingerprint: format!("spec-of-seed-{seed}"),
    }
}

fn race(curves: Vec<RunCurve>, options: RaceOptions) -> super::ProfitRace {
    assemble("w", loaded(curves), options).expect("assemble")
}

/// The blocker this module exists for: scoring each run at its own last row
/// would compare `machine` at period 3 against `hold` at period 2, and the
/// difference would be one period of profit rather than a policy effect.
#[test]
fn arms_are_scored_at_one_period_not_at_each_run_s_own_last_row() {
    let report = race(
        vec![
            curve(PolicyKind::Hold, 0, &[(1, 10.0), (2, 20.0)]),
            curve(PolicyKind::Machine, 0, &[(1, 11.0), (2, 22.0), (3, 90.0)]),
        ],
        RaceOptions::default(),
    );

    assert_eq!(report.horizon, Some(2));
    let c = &report.comparisons[0];
    // 22 − 20, not 90 − 20.
    assert_eq!(c.mean_difference, Some(2.0));
    assert_eq!(c.treatment.mean, Some(22.0));
    assert_eq!(c.baseline.mean, Some(20.0));
}

/// A run that recorded nothing cannot set a horizon — a `min` that counted it
/// as zero would score every arm at a period no row has, and the whole race
/// would come back empty with nothing to say why.
#[test]
fn a_run_with_no_periods_does_not_drag_the_horizon_to_zero() {
    let report = race(
        vec![
            curve(PolicyKind::Hold, 0, &[(1, 10.0), (2, 20.0)]),
            curve(PolicyKind::Hold, 1, &[]),
            curve(PolicyKind::Machine, 0, &[(1, 11.0), (2, 25.0)]),
        ],
        RaceOptions::default(),
    );

    assert_eq!(report.horizon, Some(2));
    let hold = report.arms.iter().find(|a| a.arm == "hold").expect("hold");
    assert_eq!(hold.scored, 1);
    assert_eq!(hold.short, 1, "the empty run is counted, not ignored");
    assert_eq!(
        hold.replicates
            .iter()
            .find(|r| r.replicate == 1)
            .unwrap()
            .reach,
        0
    );
}

/// Every run empty: there is no period to compare at, and the response says so
/// rather than inventing one.
#[test]
fn no_recorded_period_anywhere_leaves_the_horizon_unset() {
    let report = race(
        vec![
            curve(PolicyKind::Hold, 0, &[]),
            curve(PolicyKind::Machine, 0, &[]),
        ],
        RaceOptions::default(),
    );
    assert_eq!(report.horizon, None);
    assert_eq!(report.comparisons[0].n_pairs, 0);
    assert_eq!(report.comparisons[0].withheld.as_deref(), Some("no_pairs"));
}

/// Pinning the horizon is the escape from one dead run dragging four healthy
/// arms to period 2. The replicate that never got there is dropped, and both
/// the arm coverage and `race::compare` count it.
#[test]
fn a_pinned_horizon_drops_a_short_replicate_rather_than_truncating_it() {
    let report = race(
        vec![
            curve(PolicyKind::Hold, 0, &[(1, 10.0), (2, 20.0), (3, 30.0)]),
            curve(PolicyKind::Hold, 1, &[(1, 5.0), (2, 9.0)]),
            curve(PolicyKind::Machine, 0, &[(1, 11.0), (2, 22.0), (3, 36.0)]),
            curve(PolicyKind::Machine, 1, &[(1, 6.0), (2, 11.0), (3, 17.0)]),
        ],
        RaceOptions {
            horizon: Some(3),
            ..Default::default()
        },
    );

    assert_eq!(report.horizon, Some(3));
    assert!(report.horizon_pinned);
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 1, "hold #1 never reached period 3");
    // 36 − 30. Emphatically NOT 36 − 9, which is what scoring hold #1 at its
    // own last row would have produced.
    assert_eq!(c.mean_difference, Some(6.0));
    assert_eq!(c.dropped_unpaired, 1, "machine #1 has no hold twin at 3");
    let hold = report.arms.iter().find(|a| a.arm == "hold").expect("hold");
    assert_eq!(hold.short, 1);
}

/// The default baseline is the first arm present in `PolicyKind::ALL` order —
/// the null, else what a customer does today. Not alphabetical, and not
/// whichever arm happened to be queued first.
#[test]
fn the_default_baseline_is_the_most_null_arm_that_was_actually_run() {
    let report = race(
        vec![
            curve(PolicyKind::Oracle, 0, &[(1, 50.0)]),
            curve(PolicyKind::Legacy, 0, &[(1, 10.0)]),
            curve(PolicyKind::Machine, 0, &[(1, 20.0)]),
        ],
        RaceOptions::default(),
    );
    assert_eq!(report.baseline.as_deref(), Some("legacy"));
    assert_eq!(
        report
            .comparisons
            .iter()
            .map(|c| c.treatment.arm.as_str())
            .collect::<Vec<_>>(),
        vec!["machine", "oracle"],
        "challengers come back in PolicyKind::ALL order"
    );
    assert_eq!(report.family_size, 2);
}

/// A pinned baseline nobody ran is a 404 naming the arm — not a race against an
/// arm with no rows, which would answer `no_pairs` for every challenger and
/// read as "the data is bad" rather than "you asked for an arm that isn't
/// there".
#[test]
fn a_baseline_arm_with_no_runs_is_a_404() {
    let err = assemble(
        "w",
        loaded(vec![curve(PolicyKind::Machine, 0, &[(1, 1.0)])]),
        RaceOptions {
            baseline: Some(PolicyKind::Oracle),
            ..Default::default()
        },
    )
    .expect_err("racing against an arm with no runs");
    assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    assert!(err.1.contains("oracle"), "{}", err.1);
}

/// One arm has nothing to race against. That is an answer — the arm and its
/// coverage — not an error and not an invented rival.
#[test]
fn a_single_arm_produces_coverage_and_no_comparisons() {
    let report = race(
        vec![curve(PolicyKind::Machine, 0, &[(1, 1.0)])],
        RaceOptions::default(),
    );
    assert_eq!(report.baseline.as_deref(), Some("machine"));
    assert!(report.comparisons.is_empty());
    assert_eq!(report.family_size, 0);
    assert_eq!(report.arms.len(), 1);
}

/// No runs at all. Empty, not a 404: a declared world that nobody has run yet
/// is a world, and 404 is the answer reserved for one that does not exist.
#[test]
fn a_world_with_no_runs_assembles_an_empty_race() {
    let report = race(Vec::new(), RaceOptions::default());
    assert_eq!(report.baseline, None);
    assert_eq!(report.horizon, None);
    assert!(report.arms.is_empty());
    assert!(report.comparisons.is_empty());
}

/// A re-run of `(arm, seed)` is one world, not two. Rows arrive newest first,
/// so the first of a pair wins and the rest are counted.
#[test]
fn deduplication_keeps_the_newest_run_of_each_draw_and_counts_the_rest() {
    let newest = Uuid::new_v4();
    let older = Uuid::new_v4();
    let other_draw = Uuid::new_v4();
    let (survivors, set_aside) = latest_run_per_draw(vec![
        row(newest, "machine", 0, 7),
        row(older, "machine", 0, 7),
        row(other_draw, "machine", 1, 8),
    ]);
    assert_eq!(
        survivors,
        vec![
            kept(newest, PolicyKind::Machine, 0, 7),
            kept(other_draw, PolicyKind::Machine, 1, 8),
        ]
    );
    assert_eq!(
        set_aside,
        SetAside {
            superseded_runs: 1,
            in_flight_runs: 0,
        }
    );
}

/// The same world under two arms is two different runs of one world, which is
/// exactly what pairing needs — deduplication must not collapse them.
#[test]
fn deduplication_is_per_arm_not_per_world() {
    let hold = Uuid::new_v4();
    let machine = Uuid::new_v4();
    let (survivors, set_aside) =
        latest_run_per_draw(vec![row(hold, "hold", 0, 7), row(machine, "machine", 0, 7)]);
    assert_eq!(survivors.len(), 2);
    assert_eq!(set_aside, SetAside::default());
}

/// Two runs of one arm labelled `#0` and `#1` but drawing the SAME seed are one
/// world — a base seed moved between two queueings and the ladders overlap. The
/// replicate key would have kept both and let them fight for one slot in
/// `ArmProfits`, where the last one written would win silently.
#[test]
fn deduplication_keys_on_the_world_not_the_replicate_label() {
    let newest = Uuid::new_v4();
    let older = Uuid::new_v4();
    let (survivors, set_aside) = latest_run_per_draw(vec![
        in_flight(newest, "machine", 1, 8, "done"),
        row(older, "machine", 0, 8),
    ]);
    assert_eq!(survivors, vec![kept(newest, PolicyKind::Machine, 1, 8)]);
    assert_eq!(set_aside.superseded_runs, 1);
}

/// The blocker: a re-queued run is the newest row for its world and has no
/// period rows at all. Admitting it would evict the completed run it repeats,
/// whose curve then reads as empty — so re-running a world blanked the finished
/// race that was already there.
#[test]
fn an_in_flight_run_never_displaces_the_completed_run_of_its_world() {
    let requeued = Uuid::new_v4();
    let finished = Uuid::new_v4();
    let (survivors, set_aside) = latest_run_per_draw(vec![
        in_flight(requeued, "machine", 0, 7, "queued"),
        row(finished, "machine", 0, 7),
    ]);
    assert_eq!(survivors, vec![kept(finished, PolicyKind::Machine, 0, 7)]);
    assert_eq!(
        set_aside,
        SetAside {
            superseded_runs: 0,
            in_flight_runs: 1,
        },
        "a run that has not stopped has superseded nothing"
    );
}

/// `running` too, and its partial curve with it: the run is still writing
/// periods, so scoring it would move the horizon between two identical
/// requests.
#[test]
fn a_running_run_is_set_aside_the_same_way_a_queued_one_is() {
    let running = Uuid::new_v4();
    let finished = Uuid::new_v4();
    let (survivors, set_aside) = latest_run_per_draw(vec![
        in_flight(running, "hold", 0, 7, "running"),
        row(finished, "hold", 0, 7),
    ]);
    assert_eq!(survivors, vec![kept(finished, PolicyKind::Hold, 0, 7)]);
    assert_eq!(set_aside.in_flight_runs, 1);
}

/// `failed` and `cancelled` are terminal. Whatever periods such a run recorded
/// are real and frozen, and the horizon rule was written for that raggedness —
/// dropping them would discard evidence, not protect the statistic.
#[test]
fn failed_and_cancelled_runs_are_evidence_and_are_kept() {
    let failed = Uuid::new_v4();
    let cancelled = Uuid::new_v4();
    let (survivors, set_aside) = latest_run_per_draw(vec![
        in_flight(failed, "hold", 0, 7, "failed"),
        in_flight(cancelled, "machine", 0, 7, "cancelled"),
    ]);
    assert_eq!(survivors.len(), 2);
    assert_eq!(set_aside, SetAside::default());
}

/// A status the enum no longer knows is treated as in-flight, not as terminal:
/// the conservative read, since the alternative is scoring a curve that may
/// still be growing.
#[test]
fn an_unrecognised_status_is_treated_as_in_flight() {
    let (survivors, set_aside) =
        latest_run_per_draw(vec![in_flight(Uuid::new_v4(), "hold", 0, 7, "paused")]);
    assert!(survivors.is_empty());
    assert_eq!(set_aside.in_flight_runs, 1);
}

/// An arm the enum no longer knows is skipped, not fatal: the other arms are
/// still a race, and refusing to answer would help nobody.
#[test]
fn an_unparseable_arm_is_skipped_rather_than_failing_the_race() {
    let good = Uuid::new_v4();
    let (survivors, set_aside) = latest_run_per_draw(vec![
        row(Uuid::new_v4(), "quantum", 0, 7),
        row(good, "hold", 0, 7),
    ]);
    assert_eq!(survivors, vec![kept(good, PolicyKind::Hold, 0, 7)]);
    assert_eq!(
        set_aside,
        SetAside::default(),
        "a skipped arm is neither superseded nor in flight"
    );
}

// ── the world's identity ─────────────────────────────────────────────────────

/// `simulation_runs.spec` is `jsonb`, and Postgres does not preserve object key
/// order. Hashing the bytes as they come back would make one world two on a
/// round trip — every arm in its own singleton world, every comparison
/// `disjoint_worlds`, and a race that answers nothing at all.
#[test]
fn a_fingerprint_survives_the_key_reordering_jsonb_is_free_to_do() {
    let one = serde_json::json!({
        "name": "w", "seed": 7,
        "entities": {"count": 100, "scale_sigma": 0.4},
        "periods": [1, 2, 3],
    });
    let reordered = serde_json::json!({
        "periods": [1, 2, 3],
        "entities": {"scale_sigma": 0.4, "count": 100},
        "seed": 7, "name": "w",
    });
    assert_eq!(
        spec_fingerprint(&one),
        spec_fingerprint(&reordered),
        "the same world however its keys came back"
    );
}

/// Array order is data, not formatting — two specs differing only in the order
/// of a list are two worlds.
#[test]
fn a_fingerprint_does_not_reorder_arrays() {
    assert_ne!(
        spec_fingerprint(&serde_json::json!({"periods": [1, 2]})),
        spec_fingerprint(&serde_json::json!({"periods": [2, 1]})),
    );
}

/// The field the reviewer's case turns on: an edit that leaves `seed:` alone.
#[test]
fn a_fingerprint_separates_specs_that_differ_anywhere_but_the_seed() {
    let hundred = serde_json::json!({"seed": 7, "entities": {"count": 100}});
    let five_hundred = serde_json::json!({"seed": 7, "entities": {"count": 500}});
    assert_ne!(
        spec_fingerprint(&hundred),
        spec_fingerprint(&five_hundred),
        "same seed, different world"
    );
}

// ── the pairing key ──────────────────────────────────────────────────────────

/// Replicate `k` of two arms is one world only while the spec's `seed:` has not
/// moved. Pairing them regardless takes a difference over two different worlds,
/// which is world variance sold as a policy effect.
#[test]
fn draws_on_different_seeds_do_not_pair_even_under_one_replicate_number() {
    let report = race(
        vec![
            curve_at(PolicyKind::Hold, 0, 7, &[(1, 10.0), (2, 20.0)]),
            curve_at(PolicyKind::Machine, 0, 99, &[(1, 11.0), (2, 26.0)]),
        ],
        RaceOptions::default(),
    );
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 0);
    assert_eq!(c.mean_difference, None);
    assert_eq!(c.withheld.as_deref(), Some("disjoint_worlds"));
}

/// And the converse, which is why the seed key is *more* correct rather than
/// merely safer: a base seed moved 7 → 8 makes `hold` #1 and `machine` #0 the
/// same world, and they pair despite the differing labels.
#[test]
fn draws_sharing_a_seed_pair_even_under_different_replicate_numbers() {
    let report = race(
        vec![
            curve_at(PolicyKind::Hold, 0, 7, &[(1, 10.0), (2, 20.0)]),
            curve_at(PolicyKind::Hold, 1, 8, &[(1, 12.0), (2, 24.0)]),
            curve_at(PolicyKind::Machine, 0, 8, &[(1, 13.0), (2, 30.0)]),
            curve_at(PolicyKind::Machine, 1, 9, &[(1, 5.0), (2, 9.0)]),
        ],
        RaceOptions::default(),
    );
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 1, "seed 8 is the shared world");
    assert_eq!(
        c.mean_difference,
        Some(6.0),
        "30 − 24, not the label pairing"
    );
    assert_eq!(c.dropped_unpaired, 2);
}

/// The seed is only HALF the world. Two arms queued off the same seed but a
/// DIFFERENT spec are two different worlds, and pairing them is the same
/// fabrication as pairing two different seeds — worse, because it comes back
/// confident.
///
/// This is the shape the fan-out produces the moment a `.simulation.yml` is
/// edited between queueing arm A and arm B: `entities.count` 100 → 500 moves
/// every entity's scale and therefore every profit, `seed:` is untouched, so
/// the seeds match, the pairing looks complete (`dropped_unpaired: 0`) and the
/// margin reported is the edit, not the policy. `noise_ratio`, `lag_days`,
/// `period_days` and `scale_sigma` are the same story.
#[test]
fn draws_sharing_a_seed_under_different_specs_do_not_pair() {
    let report = race(
        vec![
            curve_in_world(PolicyKind::Hold, 0, 7, "entities-100", &[(1, 10.0)]),
            curve_in_world(PolicyKind::Machine, 0, 7, "entities-500", &[(1, 50.0)]),
        ],
        RaceOptions::default(),
    );
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 0, "one seed, two specs, no shared world");
    assert_eq!(c.mean_difference, None);
    assert_eq!(
        c.withheld.as_deref(),
        Some("disjoint_worlds"),
        "withheld and named, never a confident 5x margin"
    );
    assert_eq!(
        c.dropped_unpaired, 2,
        "and both draws are counted as dropped"
    );
}

/// The converse, so the key is not merely stricter: the same spec at the same
/// seed is one world however the two rows were labelled.
#[test]
fn draws_sharing_a_seed_and_a_spec_still_pair() {
    let report = race(
        vec![
            curve_in_world(PolicyKind::Hold, 0, 7, "entities-100", &[(1, 10.0)]),
            curve_in_world(PolicyKind::Machine, 3, 7, "entities-100", &[(1, 16.0)]),
        ],
        RaceOptions::default(),
    );
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 1);
    assert_eq!(c.mean_difference, Some(6.0));
    assert_eq!(c.dropped_unpaired, 0);
}

/// Deduplication is keyed on the whole world too. Two terminal runs of one arm
/// at one seed but different specs are two worlds, not a re-run — so neither
/// evicts the other, and `superseded_runs` stays 0.
#[test]
fn deduplication_does_not_treat_a_respec_as_a_re_run() {
    let (older, newer) = (Uuid::new_v4(), Uuid::new_v4());
    let (kept_runs, set_aside) = latest_run_per_draw(vec![
        row_in_world(newer, "machine", 0, 7, "entities-500"),
        row_in_world(older, "machine", 0, 7, "entities-100"),
    ]);
    assert_eq!(kept_runs.len(), 2, "two specs at one seed are two worlds");
    assert_eq!(set_aside.superseded_runs, 0);
}

/// And a genuine re-run — same arm, same seed, same spec — still supersedes.
#[test]
fn deduplication_still_supersedes_a_re_run_of_one_world() {
    let (older, newer) = (Uuid::new_v4(), Uuid::new_v4());
    let (kept_runs, set_aside) = latest_run_per_draw(vec![
        row_in_world(newer, "machine", 0, 7, "entities-100"),
        row_in_world(older, "machine", 0, 7, "entities-100"),
    ]);
    assert_eq!(kept_runs.len(), 1);
    assert_eq!(kept_runs[0].run_id, newer);
    assert_eq!(set_aside.superseded_runs, 1);
}

/// `disjoint_worlds` is reserved for two arms that each scored something. An
/// arm with nothing scored is still `no_pairs` — that is "come back when the
/// runs finish", a different message entirely.
#[test]
fn an_arm_that_scored_nothing_is_no_pairs_not_disjoint_worlds() {
    let report = race(
        vec![
            curve_at(PolicyKind::Hold, 0, 7, &[(1, 10.0)]),
            curve_at(PolicyKind::Machine, 0, 7, &[]),
        ],
        RaceOptions::default(),
    );
    assert_eq!(
        report.comparisons[0].withheld.as_deref(),
        Some("no_pairs"),
        "machine scored no world at all — nothing was disjoint, it is simply absent"
    );
}

/// The world keys are assigned once across every arm. Numbering each arm's
/// worlds `0..n` on its own would pair the treatment's first world against the
/// baseline's first — the positional zip `ArmProfits` is keyed to prevent.
#[test]
fn world_keys_are_assigned_across_arms_not_within_one() {
    let report = race(
        vec![
            curve_at(PolicyKind::Hold, 0, 7, &[(1, 10.0)]),
            curve_at(PolicyKind::Hold, 1, 8, &[(1, 20.0)]),
            curve_at(PolicyKind::Machine, 0, 8, &[(1, 25.0)]),
        ],
        RaceOptions::default(),
    );
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 1);
    // 25 − 20, on seed 8. A per-arm numbering would have made machine's only
    // world index 0 and paired it against hold's seed 7 for 25 − 10 = 15.
    assert_eq!(c.mean_difference, Some(5.0));
}

/// The seed rides on the wire beside the replicate, because once a base seed
/// has moved the label no longer identifies a draw — and the coverage list is
/// ordered by the key the race actually pairs on.
#[test]
fn coverage_carries_the_seed_and_is_ordered_by_it() {
    let report = race(
        vec![
            curve_at(PolicyKind::Machine, 5, 8, &[(1, 20.0)]),
            curve_at(PolicyKind::Machine, 0, 7, &[(1, 10.0)]),
        ],
        RaceOptions::default(),
    );
    let machine = report.arms.iter().find(|a| a.arm == "machine").unwrap();
    assert_eq!(
        machine
            .replicates
            .iter()
            .map(|r| (r.replicate, r.seed))
            .collect::<Vec<_>>(),
        vec![(0, 7), (5, 8)]
    );
}

/// The seed goes out as the `u64` it was declared as, not as the `i64` Postgres
/// stores it in. Every other surface that puts a seed on the wire —
/// `EnqueuedRun.seed` and `simulation_runs::Model` — already undoes the cast,
/// and this field exists precisely so a reader can match a coverage row against
/// the run listing. Spelt two different ways, it fails at that for exactly the
/// seeds where the spelling differs.
#[test]
fn the_coverage_seed_goes_out_as_the_u64_it_was_declared_as() {
    // `u64::MAX` bit-cast into the column, which is what a world declaring
    // `seed: 18446744073709551615` stores.
    let report = race(
        vec![curve_at(PolicyKind::Machine, 0, -1, &[(1, 1.0)])],
        RaceOptions::default(),
    );
    let json = serde_json::to_value(&report).expect("serialise the race");
    assert_eq!(
        json["arms"][0]["replicates"][0]["seed"],
        serde_json::json!(u64::MAX),
        "a negative seed on the wire is the cast leaking, not a seed"
    );
}

/// `?horizon=0` is a caller who meant "the start". Periods are 1-based, so it
/// would silently produce an empty race; it is a 400 instead.
#[test]
fn a_horizon_below_the_first_period_is_a_400() {
    let err = super::RaceQuery {
        baseline: None,
        horizon: Some(0),
    }
    .parse()
    .expect_err("horizon 0");
    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
}

/// A misspelled arm names the five rather than falling back to a default —
/// same contract as `parse_policies` on the queueing side.
#[test]
fn an_unknown_baseline_arm_is_a_400_naming_the_arms() {
    let err = super::RaceQuery {
        baseline: Some("holdd".into()),
        horizon: None,
    }
    .parse()
    .expect_err("unknown arm");
    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(err.1.contains("hold"), "{}", err.1);
}

/// An empty `?baseline=` is an unset one, not an arm named "".
#[test]
fn a_blank_baseline_parameter_is_the_default_baseline() {
    let options = super::RaceQuery {
        baseline: Some("  ".into()),
        horizon: None,
    }
    .parse()
    .expect("blank baseline");
    assert_eq!(options.baseline, None);
}
