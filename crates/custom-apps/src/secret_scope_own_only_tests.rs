//! `ctx.env` under the secrets plane's `empty` mode: the environment's own
//! path and nothing else.

use std::collections::BTreeSet;

use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::{EnvLookup, plan_env};

fn app() -> Uuid {
    Uuid::from_u128(7)
}

fn stored() -> Vec<String> {
    [
        "SHARED",
        "UNSHARED",
        "staging/SHARED",
        "staging/STG_ONLY",
        "staging/OWN",
        "dev-a1/OWN",
        "dev-a1/ONLY_SANDBOX",
    ]
    .iter()
    .map(|k| format!("apps/{}/{k}", app()))
    .collect()
}

fn shared() -> BTreeSet<String> {
    BTreeSet::from(["SHARED".to_string()])
}

fn keys(environment: &AppEnvironment, lookup: EnvLookup) -> Vec<(String, bool)> {
    plan_env(app(), environment, lookup, &shared(), &stored())
        .into_iter()
        .map(|e| (e.key, e.through_fallback))
        .collect()
}

fn own(keys: &[&str]) -> Vec<(String, bool)> {
    keys.iter().map(|k| (k.to_string(), false)).collect()
}

/// A sandbox gets neither staging's values nor production's `shared` ones.
#[test]
fn a_sandbox_that_is_empty_reads_only_its_own_path() {
    let sandbox = AppEnvironment::Dev {
        handle: "a1".into(),
    };
    assert_eq!(
        keys(&sandbox, EnvLookup::OwnOnly),
        own(&["ONLY_SANDBOX", "OWN"])
    );
    // The chain, by contrast, reaches both.
    let chain = keys(&sandbox, EnvLookup::Chain);
    assert!(chain.contains(&("STG_ONLY".to_string(), true)));
    assert!(chain.contains(&("SHARED".to_string(), true)));
}

/// Staging has no production `shared` fallback either.
#[test]
fn staging_that_is_empty_reads_only_its_own_path() {
    assert_eq!(
        keys(&AppEnvironment::Staging, EnvLookup::OwnOnly),
        own(&["OWN", "SHARED", "STG_ONLY"])
    );
    let shared_only = plan_env(
        app(),
        &AppEnvironment::Staging,
        EnvLookup::OwnOnly,
        &shared(),
        &[format!("apps/{}/SHARED", app())],
    );
    assert!(shared_only.is_empty(), "{shared_only:?}");
}

/// Nothing an `OwnOnly` run reads is through a fallback, so
/// `ctx.secrets.set` may overwrite any key it sees.
#[test]
fn nothing_an_empty_run_reads_is_through_a_fallback() {
    for environment in [
        AppEnvironment::Staging,
        AppEnvironment::Dev {
            handle: "a1".into(),
        },
    ] {
        assert!(
            keys(&environment, EnvLookup::OwnOnly)
                .iter()
                .all(|(_, through_fallback)| !through_fallback)
        );
    }
}
