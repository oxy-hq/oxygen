use super::*;
use crate::config::{ExperimentConfig, WaveConfig};
use crate::test_fixtures::{date, draft, full, names, switchback};

/// `full()` without its strata. The spec's example blocks eight stores into two
/// strata; these tests are about waves and the control pool, and
/// `tests_strata` owns the strata checks.
fn unstratified() -> ExperimentConfig {
    let mut c = full();
    c.design.strata = None;
    c
}

/// `unstratified()` with one wave that names exactly `units`.
fn one_wave(units: &[&str]) -> ExperimentConfig {
    let mut c = unstratified();
    c.design.waves = vec![WaveConfig {
        switch_on: date(2026, 3, 2).into(),
        units: names(units),
    }];
    c
}

#[test]
fn unit_name_near_miss_is_refused_with_suggestions() {
    // Review Focus 1. The panel is what the warehouse rendered as strings.
    let panel = names(&["bondi", "newtown", "manly", "coogee", "12"]);
    let cases = [
        ("Bondi", "bondi"),     // case
        ("bondi ", "bondi"),    // trailing space
        (" 12", "12"),          // leading space on a numeric key
        ("12.0", "12"),         // a number rendered differently
        ("newtwon", "newtown"), // a transposition (2 edits)
    ];
    for (typed, expected) in cases {
        let errs = validate_against_units(&one_wave(&[typed]), &panel);
        let [
            ConfigError::UnknownUnit {
                unit,
                wave,
                suggestions,
                ..
            },
        ] = errs.as_slice()
        else {
            panic!("{typed:?}: expected exactly one UnknownUnit, got {errs:?}");
        };
        assert_eq!(unit, typed, "the refusal names the unit exactly as typed");
        assert_eq!(*wave, 1);
        assert!(
            suggestions.iter().any(|s| s == expected),
            "{typed:?}: expected {expected:?} among {suggestions:?}"
        );
        let msg = errs[0].to_string();
        assert!(
            msg.contains(&format!("{typed:?}")),
            "message names the unmatched unit: {msg}"
        );
        assert!(
            msg.contains(&format!("{expected:?}")),
            "message names the near panel name: {msg}"
        );
    }
}

#[test]
fn a_far_name_is_refused_and_says_nothing_is_close() {
    let errs = validate_against_units(&one_wave(&["zzzzzz"]), &names(&["bondi", "manly"]));
    let [ConfigError::UnknownUnit { suggestions, .. }] = errs.as_slice() else {
        panic!("expected one UnknownUnit, got {errs:?}");
    };
    assert!(suggestions.is_empty());
    assert!(
        errs[0].to_string().contains("within 2 edits"),
        "{}",
        errs[0]
    );
}

#[test]
fn suggestions_are_capped_at_three_nearest_first() {
    let panel = names(&["ab", "ac", "ad", "ae", "aa1"]);
    let errs = validate_against_units(&one_wave(&["aa"]), &panel);
    let [ConfigError::UnknownUnit { suggestions, .. }] = errs.as_slice() else {
        panic!("expected one UnknownUnit, got {errs:?}");
    };
    assert_eq!(suggestions, &names(&["aa1", "ab", "ac"]));
}

#[test]
fn a_near_miss_is_not_counted_as_a_placed_unit() {
    // `Manly` is not `manly`: it must not both be refused AND make the panel
    // look fully placed. Here bondi+newtown are placed, manly is not.
    let mut c = unstratified();
    c.design.waves[1].units = names(&["Manly"]);
    let errs = validate_against_units(&c, &names(&["bondi", "newtown", "manly"]));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(matches!(errs[0], ConfigError::UnknownUnit { .. }));
}

#[test]
fn every_unit_switched_on_one_day_leaves_no_control_and_is_refused() {
    let errs = validate_against_units(
        &one_wave(&["bondi", "newtown", "manly"]),
        &names(&["bondi", "newtown", "manly"]),
    );
    assert_eq!(errs, vec![ConfigError::NoControlUnits { units: 3 }]);
    let msg = errs[0].to_string();
    assert!(msg.contains("3") && msg.contains("control"), "{msg}");
    assert!(
        !msg.contains("unidentified"),
        "states the guard, not the conclusion: {msg}"
    );
}

#[test]
fn every_unit_placed_over_two_switch_dates_is_a_fully_staggered_rollout() {
    // No permanent holdout: the later wave controls the earlier, and the last is dropped.
    assert_eq!(
        validate_against_units(&unstratified(), &names(&["bondi", "newtown", "manly"])),
        vec![]
    );
}

#[test]
fn one_unit_left_out_is_a_control_and_passes() {
    let errs = validate_against_units(
        &unstratified(),
        &names(&["bondi", "newtown", "manly", "coogee"]),
    );
    assert_eq!(errs, vec![]);
}

