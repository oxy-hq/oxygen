//! The Airhouse plane's modes in the policy: what `ctx.airhouse.query`
//! decides under each, that writes stay in the sibling under every mode, and
//! the `snapshot` pin's window.

use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::airhouse_overlay::*;
use super::*;
use crate::custom_apps_planes::AirhouseMode;

const MODES: [AirhouseMode; 3] = [
    AirhouseMode::WriteOnly,
    AirhouseMode::Branch,
    AirhouseMode::Snapshot,
];

/// The extra golden section for the overlay modes: `airhouse.query` reads
/// through the overlay, and nothing else the plane does changes.
#[test]
fn query_decides_by_mode_and_writes_stay_in_the_sibling() {
    let decided: Vec<(AirhouseMode, Decision)> =
        MODES.iter().map(|m| (*m, query_decision(*m))).collect();
    assert_eq!(
        decided,
        vec![
            (AirhouseMode::WriteOnly, Decision::Allow),
            (
                AirhouseMode::Branch,
                Decision::Isolate(Target::AirhouseOverlay)
            ),
            (
                AirhouseMode::Snapshot,
                Decision::Isolate(Target::AirhouseSnapshot)
            ),
        ]
    );
    for op in [HostOp::AirhouseExec, HostOp::AirhouseAppend] {
        assert_eq!(
            op.spec().staging,
            Staged::Decided(Decision::Isolate(Target::SiblingSchema)),
            "{op:?} lands in the sibling under every mode"
        );
    }
}

/// Under the default mode the policy is today's: `airhouse.query` is
/// `Allow` in staging and in a sandbox (the golden file pins the rest).
#[test]
fn the_default_mode_reads_production() {
    for env in [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a dev slot"),
    ] {
        assert_eq!(
            EnvPolicy::for_environment(env).decide(HostOp::AirhouseQuery),
            Decision::Allow
        );
    }
}

#[test]
fn the_overlay_key_is_never_a_preview_key() {
    let app = Uuid::nil();
    let key = overlay_key(app, &AppEnvironment::parse("dev-a1").unwrap());
    assert_eq!(key, format!("env:{app}:dev-a1"));
    assert!(airhouse::preview_sql::PreviewNamespace::from_key(&key).is_err());
    assert_ne!(
        key,
        overlay_key(app, &AppEnvironment::Staging),
        "one key per environment, so no overlay is layered on another"
    );
}

#[test]
fn the_reset_fix_names_the_command() {
    let fix = reset_fix("store-ops", &AppEnvironment::parse("dev-a1").unwrap());
    assert_eq!(
        fix,
        "re-pin it: oxyc env reset store-ops dev-a1 --plane airhouse"
    );
}
