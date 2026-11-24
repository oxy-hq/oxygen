//! Unit tests of the install's constants, its error mapping and its summary.

use oxy_oltp::platform::{CHANGES_SCHEMA, CHANGES_SHAPES_TABLE, steps_since};

use super::{
    APPLYING, CHANGES_STEP, InstallError, InstallSummary, LATEST_SHAPE_SQL, MigrationError,
};

/// The step a store must have reached to hold any capture is the one that
/// creates `oxy_changes`.
#[test]
fn the_changes_step_creates_oxy_changes() {
    let steps = steps_since(CHANGES_STEP - 1, "neon", "neondb").expect("the steps");
    let step = steps.first().expect("a step at CHANGES_STEP");
    assert_eq!(step.version, CHANGES_STEP);
    let create = format!("CREATE SCHEMA IF NOT EXISTS {CHANGES_SCHEMA}");
    assert!(
        step.statements.iter().any(|s| s.starts_with(&create)),
        "{:?}",
        step.statements
    );
    assert!(LATEST_SHAPE_SQL.contains(CHANGES_SHAPES_TABLE));
}

/// An author error is refused as a migration's author error is, an infra
/// error retried as a migration's infra error is.
#[test]
fn errors_land_on_a_migrations_surfaces() {
    let author: MigrationError = InstallError::Author("leases has no key".into()).into();
    assert!(author.is_author_fault());
    assert!(!author.is_retryable());
    assert_eq!(author.to_string(), "leases has no key");

    let infra: MigrationError = InstallError::Infra("connection reset".into()).into();
    assert!(infra.is_retryable());
    assert!(!infra.is_author_fault());
    let said = infra.to_string();
    assert!(said.contains(APPLYING), "{said}");
    assert!(said.contains("connection reset"), "{said}");
}

#[test]
fn the_summary_says_only_what_changed() {
    assert_eq!(InstallSummary::default().summary(), "");
    let summary = InstallSummary {
        installed: vec!["leases".into(), "landlords".into()],
        reshaped: Vec::new(),
        removed: vec!["drafts".into()],
    };
    assert!(!summary.is_empty());
    assert_eq!(
        summary.summary(),
        "change capture installed on leases, landlords; removed from drafts"
    );
}