#[test]
fn a_draft_has_no_unit_refusals() {
    assert_eq!(
        validate_against_units(&draft(), &names(&["bondi", "manly"])),
        vec![]
    );
}

#[test]
fn an_empty_panel_refuses_each_wave_unit_and_never_reports_a_control_shortfall() {
    let errs = validate_against_units(&unstratified(), &[]);
    assert_eq!(errs.len(), 3, "{errs:?}");
    assert!(errs.iter().all(
        |e| matches!(e, ConfigError::UnknownUnit { suggestions, .. } if suggestions.is_empty())
    ));
}

/// `unstratified()` with `assign_by` set and one wave per entry of `groups`,
/// a fortnight apart.
fn grouped(groups: &[&[&str]]) -> ExperimentConfig {
    let mut c = unstratified();
    c.assign_by = Some("sku.category".into());
    c.design.waves = groups
        .iter()
        .enumerate()
        .map(|(i, g)| WaveConfig {
            switch_on: date(2026, 3, 2 + 14 * i as u32).into(),
            units: names(g),
        })
        .collect();
    c
}

#[test]
fn a_group_name_near_miss_is_refused_with_suggestions() {
    // Under assign_by the names validated are the dimension's values.
    let groups = names(&["Electronics", "Home", "Garden", "Toys"]);
    let errs = validate_against_units(&grouped(&[&["electronics"]]), &groups);
    let [
        ConfigError::UnknownUnit {
            unit, suggestions, ..
        },
    ] = errs.as_slice()
    else {
        panic!("expected exactly one UnknownUnit, got {errs:?}");
    };
    assert_eq!(unit, "electronics");
    assert_eq!(suggestions, &names(&["Electronics"]));
    let msg = errs[0].to_string();
    assert!(
        msg.contains("assign_by") && msg.contains("\"Electronics\""),
        "{msg}"
    );
}

#[test]
fn groups_left_out_of_every_wave_are_the_control_pool() {
    let groups = names(&["Electronics", "Home", "Garden", "Toys"]);
    assert_eq!(
        validate_against_units(&grouped(&[&["Electronics"], &["Home"]]), &groups),
        vec![]
    );
}

#[test]
fn every_group_switched_on_one_day_leaves_no_control() {
    let groups = names(&["Electronics", "Home"]);
    let errs = validate_against_units(&grouped(&[&["Electronics", "Home"]]), &groups);
    assert_eq!(errs, vec![ConfigError::NoControlUnits { units: 2 }]);
    assert!(errs[0].to_string().contains("assign_by"), "{}", errs[0]);
    assert_eq!(
        validate_against_units(&grouped(&[&["Electronics"], &["Home"]]), &groups),
        vec![],
        "staggered over two dates, every group placed is allowed"
    );
}

#[test]
fn a_switchback_names_no_unit_so_it_earns_no_unit_refusal() {
    assert_eq!(
        validate_against_units(&switchback(), &names(&["a", "b"])),
        vec![]
    );
    assert_eq!(validate_against_units(&switchback(), &[]), vec![]);
}

#[test]
fn unit_refusals_state_only_the_pool_they_tested() {
    // `validate_against_units` tests membership in the slice it is given, which
    // PR 4 narrows to the units with outcome history: no message may claim it
    // tested "the entity".
    let pool = names(&["bondi", "newtown", "manly"]);
    let unknown = validate_against_units(&one_wave(&["zzzzzz"]), &pool);
    let no_control = validate_against_units(&one_wave(&["bondi", "newtown", "manly"]), &pool);
    let mut c = unstratified();
    c.design.strata = Some(vec![names(&["bondi", "zzzzzz"])]);
    let strata = validate_against_units(&c, &pool);
    let pick = |want: fn(&ConfigError) -> bool| {
        strata
            .iter()
            .find(|e| want(e))
            .expect("the refusal is earned")
    };
    let msgs: Vec<String> = [
        &unknown[0],
        &no_control[0],
        pick(|e| matches!(e, ConfigError::UnknownStratumUnit { .. })),
        pick(|e| matches!(e, ConfigError::UnitInNoStratum { .. })),
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect();
    for msg in &msgs {
        assert!(!msg.contains("of the entity"), "{msg}");
        assert!(msg.contains("pool"), "{msg}");
    }
    assert!(
        msgs[0].contains("matches no name in the unit pool being checked"),
        "{}",
        msgs[0]
    );
    assert!(msgs[1].contains("all 3 units in the pool"), "{}", msgs[1]);
    assert!(
        msgs[2].contains("matches no name in the unit pool being checked"),
        "{}",
        msgs[2]
    );
    assert!(
        msgs[3].contains("of the pool is in no stratum"),
        "{}",
        msgs[3]
    );
}
