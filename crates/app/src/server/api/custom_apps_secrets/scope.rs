//! Where an app's secrets live per **environment**, and which of them a run's
//! `ctx.env` sees (`internal-docs/2026-09-10-custom-app-environments-design.md`
//! §4.2).
//!
//! Production's keys are `apps/<app_id>/<KEY>`, as before environments. Every
//! other environment has its own path beneath, `apps/<app_id>/<env>/<KEY>`:
//!
//! - **Production lists top-level keys only.** A name with a further `/` is an
//!   environment's, never production's `ctx.env` — without this rule a staging
//!   value would reach production as `ctx.env["staging/KEY"]`.
//! - **An environment reads its own path first.** It falls back to
//!   production's value **only** for a key the running build marks
//!   `"shared": true` in `oxy-app.json`'s `env` block; an unshared key with no
//!   environment value reads as unset. So staging never holds a production
//!   credential it was not explicitly given.
//! - **A sandbox reads staging's path next** (`dev-<handle>`,
//!   `internal-docs/custom-app-sandboxes.md`): its own value, then staging's
//!   for any key staging holds, then production's for a `shared` key. Staging
//!   itself never reads a sandbox's, and no sandbox reads another's.
//! - **`ctx.secrets.set` writes the environment's path**, and is refused for a
//!   key the run read through a fallback (`EnvPolicy::read_through_fallback`)
//!   — a rotation would fork the grant of the environment it was read from: a
//!   sandbox rotating a token it read from staging voids staging's.

use std::collections::BTreeSet;

use oxy::service::secret_manager::SecretManagerService;
use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

/// The environment segment of a secret path: `None` for production.
pub(crate) fn environment_segment(environment: &AppEnvironment) -> Option<String> {
    match environment {
        AppEnvironment::Production => None,
        other => Some(other.name()),
    }
}

/// The storage name of `key` in `environment` of the app.
pub(crate) fn secret_name(app_id: Uuid, environment: &AppEnvironment, key: &str) -> String {
    SecretManagerService::app_secret_name(app_id, environment_segment(environment).as_deref(), key)
}

/// The prefix of the app's keys in `environment`, with its trailing slash.
pub(crate) fn secret_prefix(app_id: Uuid, environment: &AppEnvironment) -> String {
    secret_name(app_id, environment, "")
}

/// Whether a stored secret name is a non-production environment's
/// (`apps/<app_id>/<env>/<KEY>`). Those are staff-only
/// (`custom_apps_secrets::environment`), so the tenant project-secrets routes
/// neither list them nor reach them by id.
pub(crate) fn is_non_production_app_secret(name: &str) -> bool {
    let Some((app_id, rest)) = name.strip_prefix("apps/").and_then(|r| r.split_once('/')) else {
        return false;
    };
    Uuid::parse_str(app_id).is_ok() && rest.contains('/')
}

/// One key `ctx.env` resolves: its bare name, the stored name the value is
/// read from, and whether that is another environment's — staging's, for a
/// sandbox, or production's through the shared fallback.
/// (Read by the functions runtime only, hence the feature-gated allow.)
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvEntry {
    pub key: String,
    pub stored_name: String,
    pub through_fallback: bool,
}

/// Which stored secrets `ctx.env` resolves for a run in `environment`, given
/// every stored name of the project and the build's `shared` keys. Pure, so
/// the overlay rules are testable without a database.
///
/// The layers, nearest first; a key is taken from the first that holds it:
///
/// 1. the environment's own path;
/// 2. **for a sandbox only**, staging's path — every key staging holds,
///    `shared` or not: a staging value is already a non-production credential,
///    so a sandbox starts with the ones its app was given for testing;
/// 3. outside production, production's path, for a `shared` key only.
///
/// Everything from layers 2 and 3 is `through_fallback`: the run did not store
/// it, and `ctx.secrets.set` refuses to write it.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
pub(crate) fn plan_env(
    app_id: Uuid,
    environment: &AppEnvironment,
    shared: &BTreeSet<String>,
    stored_names: &[String],
) -> Vec<EnvEntry> {
    let layer = |of: &AppEnvironment| keys_under(&secret_prefix(app_id, of), stored_names);
    let mut plan: Vec<EnvEntry> = Vec::new();
    let mut take = |keys: Vec<(String, String)>, through_fallback: bool| {
        for (key, stored_name) in keys {
            if !plan.iter().any(|taken| taken.key == key) {
                plan.push(EnvEntry {
                    key,
                    stored_name,
                    through_fallback,
                });
            }
        }
    };
    take(layer(environment), false);
    if matches!(environment, AppEnvironment::Dev { .. }) {
        take(layer(&AppEnvironment::Staging), true);
    }
    if environment_segment(environment).is_some() {
        let production = layer(&AppEnvironment::Production);
        take(
            production
                .into_iter()
                .filter(|(key, _)| shared.contains(key))
                .collect(),
            true,
        );
    }
    plan.sort_by(|a, b| a.key.cmp(&b.key));
    plan
}

