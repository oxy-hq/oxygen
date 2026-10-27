//! Pricing the waves core against a synthetic panel: 12 units × 300 days of
//! seasonal noise. The engine is real; only the iteration counts are turned down.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use oxy_airlayer_compat::engine::experiment::{
    PanelMatrix, ProposedWave, day_ordinal, propose_strata, propose_waves,
};
use oxy_experiment::parse;

use super::candidate::{Candidate, WavesCandidate};
use super::panel::GroupMembership;
use super::price::*;
use super::snippet::plan_waves;
use super::testing::{TWO_WAVES, draft, registered, switchback_yaml, yaml};

const UNITS: usize = 12;
const DAYS: i64 = 300;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// Deterministic pseudo-noise, so a failure is reproducible.
fn noise(unit: usize, day: i64) -> f64 {
    let mut x = (unit as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (day as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 31;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    ((x >> 11) as f64 / (1u64 << 53) as f64) - 0.5
}

pub(super) fn panel() -> PanelMatrix {
    let start = day_ordinal(date(2025, 6, 1));
    PanelMatrix::from_triples((0..UNITS).flat_map(move |u| {
        (0..DAYS).map(move |d| {
            let season = 10.0 * ((d as f64) * 2.0 * std::f64::consts::PI / 7.0).sin();
            (
                format!("u{u:02}"),
                start + d,
                100.0 + 5.0 * u as f64 + season + 8.0 * noise(u, d),
            )
        })
    }))
}

pub(super) fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap()
}

fn waves(sizes: &[usize]) -> WavesCandidate {
    WavesCandidate {
        wave_sizes: sizes.to_vec(),
        first_switch_on: date(2026, 3, 9).into(),
        spacing_days: 14,
        blocks: None,
        pairs: false,
        assign_by: None,
        alpha: None,
    }
}

pub(super) fn run() -> RunParams {
    RunParams {
        seed: 8841207,
        now: now(),
        iterations: Iterations {
            common_date: 60,
            staggered: 20,
            switchback: 40,
        },
        membership: None,
    }
}

fn priced(c: PriceComputation) -> Priced {
    match c {
        PriceComputation::Priced(p) => p,
        PriceComputation::Refused { reason, problems } => panic!("refused: {reason} {problems:?}"),
    }
}

fn wave_plan(p: &Priced) -> &[super::snippet::WavePlan] {
    match &p.plan {
        SchedulePlan::Waves(w) => w,
        other => panic!("expected waves, got {other:?}"),
    }
}

/// Groups `u00..u11`, five members each: what a membership of 60 SKUs in 12 categories looks like.
fn membership() -> GroupMembership {
    GroupMembership(
        (0..UNITS)
            .map(|g| {
                (
                    format!("u{g:02}"),
                    (0..5).map(|k| format!("u{g:02}-m{k}")).collect(),
                )
            })
            .collect(),
    )
}

#[test]
fn every_unit_placed_in_one_wave_is_refused_as_unidentified() {
    let c = price_waves(&draft(7), &panel(), &waves(&[UNITS]), &run());
    let PriceComputation::Refused { reason, .. } = c else {
        panic!("must refuse")
    };
    assert!(reason.contains("all 12 units of `store`"), "{reason}");
    assert!(reason.contains("none is left as a control"), "{reason}");
}

#[test]
fn every_unit_placed_across_several_waves_prices_as_a_fully_staggered_rollout() {
    // No permanent holdout: the later wave controls the earlier, the last is dropped.
    let mut c = waves(&[6, 6]);
    c.spacing_days = 40; // past the first wave's washout + post window (7 + 28), so the second is a clean control
    let p = priced(price_waves(&draft(7), &panel(), &c, &run()));
    assert_eq!(
        wave_plan(&p).iter().map(|w| w.units.len()).sum::<usize>(),
        UNITS
    );
    assert!(p.mde_unit_day.is_finite(), "{}", p.mde_unit_day);
    assert!(
        p.warnings.iter().any(|w| w.contains("last wave")),
        "says the last wave is not estimated: {:?}",
        p.warnings
    );
}

#[test]
fn a_holdout_leaves_a_common_date_design_that_prices() {
    let p = priced(price_waves(&draft(7), &panel(), &waves(&[6]), &run()));
    assert_eq!(p.seed, 8841207);
    assert_eq!(wave_plan(&p).len(), 1);
    assert_eq!(wave_plan(&p)[0].units.len(), 6);
    assert_eq!(wave_plan(&p)[0].switch_on, date(2026, 3, 9));
    assert!(
        p.power.mde > 0.0 && p.power.mde.is_finite(),
        "mde {}",
        p.power.mde
    );
    assert!(p.power.refusal.is_none());
    // A unit design: no groups, no blocks, the file's α, and the MDE already per unit-day.
    assert_eq!(
        (p.assign_by.clone(), p.strata.clone(), p.alpha),
        (None, None, 0.1)
    );
    assert_eq!(p.mde_unit_day, p.power.mde);
}

#[test]
fn a_staggered_request_prices_with_the_staggered_shape() {
    let p = priced(price_waves(&draft(7), &panel(), &waves(&[3, 3]), &run()));
    assert_eq!(
        wave_plan(&p)
            .iter()
            .map(|w| w.units.len())
            .collect::<Vec<_>>(),
        vec![3, 3]
    );
    assert_eq!(wave_plan(&p)[0].switch_on, date(2026, 3, 9));
    assert_eq!(
        wave_plan(&p)[1].switch_on,
        date(2026, 3, 23),
        "spacing 14 days"
    );
    assert!(p.power.mde > 0.0);
}

#[test]
fn the_same_seed_reproduces_the_waves_and_the_mde() {
    let a = priced(price_waves(&draft(7), &panel(), &waves(&[4]), &run()));
    let b = priced(price_waves(&draft(7), &panel(), &waves(&[4]), &run()));
    assert_eq!(wave_plan(&a), wave_plan(&b));
    assert_eq!(a.power.mde, b.power.mde);
}

#[test]
fn the_waves_are_what_the_engine_proposes_for_that_seed() {
    let p = priced(price_waves(&draft(7), &panel(), &waves(&[4]), &run()));
    let units = panel().units().to_vec();
    let expected: Vec<ProposedWave> = propose_waves(
        &units,
        &[4],
        day_ordinal(date(2026, 3, 9)),
        14,
        None,
        8841207,
    )
    .unwrap();
    assert_eq!(
        wave_plan(&p),
        plan_waves(oxy_experiment::Grain::Day, &expected).unwrap()
    );
}

#[test]
fn blocks_cut_strata_that_partition_the_panel_and_balance_every_wave() {
    let mut c = waves(&[6]);
    c.blocks = Some(3);
    let p = priced(price_waves(&draft(7), &panel(), &c, &run()));
    let strata = p
        .strata
        .clone()
        .expect("a blocked proposal carries its strata");
    assert_eq!(strata.len(), 3);
    let mut all: Vec<String> = strata.iter().flatten().cloned().collect();
    all.sort();
    assert_eq!(
        all,
        panel().units().to_vec(),
        "every unit sits in exactly one stratum"
    );
    for stratum in &strata {
        let treated = stratum
            .iter()
            .filter(|u| wave_plan(&p)[0].units.contains(u))
            .count();
        assert_eq!(
            treated, 2,
            "six treated units over three equal blocks is two each: {stratum:?}"
        );
    }
}

#[test]
fn a_stratified_proposal_is_what_the_engine_proposes_for_those_strata() {
    let mut c = waves(&[4]);
    c.blocks = Some(2);
    let p = priced(price_waves(&draft(7), &panel(), &c, &run()));
    // The window is the last `pre_days` (28) before today (2026-03-01 in Sydney).
    let to = day_ordinal(date(2026, 3, 1));
    let strata = propose_strata(&panel(), 2, to - 28, to).unwrap();
    assert_eq!(p.strata, Some(strata.clone()));
    let units = panel().units().to_vec();
    let expected = propose_waves(
        &units,
        &[4],
        day_ordinal(date(2026, 3, 9)),
        14,
        Some(&strata),
        8841207,
    )
    .unwrap();
    assert_eq!(
        wave_plan(&p),
        plan_waves(oxy_experiment::Grain::Day, &expected).unwrap()
    );
}

#[test]
fn more_blocks_than_the_engine_can_cut_are_refused_with_its_reason() {
    let mut c = waves(&[4]);
    c.blocks = Some(7);
    let PriceComputation::Refused { reason, .. } = price_waves(&draft(7), &panel(), &c, &run())
    else {
        panic!("must refuse");
    };
    assert!(reason.contains("no strata could be proposed"), "{reason}");
}

#[test]
fn a_looser_alpha_prices_a_smaller_mde_and_records_the_alpha_it_used() {
    let strict = priced(price_waves(&draft(7), &panel(), &waves(&[6]), &run()));
    let mut loose_c = waves(&[6]);
    loose_c.alpha = Some(0.4);
    let loose = priced(price_waves(&draft(7), &panel(), &loose_c, &run()));
    assert_eq!((strict.alpha, loose.alpha), (0.1, 0.4));
    assert!(
        loose.power.mde < strict.power.mde,
        "{} !< {}",
        loose.power.mde,
        strict.power.mde
    );
    assert_eq!(
        wave_plan(&strict),
        wave_plan(&loose),
        "the same seed draws the same split at either alpha"
    );
}

#[test]
fn a_group_candidate_prices_group_sums_and_scales_the_mde_by_the_mean_group_size() {
    let mut c = waves(&[6]);
    c.assign_by = Some("store.region".to_string());
    let mut r = run();
    r.membership = Some(membership());
    let p = priced(price_waves(&draft(7), &panel(), &c, &r));
    assert_eq!(p.assign_by.as_deref(), Some("store.region"));
    assert!(
        (p.mde_unit_day - p.power.mde / 5.0).abs() < 1e-12,
        "{} vs {}",
        p.mde_unit_day,
        p.power.mde
    );
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("spreads evenly") && w.contains("5.0")),
        "{:?}",
        p.warnings
    );
    let matrix = panel();
    assert!(
        wave_plan(&p)[0]
            .units
            .iter()
            .all(|u| matrix.units().contains(u)),
        "waves list group names, not members"
    );
}

