use super::*;
use crate::test_fixtures::{draft, full, names};

/// The eight stores `full()`'s two strata block.
fn eight() -> Vec<String> {
    names(&[
        "bondi",
        "manly",
        "parramatta",
        "chatswood",
        "newtown",
        "glebe",
        "surry_hills",
        "balmain",
    ])
}

#[test]
fn the_specs_example_over_its_own_eight_units_has_no_refusal() {
    assert_eq!(validate_against_units(&full(), &eight()), vec![]);
}

#[test]
fn a_unit_in_no_stratum_is_refused() {
    // `coogee` exists in the pool being pinned but no block holds it.
    let mut units = eight();
    units.push("coogee".into());
    let errs = validate_against_units(&full(), &units);
    assert_eq!(
        errs,
        vec![ConfigError::UnitInNoStratum {
            unit: "coogee".into()
        }]
    );
    let msg = errs[0].to_string();
    assert!(
        msg.contains("\"coogee\"") && msg.contains("design.strata"),
        "{msg}"
    );
    assert!(
        !msg.contains("two") && !msg.contains("unknown"),
        "states only its own guard: {msg}"
    );
}

#[test]
fn a_stratum_name_the_entity_does_not_hold_is_refused_with_suggestions() {
    // The strata say `Bondi`; the entity holds `bondi`. Two true statements:
    // `Bondi` is not a name, and `bondi` is in no stratum.
    let mut c = full();
    c.design.strata.as_mut().unwrap()[0][0] = "Bondi".into();
    let errs = validate_against_units(&c, &eight());
    assert_eq!(
        errs,
        vec![
            ConfigError::UnknownStratumUnit {
                unit: "Bondi".into(),
                stratum: 1,
                suggestions: names(&["bondi"])
            },
            ConfigError::UnitInNoStratum {
                unit: "bondi".into()
            },
        ]
    );
    let msg = errs[0].to_string();
    assert!(
        msg.contains("\"Bondi\"") && msg.contains("stratum 1") && msg.contains("\"bondi\""),
        "{msg}"
    );
}

#[test]
fn strata_are_checked_on_a_draft_that_already_carries_them() {
    let mut d = draft();
    d.design.strata = Some(vec![names(&["bondi"]), names(&["manly"])]);
    let errs = validate_against_units(&d, &names(&["bondi", "manly", "coogee"]));
    assert_eq!(
        errs,
        vec![ConfigError::UnitInNoStratum {
            unit: "coogee".into()
        }]
    );
}

#[test]
fn a_file_without_strata_owes_no_stratum() {
    assert_eq!(
        validate_against_units(&draft(), &names(&["bondi", "manly"])),
        vec![]
    );
}

#[test]
fn an_empty_entity_reports_every_stratum_name_as_unknown_and_none_as_missing() {
    // full(): 3 wave units + 8 stratum names, none of them held by an empty entity.
    let errs = validate_against_units(&full(), &[]);
    assert_eq!(errs.len(), 11, "{errs:?}");
    assert!(
        !errs
            .iter()
            .any(|e| matches!(e, ConfigError::UnitInNoStratum { .. }))
    );
}
