//! The warehouse plane's override of the build's `nonProduction.destinations`
//! (`homes::with_destinations`): replaced key by key, read by the same
//! per-write check as the manifest's map.

use std::collections::BTreeMap;

use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::json;

use super::*;

const MAPPED: Decision = Decision::Isolate(Target::MappedDestination);

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .collect()
}

fn planes(environment: &AppEnvironment, overrides: serde_json::Value) -> ResolvedPlanes {
    PlaneSpec::parse(&json!({
        "warehouse": { "mode": "mapped", "destinations": overrides }
    }))
    .expect("a valid spec")
    .resolve(environment)
    .expect("a non-production environment has planes")
}

fn sandbox() -> AppEnvironment {
    AppEnvironment::Dev {
        handle: "a1".into(),
    }
}

/// The manifest maps `a` and `b`; the environment overrides `a`: `a` goes
/// where the environment says, `b` keeps the manifest's.
#[test]
fn an_override_replaces_the_manifest_key_by_key() {
    let env = sandbox();
    let policy = EnvPolicy::for_environment(env.clone())
        .with_planes(planes(&env, json!({ "a": "a_dev" })))
        .with_destinations(map(&[("a", "a_stg"), ("b", "b_stg")]));
    assert_eq!(policy.mapped_destination("a"), Some("a_dev"));
    assert_eq!(policy.mapped_destination("b"), Some("b_stg"));
    assert_eq!(
        policy.decide_on_database(HostOp::WarehouseInsert, "a"),
        MAPPED
    );
    assert_eq!(
        policy.destinations(),
        &map(&[("a", "a_dev"), ("b", "b_stg")])
    );
}

/// A database the manifest does not map was held; an override gives it a home.
#[test]
fn an_override_can_map_a_database_the_manifest_does_not() {
    let env = AppEnvironment::Staging;
    let held = EnvPolicy::for_environment(env.clone()).with_destinations(map(&[("b", "b_stg")]));
    assert_eq!(
        held.decide_on_database(HostOp::WarehouseExec, "c"),
        Decision::Hold
    );
    let policy = EnvPolicy::for_environment(env.clone())
        .with_planes(planes(&env, json!({ "c": "c_dev" })))
        .with_destinations(map(&[("b", "b_stg")]));
    assert_eq!(
        policy.decide_on_database(HostOp::WarehouseExec, "c"),
        MAPPED
    );
    assert_eq!(policy.mapped_destination("c"), Some("c_dev"));
}

/// The layers apply in either order: the build's map is read after the planes
/// in `run_policy`, and a caller that sets them the other way gets the same.
#[test]
fn the_override_wins_whichever_is_applied_first() {
    let env = sandbox();
    let manifest = map(&[("a", "a_stg")]);
    let planes_first = EnvPolicy::for_environment(env.clone())
        .with_planes(planes(&env, json!({ "a": "a_dev" })))
        .with_destinations(manifest.clone());
    let manifest_first = EnvPolicy::for_environment(env.clone())
        .with_destinations(manifest)
        .with_planes(planes(&env, json!({ "a": "a_dev" })));
    assert_eq!(planes_first.destinations(), manifest_first.destinations());
    assert_eq!(manifest_first.mapped_destination("a"), Some("a_dev"));
}

/// Merged, the map is judged as one: an override onto a database the manifest
/// maps is a chain, so it has no home.
#[test]
fn an_override_onto_a_mapped_production_database_holds() {
    let env = sandbox();
    let policy = EnvPolicy::for_environment(env.clone())
        .with_planes(planes(&env, json!({ "a": "b" })))
        .with_destinations(map(&[("b", "b_stg")]));
    assert_eq!(policy.mapped_destination("a"), None);
    assert_eq!(
        policy.decide_on_database(HostOp::WarehouseInsert, "a"),
        Decision::Hold
    );
}

#[test]
fn production_has_no_planes_and_ignores_an_override() {
    let policy = EnvPolicy::production()
        .with_planes(planes(&sandbox(), json!({ "a": "a_dev" })))
        .with_destinations(map(&[("a", "a_stg")]));
    assert_eq!(policy.mapped_destination("a"), None);
    assert!(policy.destinations().is_empty());
}