#[test]
fn a_group_candidate_with_no_membership_is_refused_not_priced_as_units() {
    let mut c = waves(&[6]);
    c.assign_by = Some("store.region".to_string());
    let PriceComputation::Refused { reason, .. } = price_waves(&draft(7), &panel(), &c, &run())
    else {
        panic!("must refuse");
    };
    assert!(
        reason.contains("`assign_by: store.region`") && reason.contains("no group membership"),
        "{reason}"
    );
}

#[test]
fn a_waves_candidate_on_a_draft_without_windows_is_refused_naming_the_key() {
    // Review Focus 10. A draft of a shape not yet chosen owes no window (PR 2),
    // so a waves candidate must refuse rather than unwrap.
    for (drop, key) in [
        ("  pre_days: 28\n", "design.pre_days"),
        ("  post_days: 28\n", "design.post_days"),
    ] {
        let cfg = parse(&yaml("", "", 28, 28, 7).replace(drop, "")).expect("parses");
        let PriceComputation::Refused { reason, .. } =
            price_waves(&cfg, &panel(), &waves(&[4]), &run())
        else {
            panic!("must refuse");
        };
        assert!(reason.contains(&format!("`{key}`")), "{reason}");
    }
    // A switchback draft is such a draft too.
    let sb = parse(&switchback_yaml("", "", "", 2)).expect("parses");
    let PriceComputation::Refused { reason, .. } = price_waves(&sb, &panel(), &waves(&[4]), &run())
    else {
        panic!("must refuse");
    };
    assert!(
        reason.contains("`design.pre_days`") && reason.contains("`design.post_days`"),
        "{reason}"
    );
}

