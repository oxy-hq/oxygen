//! A promoting publish meets the blockers a promote meets — and, with the
//! four-eyes rule on, always the second-person one.

use super::*;
use crate::custom_apps_agent_fixture as fixture;
use crate::custom_apps_promote::error::{
    REQUIRED_SECRETS_UNSET, SAME_PERSON_PROMOTE, SCHEDULE_REMOVAL_UNCONFIRMED,
};

fn app(rule: bool) -> apps::Model {
    let mut row = fixture::app(Uuid::from_u128(9), Uuid::from_u128(7), Uuid::nil());
    row.require_second_person_to_promote = rule;
    row
}

fn build() -> app_builds::Model {
    serde_json::from_value(serde_json::json!({
        "id": Uuid::from_u128(2),
        "app_id": Uuid::from_u128(9),
        "build_id": "b-7",
        "s3_prefix": "customer-apps/x/builds/b-7/",
        "manifest_json": null,
        "created_at": chrono::Utc::now().fixed_offset(),
        "published_by": Uuid::from_u128(0xA11CE),
        "published_via": null,
        "source_repo": null,
        "commit_sha": null,
        "source_branch": null,
        "validation_status": "passed",
        "validation_detail": null,
        "semantic_revision_id": null,
    }))
    .expect("an app_builds row")
}

fn codes(blockers: &[Blocker]) -> Vec<&'static str> {
    blockers.iter().map(|b| b.code).collect()
}

/// Every blocker a promote would list, in its table's order.
fn refused(
    rule: bool,
    unset: &[String],
    removals: &[String],
    confirmed: &[String],
) -> Vec<Blocker> {
    refused_by(Facts {
        app: &app(rule),
        build: &build(),
        required_unset: unset,
        schedule_removals: removals,
        confirmed_removals: confirmed,
        checks: None,
    })
}

#[test]
fn a_promoting_publish_with_nothing_wrong_passes() {
    assert!(refused(false, &[], &[], &[]).is_empty());
}

/// Parity: the two checks a promoting publish used to skip.
#[test]
fn an_unset_required_key_and_an_unconfirmed_retirement_refuse_it() {
    let unset = ["STRIPE_KEY".to_string()];
    let removals = ["nightly".to_string()];
    let blockers = refused(false, &unset, &removals, &[]);
    assert_eq!(
        codes(&blockers),
        vec![REQUIRED_SECRETS_UNSET, SCHEDULE_REMOVAL_UNCONFIRMED]
    );
    assert!(blockers[0].message.contains("STRIPE_KEY"));
    assert!(blockers[1].message.contains("--remove-schedule"));
    // Naming the retired schedule confirms it, as `oxyc promote` does.
    assert_eq!(
        codes(&refused(false, &[], &removals, &removals)),
        Vec::<&str>::new()
    );
}

/// The publisher is the promoter, so with the rule on a promoting publish is
/// always refused — and with it off, never for that reason.
#[test]
fn the_four_eyes_rule_refuses_every_promoting_publish_while_it_is_on() {
    assert_eq!(
        codes(&refused(true, &[], &[], &[])),
        vec![SAME_PERSON_PROMOTE]
    );
    assert!(refused(false, &[], &[], &[]).is_empty());
    let unset = ["STRIPE_KEY".to_string()];
    assert_eq!(
        codes(&refused(true, &unset, &[], &[])),
        vec![REQUIRED_SECRETS_UNSET, SAME_PERSON_PROMOTE]
    );
}

/// With the promote checks gate on, the build a promoting publish just made
/// has no staging attempt, so every check the app requires is unmet; an app
/// that requires none is not held.
#[test]
fn the_promote_checks_gate_refuses_a_promoting_publish_of_an_unchecked_build() {
    use crate::custom_apps_build_checks::settings::Settings;
    use crate::custom_apps_promote::error::CHECKS_NOT_PASSED;

    let refused_with = |settings: &Settings| {
        let checks = checks_gate::evaluate(settings, Vec::new(), chrono::Utc::now());
        refused_by(Facts {
            app: &app(false),
            build: &build(),
            required_unset: &[],
            schedule_removals: &[],
            confirmed_removals: &[],
            checks: Some(&checks),
        })
    };
    let blockers = refused_with(&Settings::default());
    assert_eq!(codes(&blockers), vec![CHECKS_NOT_PASSED]);
    assert!(
        blockers[0].message.contains("Every screen opens (not run)"),
        "{}",
        blockers[0].message
    );
    let nothing_required = Settings {
        required: Some(Vec::new()),
        ..Settings::default()
    };
    assert!(refused_with(&nothing_required).is_empty());
}

/// The sentence says nothing landed and carries every blocker's own.
#[test]
fn the_refusal_says_nothing_landed_and_names_each_blocker() {
    let unset = ["STRIPE_KEY".to_string()];
    let message = refused_message(&refused(true, &unset, &[], &[]));
    assert!(message.contains("nothing of it was published"), "{message}");
    assert!(message.contains("staging"), "{message}");
    assert!(message.contains("STRIPE_KEY"), "{message}");
    assert!(message.contains("second person"), "{message}");
}