/// `(KEY, stored name)` for every name directly under `prefix` — one segment
/// deeper and no more, so production's listing never picks up an
/// environment's `staging/KEY`.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
fn keys_under(prefix: &str, stored_names: &[String]) -> Vec<(String, String)> {
    stored_names
        .iter()
        .filter_map(|name| {
            let key = name.strip_prefix(prefix)?;
            (!key.is_empty() && !key.contains('/')).then(|| (key.to_string(), name.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Uuid {
        Uuid::from_u128(7)
    }

    fn names(keys: &[&str]) -> Vec<String> {
        keys.iter()
            .map(|k| format!("apps/{}/{k}", app()))
            .chain(std::iter::once(format!("apps/{}/X", Uuid::from_u128(8))))
            .collect()
    }

    fn shared(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    fn resolved(plan: &[EnvEntry]) -> Vec<(&str, bool)> {
        plan.iter()
            .map(|e| (e.key.as_str(), e.through_fallback))
            .collect()
    }

    #[test]
    fn production_lists_top_level_keys_only() {
        let plan = plan_env(
            app(),
            &AppEnvironment::Production,
            &shared(&["API_KEY"]),
            &names(&["API_KEY", "TOKEN", "staging/TOKEN", "dev-luong/TOKEN"]),
        );
        assert_eq!(resolved(&plan), vec![("API_KEY", false), ("TOKEN", false)]);
        assert_eq!(plan[1].stored_name, format!("apps/{}/TOKEN", app()));
    }

    #[test]
    fn staging_reads_its_own_path_and_falls_back_only_for_shared_keys() {
        let plan = plan_env(
            app(),
            &AppEnvironment::Staging,
            &shared(&["API_KEY", "ONLY_SHARED_NAME"]),
            &names(&["API_KEY", "TOKEN", "UNSHARED", "staging/TOKEN"]),
        );
        assert_eq!(
            resolved(&plan),
            vec![("API_KEY", true), ("TOKEN", false)],
            "an unshared key with no staging value reads as unset"
        );
        assert_eq!(plan[1].stored_name, format!("apps/{}/staging/TOKEN", app()));
        assert_eq!(plan[0].stored_name, format!("apps/{}/API_KEY", app()));
    }

    #[test]
    fn an_environment_value_wins_over_the_shared_fallback() {
        let plan = plan_env(
            app(),
            &AppEnvironment::Staging,
            &shared(&["API_KEY"]),
            &names(&["API_KEY", "staging/API_KEY"]),
        );
        assert_eq!(resolved(&plan), vec![("API_KEY", false)]);
        assert_eq!(
            plan[0].stored_name,
            format!("apps/{}/staging/API_KEY", app())
        );
    }

    fn sandbox(handle: &str) -> AppEnvironment {
        AppEnvironment::Dev {
            handle: handle.into(),
        }
    }

    fn sources(plan: &[EnvEntry]) -> Vec<(&str, &str, bool)> {
        plan.iter()
            .map(|e| {
                let stored = e
                    .stored_name
                    .strip_prefix(&format!("apps/{}/", app()))
                    .expect("this app's");
                (e.key.as_str(), stored, e.through_fallback)
            })
            .collect()
    }

    /// A sandbox reads its own value, then staging's, then production's for a
    /// `shared` key — and everything it did not store itself is read through
    /// a fallback, which is what `ctx.secrets.set` refuses.
    #[test]
    fn a_sandbox_reads_its_own_then_stagings_then_shared_production() {
        let plan = plan_env(
            app(),
            &sandbox("a1"),
            &shared(&["SHARED", "SHARED_STG", "SHARED_OWN"]),
            &names(&[
                // In all three: the sandbox's own wins.
                "OWN",
                "staging/OWN",
                "dev-a1/OWN",
                // Staging and production: staging's wins, shared or not.
                "STG",
                "staging/STG",
                "SHARED_STG",
                "staging/SHARED_STG",
                // Shared, own value held: the sandbox's.
                "SHARED_OWN",
                "dev-a1/SHARED_OWN",
                // Production only: read only when shared.
                "SHARED",
                "UNSHARED",
                // Staging only: staging's keys need no `shared` mark.
                "staging/STG_ONLY",
                // Another sandbox's: never this one's.
                "dev-b2/OTHER",
                "dev-a1-b/PREFIXED",
            ]),
        );
        assert_eq!(
            sources(&plan),
            vec![
                ("OWN", "dev-a1/OWN", false),
                ("SHARED", "SHARED", true),
                ("SHARED_OWN", "dev-a1/SHARED_OWN", false),
                ("SHARED_STG", "staging/SHARED_STG", true),
                ("STG", "staging/STG", true),
                ("STG_ONLY", "staging/STG_ONLY", true),
            ]
        );
    }

    /// Staging does not read a sandbox's values, and one sandbox does not
    /// read another's: the staging layer is a sandbox's alone.
    #[test]
    fn staging_and_another_sandbox_never_read_a_sandboxs_value() {
        let stored = names(&["dev-a1/TOKEN", "API_KEY"]);
        let staging = plan_env(
            app(),
            &AppEnvironment::Staging,
            &shared(&["TOKEN"]),
            &stored,
        );
        assert!(sources(&staging).is_empty(), "{staging:?}");
        let other = plan_env(app(), &sandbox("b2"), &shared(&["TOKEN"]), &stored);
        assert!(sources(&other).is_empty(), "{other:?}");
        let production = plan_env(
            app(),
            &AppEnvironment::Production,
            &BTreeSet::new(),
            &stored,
        );
        assert_eq!(sources(&production), vec![("API_KEY", "API_KEY", false)]);
    }

    /// A sandbox's own secrets sit under a prefix no other sandbox's name
    /// starts with, so removing one's never removes another's.
    #[test]
    fn a_sandboxs_prefix_is_its_own() {
        let a = secret_prefix(app(), &sandbox("a1"));
        assert_eq!(a, format!("apps/{}/dev-a1/", app()));
        for other in [
            secret_name(app(), &sandbox("a1-b"), "K"),
            secret_name(app(), &AppEnvironment::Staging, "K"),
            secret_name(app(), &AppEnvironment::Production, "K"),
            secret_name(Uuid::from_u128(8), &sandbox("a1"), "K"),
        ] {
            assert!(!other.starts_with(&a), "{other}");
        }
        assert!(is_non_production_app_secret(&secret_name(
            app(),
            &sandbox("a1"),
            "K"
        )));
    }

    #[test]
    fn only_an_environment_path_is_staff_only() {
        let staging = secret_name(app(), &AppEnvironment::Staging, "K");
        assert!(is_non_production_app_secret(&staging));
        for tenant in [
            secret_name(app(), &AppEnvironment::Production, "K"),
            "STRIPE_KEY".to_string(),
            "apps/not-a-uuid/x/K".to_string(),
        ] {
            assert!(!is_non_production_app_secret(&tenant), "{tenant}");
        }
    }

    #[test]
    fn each_environment_writes_a_path_no_other_reads_as_its_own() {
        let production = secret_name(app(), &AppEnvironment::Production, "K");
        let staging = secret_name(app(), &AppEnvironment::Staging, "K");
        assert_eq!(production, format!("apps/{}/K", app()));
        assert_eq!(staging, format!("apps/{}/staging/K", app()));
        let only_staging = plan_env(
            app(),
            &AppEnvironment::Production,
            &BTreeSet::new(),
            &[staging],
        );
        assert!(
            only_staging.is_empty(),
            "production never reads staging's key"
        );
    }
}