#[test]
fn zero_washout_warns_about_the_partial_first_day() {
    let p = priced(price_waves(&draft(0), &panel(), &waves(&[4]), &run()));
    assert!(
        p.warnings.iter().any(|w| w.contains("partial first day")),
        "{:?}",
        p.warnings
    );
    let quiet = priced(price_waves(&draft(7), &panel(), &waves(&[4]), &run()));
    assert!(
        quiet
            .warnings
            .iter()
            .all(|w| !w.contains("partial first day"))
    );
}

#[test]
fn a_unit_the_file_names_but_the_panel_lacks_refuses_with_the_unit_named() {
    let ghost = "  waves:\n    - { switch_on: 2026-03-09, units: [ghost] }\n  seed: 1\n";
    let PriceComputation::Refused { reason, problems } =
        price_waves(&registered(ghost, 7), &panel(), &waves(&[4]), &run())
    else {
        panic!("must refuse");
    };
    assert!(reason.contains("problem"), "{reason}");
    assert!(problems.iter().any(|p| p.contains("ghost")), "{problems:?}");
}

#[test]
fn a_request_the_engine_cannot_place_is_refused_not_panicked() {
    let c = price_waves(&draft(7), &panel(), &waves(&[50]), &run());
    let PriceComputation::Refused { reason, .. } = c else {
        panic!("must refuse")
    };
    assert!(reason.contains("no waves could be proposed"), "{reason}");
}

