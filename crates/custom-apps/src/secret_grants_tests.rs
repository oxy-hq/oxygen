use std::collections::BTreeSet;

use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::{is_shareable, plan_granted};
use crate::secret_scope::{EnvLookup, plan_env};

fn app() -> Uuid {
    Uuid::from_u128(7)
}

fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

fn own(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| format!("apps/{}/{k}", app())).collect()
}

/// `(key, stored name, through_fallback)` of what the grants add.
fn granted(
    environment: &AppEnvironment,
    lookup: EnvLookup,
    shared: &[&str],
    own_keys: &[&str],
    names: &[&str],
) -> Vec<(String, String, bool)> {
    let shared = set(shared);
    let plan = plan_env(app(), environment, lookup, &shared, &own(own_keys));
    plan_granted(&plan, environment, lookup, &shared, &set(names))
        .into_iter()
        .map(|e| (e.key, e.stored_name, e.through_fallback))
        .collect()
}

fn entry(key: &str, through_fallback: bool) -> (String, String, bool) {
    (key.to_string(), key.to_string(), through_fallback)
}

#[test]
fn production_reads_every_shared_secret_under_its_own_name() {
    assert_eq!(
        granted(
            &AppEnvironment::Production,
            EnvLookup::Chain,
            &[],
            &[],
            &["ANTHROPIC_API_KEY", "SLACK_BOT_TOKEN"],
        ),
        vec![
            entry("ANTHROPIC_API_KEY", false),
            entry("SLACK_BOT_TOKEN", false)
        ]
    );
}

/// An app that set its own value of a key keeps it: sharing a workspace
/// secret of the same name never overrides what the app was given directly.
#[test]
fn the_apps_own_value_wins_over_a_shared_one() {
    assert_eq!(
        granted(
            &AppEnvironment::Production,
            EnvLookup::Chain,
            &[],
            &["SLACK_BOT_TOKEN"],
            &["ANTHROPIC_API_KEY", "SLACK_BOT_TOKEN"],
        ),
        vec![entry("ANTHROPIC_API_KEY", false)]
    );
}

/// A shared workspace secret is a production credential: staging reads one
/// only for a key the build marks `shared`, and through the fallback, as it
/// reads production's own app secrets.
#[test]
fn staging_reads_a_shared_secret_only_for_a_shared_key() {
    assert_eq!(
        granted(
            &AppEnvironment::Staging,
            EnvLookup::Chain,
            &["SLACK_BOT_TOKEN"],
            &[],
            &["ANTHROPIC_API_KEY", "SLACK_BOT_TOKEN"],
        ),
        vec![entry("SLACK_BOT_TOKEN", true)]
    );
}

#[test]
fn a_staging_value_wins_over_a_shared_secret() {
    let shared = set(&["SLACK_BOT_TOKEN"]);
    let staging = vec![format!("apps/{}/staging/SLACK_BOT_TOKEN", app())];
    let plan = plan_env(
        app(),
        &AppEnvironment::Staging,
        EnvLookup::Chain,
        &shared,
        &staging,
    );
    let added = plan_granted(
        &plan,
        &AppEnvironment::Staging,
        EnvLookup::Chain,
        &shared,
        &shared,
    );
    assert!(added.is_empty(), "{added:?}");
}

#[test]
fn a_sandbox_follows_the_same_rule() {
    let sandbox = AppEnvironment::Dev {
        handle: "a1".into(),
    };
    assert_eq!(
        granted(
            &sandbox,
            EnvLookup::Chain,
            &["SLACK_BOT_TOKEN"],
            &[],
            &["ANTHROPIC_API_KEY", "SLACK_BOT_TOKEN"],
        ),
        vec![entry("SLACK_BOT_TOKEN", true)]
    );
}

#[test]
fn an_environment_reading_its_own_path_alone_reads_no_shared_secret() {
    assert!(
        granted(
            &AppEnvironment::Staging,
            EnvLookup::OwnOnly,
            &["SLACK_BOT_TOKEN"],
            &[],
            &["SLACK_BOT_TOKEN"],
        )
        .is_empty()
    );
}

/// The grant route refuses one, but a name that is an app's own row must not
/// become another app's `ctx.env` even if a grant somehow named it.
#[test]
fn an_app_scoped_name_is_never_shared() {
    let other = format!("apps/{}/TOKEN", Uuid::from_u128(8));
    assert!(!is_shareable(&other));
    assert!(!is_shareable(""));
    assert!(is_shareable("SLACK_BOT_TOKEN"));
    assert!(
        granted(
            &AppEnvironment::Production,
            EnvLookup::Chain,
            &[],
            &[],
            &[other.as_str()],
        )
        .is_empty()
    );
}
