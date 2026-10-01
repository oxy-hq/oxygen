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
//! - **`ctx.secrets.set` writes the environment's path**, and is refused for a
//!   key the run read through the fallback (`EnvPolicy::read_through_fallback`)
//!   — a rotation would fork production's grant.

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
/// read from, and whether that is production's through the shared fallback.
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
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
pub(crate) fn plan_env(
    app_id: Uuid,
    environment: &AppEnvironment,
    shared: &BTreeSet<String>,
    stored_names: &[String],
) -> Vec<EnvEntry> {
    let own = keys_under(&secret_prefix(app_id, environment), stored_names);
    let mut plan: Vec<EnvEntry> = own
        .iter()
        .map(|(key, stored_name)| EnvEntry {
            key: key.clone(),
            stored_name: stored_name.clone(),
            through_fallback: false,
        })
        .collect();
    if environment_segment(environment).is_some() {
        let production = keys_under(
            &secret_prefix(app_id, &AppEnvironment::Production),
            stored_names,
        );
        let own_keys: BTreeSet<&String> = own.iter().map(|(key, _)| key).collect();
        plan.extend(
            production
                .into_iter()
                .filter(|(key, _)| shared.contains(key) && !own_keys.contains(key))
                .map(|(key, stored_name)| EnvEntry {
                    key,
                    stored_name,
                    through_fallback: true,
                }),
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