#[test]
fn the_registration_deadline_in_the_warning_is_the_start_of_the_local_day() {
    // First switch 2026-03-09, anticipation 3 → the start of 2026-03-06 in
    // Australia/Sydney. Sydney is UTC+11 in March (DST until the first Sunday of
    // April), so that instant is 2026-03-05T13:00:00Z.
    let mut late = run();
    late.now = Utc.with_ymd_and_hms(2026, 3, 6, 0, 0, 0).unwrap();
    let p = priced(price_waves(&draft(7), &panel(), &waves(&[4]), &late));
    let warning = p
        .warnings
        .iter()
        .find(|w| w.contains("registration deadline"))
        .expect("a deadline warning");
    assert!(warning.contains("2026-03-05 13:00:00 UTC"), "{warning}");
}

#[test]
fn no_deadline_warning_while_the_deadline_is_ahead() {
    let p = priced(price_waves(&draft(7), &panel(), &waves(&[4]), &run()));
    assert!(
        p.warnings
            .iter()
            .all(|w| !w.contains("registration deadline")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn a_file_that_already_carries_waves_prices_as_if_it_did_not() {
    // Re-pricing a file that already carries waves replaces them; the old ones
    // must not leak into the design.
    let old = TWO_WAVES
        .replace("bondi, newtown", "u00, u01")
        .replace("manly", "u02");
    let a = priced(price_waves(
        &registered(&old, 7),
        &panel(),
        &waves(&[4]),
        &run(),
    ));
    let b = priced(price_waves(&draft(7), &panel(), &waves(&[4]), &run()));
    assert_eq!(wave_plan(&a), wave_plan(&b));
}

#[test]
fn waves_written_for_another_grouping_are_ignored_with_a_note() {
    // The draft carries waves naming units; this candidate is a group design, so
    // those names are not groups of the panel — and must not refuse it.
    let by_unit = "  waves:\n    - { switch_on: 2026-03-09, units: [bondi] }\n  seed: 1\n";
    let mut c = waves(&[6]);
    c.assign_by = Some("store.region".to_string());
    let mut r = run();
    r.membership = Some(membership());
    let p = priced(price_waves(&registered(by_unit, 7), &panel(), &c, &r));
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("were ignored") && w.contains("`assign_by: store.region`")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn a_draft_assign_by_the_unit_candidate_does_not_use_is_named_to_delete() {
    let cfg = parse(&super::testing::with_assign_by(
        &yaml("", "", 28, 28, 7),
        "store.region",
    ))
    .expect("parses");
    let p = priced(price_waves(&cfg, &panel(), &waves(&[4]), &run()));
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("`assign_by: store.region`") && w.contains("delete")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn infinite_power_values_serialize_as_null() {
    // Index C2b: "infinite values serialize as null". serde_json does this itself;
    // the read models rely on it, so pin it.
    assert!(serde_json::to_value(f64::INFINITY).unwrap().is_null());
    assert!(serde_json::to_value(f64::NAN).unwrap().is_null());
}

#[test]
fn matched_pairs_treat_exactly_one_unit_of_each_baseline_pair() {
    // Few geo units: pair markets of like baseline and treat one of each pair.
    let mut c = waves(&[]);
    c.pairs = true;
    let p = priced(price_waves(&draft(7), &panel(), &c, &run()));
    let strata = p.strata.clone().expect("pairs are strata");
    assert_eq!(strata.len(), UNITS / 2);
    let plan = wave_plan(&p);
    assert_eq!(plan.len(), 1, "one switch date");
    assert_eq!(plan[0].units.len(), UNITS / 2);
    for pair in &strata {
        assert_eq!(pair.len(), 2, "{pair:?}");
        let treated = pair.iter().filter(|u| plan[0].units.contains(u)).count();
        assert_eq!(treated, 1, "one of each pair: {pair:?}");
    }
}

#[test]
fn a_ratio_outcome_is_refused_with_its_reason() {
    let mut cfg = draft(7);
    cfg.outcome.denominator = Some("store_days.covers".into());
    let c = price_design(&cfg, &panel(), &Candidate::Waves(waves(&[6])), &run());
    let PriceComputation::Refused { reason, .. } = c else {
        panic!("a ratio outcome is not priced")
    };
    assert!(reason.contains("ratio"), "{reason}");
}
