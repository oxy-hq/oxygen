//! `ctx.env` for a run (§11.4), resolved once and reused.
//!
//! Resolving it is the most expensive read on the pre-isolate path: list the
//! project's secrets, then `get_secret` each matching one — N+1 round trips,
//! the N on connections of their own — for values that change when a person
//! rotates a key, which is to say almost never. Until this existed every one
//! of the ~34k invocations a week paid it in full.
//!
//! ## What an entry is keyed by
//!
//! Everything the resolution reads: the project (whose secrets are listed),
//! the app and environment (whose paths `scope::plan_env` keeps), and the
//! build's `shared` keys (which production values an environment may read
//! through). A principal is not in the key because `ctx.env` does not depend
//! on one — the same app in the same environment reads the same values
//! whoever calls — so there is nothing of one caller's for another to read.
//!
//! ## How a rotation is seen
//!
//! Three ways, and the first is what makes the other two bounds rather than
//! promises:
//!
//! 1. **A stamp, checked on every hit.** One aggregate query — the newest
//!    `updated_at` under `apps/<app_id>/` and the row count, and the same pair
//!    over the workspace secrets shared with the app (a grant's `created_at`
//!    or its secret's `updated_at`, whichever is newer, and the grant count) —
//!    is compared to the stamp the entry was resolved under. Every set,
//!    rotation and delete moves `updated_at` (a delete deactivates the row in
//!    place), and every share and unshare moves the grant half, so a write
//!    from *any* writer on *any* replica — the staff secrets surface, a
//!    function's own `ctx.secrets.set`, a sandbox teardown, a workspace admin
//!    rotating or sharing a workspace secret — is read by the next call
//!    everywhere. One round trip in place of N+1.
//! 2. **Explicit invalidation** from the secrets write path
//!    (`custom_apps_secrets`), so the replica that took the write drops its
//!    entry without even the stamp read. A production write drops every
//!    environment's entry (they read production through the shared fallback);
//!    an environment's write drops that environment's alone.
//! 3. **A 60 s TTL**, the backstop for a stamp that did not move: a value
//!    rewritten inside the same microsecond, or a writer that bypassed
//!    `updated_at` altogether.
//!
//! A resolution that did not complete — a listing that failed, a value that
//! would not decrypt — is used for that run, as before, and not remembered:
//! a blip costs one call, not a minute of missing keys.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use lru::LruCache;
use oxy::service::secret_manager::SecretInfo;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::secret_grants;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tracing::error;
use uuid::Uuid;

use super::super::custom_apps_secrets::scope;
use super::env_policy::{EnvPolicy, SecretsHome};

const TTL: Duration = Duration::from_secs(60);
/// Apps × environments a replica serves live, with room to spare.
const MAX_ENTRIES: usize = 4096;

/// `ctx.env` for one run, and which of its keys came from production through
/// the shared fallback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ResolvedEnv {
    pub(super) values: BTreeMap<String, String>,
    pub(super) read_through_fallback: BTreeSet<String>,
    /// The keys read from a workspace secret shared with the app
    /// (`secret_grants`), in any environment.
    pub(super) shared_secrets: BTreeSet<String>,
}

impl ResolvedEnv {
    /// The values of the keys read through the fallback, never the keys: a
    /// provider-sandbox `ctx.fetch` carrying one is held
    /// (`host::provider_fetch`).
    #[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
    pub(super) fn fallback_values(&self) -> Vec<String> {
        self.read_through_fallback
            .iter()
            .filter_map(|key| self.values.get(key).cloned())
            .collect()
    }
}

/// Everything a resolution depends on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct EnvKey {
    pub(super) project_id: Uuid,
    pub(super) app_id: Uuid,
    pub(super) environment: AppEnvironment,
    pub(super) shared: BTreeSet<String>,
    /// The secrets plane's home: `inherit` and `empty` read different paths
    /// for the same app and environment, so they never share an entry.
    pub(super) home: SecretsHome,
}

/// The app's secret rows, and the workspace secrets shared with it, as they
/// stood when an entry was resolved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Stamp {
    /// Newest `updated_at` under `apps/<app_id>/`, active or not.
    pub(super) newest: Option<DateTimeWithTimeZone>,
    pub(super) rows: i64,
    /// Newest of a grant's `created_at` and its secret's `updated_at`: moves
    /// when a secret is shared with the app, and when a shared one is rotated
    /// or deleted (a delete deactivates the row in place).
    pub(super) shared_newest: Option<DateTimeWithTimeZone>,
    /// How many grants the app holds: moves when one is revoked.
    pub(super) shared_rows: i64,
}

struct Entry {
    stamp: Stamp,
    stored_at: Instant,
    env: Arc<ResolvedEnv>,
}

pub(super) struct EnvCache {
    entries: Mutex<LruCache<EnvKey, Entry>>,
    ttl: Duration,
}

impl EnvCache {
    pub(super) fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(LruCache::new(
                NonZeroUsize::new(max_entries).expect("max_entries > 0"),
            )),
            ttl,
        }
    }

    /// The entry under `key`, if it was resolved under `stamp` within the TTL.
    pub(super) fn get(&self, key: &EnvKey, stamp: Stamp) -> Option<Arc<ResolvedEnv>> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = entries.get(key)?;
        if entry.stamp == stamp && entry.stored_at.elapsed() < self.ttl {
            return Some(Arc::clone(&entry.env));
        }
        entries.pop(key);
        None
    }

    pub(super) fn put(&self, key: EnvKey, stamp: Stamp, env: Arc<ResolvedEnv>) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .put(
                key,
                Entry {
                    stamp,
                    stored_at: Instant::now(),
                    env,
                },
            );
    }

    /// Drop what a write to `environment` of `app_id` may have changed: every
    /// environment's entry for a production write, one environment's otherwise.
    pub(super) fn invalidate(&self, app_id: Uuid, environment: &AppEnvironment) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let stale: Vec<EnvKey> = entries
            .iter()
            .filter(|(key, _)| {
                key.app_id == app_id
                    && (*environment == AppEnvironment::Production
                        || key.environment == *environment)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            entries.pop(&key);
        }
    }
}

fn global() -> &'static EnvCache {
    static CACHE: OnceLock<EnvCache> = OnceLock::new();
    CACHE.get_or_init(|| EnvCache::new(TTL, MAX_ENTRIES))
}

/// A secret of `environment` of `app_id` was set or deleted: the next call
/// resolves `ctx.env` afresh here, whatever the stamp says.
pub(crate) fn invalidate(app_id: Uuid, environment: &AppEnvironment) {
    global().invalidate(app_id, environment);
}

/// One round trip for a [`Stamp`]: the app's own rows, active or not, and the
/// grants that share a workspace secret with it, joined to that secret.
const STAMP_SQL: &str = r#"
SELECT
    (SELECT max(updated_at) FROM secrets WHERE project_id = $1 AND starts_with(name, $2)) AS newest,
    (SELECT count(*) FROM secrets WHERE project_id = $1 AND starts_with(name, $2)) AS rows,
    (SELECT max(GREATEST(g.created_at, s.updated_at))
       FROM app_secret_grants g JOIN secrets s ON s.id = g.secret_id
      WHERE g.app_id = $3) AS shared_newest,
    (SELECT count(*) FROM app_secret_grants WHERE app_id = $3) AS shared_rows
"#;

/// The app's current [`Stamp`]; `None`, logged, when it cannot be read.
async fn stamp_of(db: &DatabaseConnection, project_id: Uuid, app_id: Uuid) -> Option<Stamp> {
    match read_stamp(db, project_id, app_id).await {
        Ok(stamp) => Some(stamp),
        Err(e) => {
            error!("ctx.env: secrets stamp read failed for app {app_id}: {e}");
            None
        }
    }
}

async fn read_stamp(
    db: &DatabaseConnection,
    project_id: Uuid,
    app_id: Uuid,
) -> Result<Stamp, sea_orm::DbErr> {
    let prefix = scope::secret_prefix(app_id, &AppEnvironment::Production);
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            STAMP_SQL,
            [project_id.into(), prefix.into(), app_id.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(Stamp::default());
    };
    Ok(Stamp {
        newest: row.try_get("", "newest")?,
        rows: row.try_get("", "rows")?,
        shared_newest: row.try_get("", "shared_newest")?,
        shared_rows: row.try_get("", "shared_rows")?,
    })
}

/// Where the policy reads `ctx.env` from: the chain, unless its planes say
/// the environment's own path alone.
fn secrets_home(policy: &EnvPolicy) -> SecretsHome {
    policy
        .homes()
        .map_or(SecretsHome::Chain, |homes| homes.secrets)
}

/// Resolve `ctx.env` for an invocation: secrets stored under the
/// `apps/<app_id>/<KEY>` prefix in the project's secret manager, exposed to
/// the isolate with the prefix stripped (so the function sees plain `KEY`).
///
/// Outside production the environment's own path, `apps/<app_id>/<env>/<KEY>`,
/// is read instead, falling back to production's value only for the
/// manifest's `shared` keys (`custom_apps_secrets::scope`); production reads
/// its top-level keys only, never an environment's.
///
/// Then the workspace secrets a workspace admin shared with the app, each
/// under its own name, for a key the app holds no value of
/// (`secret_grants::plan_granted`): every one in production, a `shared` key's
/// elsewhere.
///
/// Served from the cache when the app's secrets have not moved since the
/// entry was resolved (module docs); resolved and remembered otherwise.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
pub(super) async fn resolve(
    db: &DatabaseConnection,
    project_id: Uuid,
    app_id: Uuid,
    policy: &EnvPolicy,
) -> ResolvedEnv {
    let key = EnvKey {
        project_id,
        app_id,
        environment: policy.environment().clone(),
        shared: policy.shared_env().clone(),
        home: secrets_home(policy),
    };
    let stamp = stamp_of(db, project_id, app_id).await;
    if let Some(stamp) = stamp
        && let Some(env) = global().get(&key, stamp)
    {
        return (*env).clone();
    }
    let (env, complete) = resolve_uncached(db, project_id, app_id, policy).await;
    if let Some(stamp) = stamp
        && complete
    {
        global().put(key, stamp, Arc::new(env.clone()));
    }
    env
}

/// The resolution itself, and whether every planned key was read — only a
/// complete one is worth remembering.
async fn resolve_uncached(
    db: &DatabaseConnection,
    project_id: Uuid,
    app_id: Uuid,
    policy: &EnvPolicy,
) -> (ResolvedEnv, bool) {
    use oxy::service::secret_manager::SecretManagerService;

    let secret_manager = SecretManagerService::new(project_id);
    let secrets = match secret_manager.list_secrets(db).await {
        Ok(secrets) => secrets,
        Err(e) => {
            error!("ctx.env: failed to list secrets for app {app_id}: {e}");
            return (ResolvedEnv::default(), false);
        }
    };
    let (shared, mut complete) = shared_secret_names(db, app_id, &secrets).await;
    let names: Vec<String> = secrets.into_iter().map(|s| s.name).collect();
    let lookup = secrets_home(policy).lookup();
    let plan = scope::plan_env(
        app_id,
        policy.environment(),
        lookup,
        policy.shared_env(),
        &names,
    );
    let granted = secret_grants::plan_granted(
        &plan,
        policy.environment(),
        lookup,
        policy.shared_env(),
        &shared,
    );

    let mut resolved = ResolvedEnv::default();
    let own = plan.into_iter().map(|entry| (entry, false));
    for (entry, from_grant) in own.chain(granted.into_iter().map(|entry| (entry, true))) {
        match secret_manager.get_secret(&entry.stored_name).await {
            Some(value) => {
                if entry.through_fallback {
                    resolved.read_through_fallback.insert(entry.key.clone());
                }
                if from_grant {
                    resolved.shared_secrets.insert(entry.key.clone());
                }
                resolved.values.insert(entry.key, value);
            }
            None => {
                error!("ctx.env: failed to resolve secret '{}'", entry.stored_name);
                complete = false;
            }
        }
    }
    (resolved, complete)
}

/// The names of the active workspace secrets shared with `app_id`, out of the
/// project's `secrets` already listed — so a grant can only ever name a secret
/// of the app's own workspace. `false` when the grants could not be read: the
/// run goes ahead on the app's own keys and the result is not remembered.
async fn shared_secret_names(
    db: &DatabaseConnection,
    app_id: Uuid,
    secrets: &[SecretInfo],
) -> (BTreeSet<String>, bool) {
    match secret_grants::shared_with_app(db, app_id).await {
        Ok(ids) => (
            secrets
                .iter()
                .filter(|s| ids.contains(&s.id) && secret_grants::is_shareable(&s.name))
                .map(|s| s.name.clone())
                .collect(),
            true,
        ),
        Err(e) => {
            error!("ctx.env: failed to read the secrets shared with app {app_id}: {e}");
            (BTreeSet::new(), false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(app: u128, environment: AppEnvironment) -> EnvKey {
        EnvKey {
            project_id: Uuid::from_u128(1),
            app_id: Uuid::from_u128(app),
            environment,
            shared: BTreeSet::new(),
            home: SecretsHome::Chain,
        }
    }

    fn stamp(seq: i64) -> Stamp {
        let newest =
            chrono::DateTime::from_timestamp(1_700_000_000 + seq, 0).map(|t| t.fixed_offset());
        Stamp {
            newest,
            rows: 1,
            ..Stamp::default()
        }
    }

    fn env(value: &str) -> Arc<ResolvedEnv> {
        Arc::new(ResolvedEnv {
            values: BTreeMap::from([("API_KEY".to_string(), value.to_string())]),
            read_through_fallback: BTreeSet::new(),
            shared_secrets: BTreeSet::new(),
        })
    }

    fn staging() -> AppEnvironment {
        AppEnvironment::Staging
    }

    /// The same app and environment resolve differently under `empty`, so an
    /// entry cached under one home is never served for the other.
    #[test]
    fn inherit_and_empty_never_share_a_cache_entry() {
        let cache = EnvCache::new(Duration::from_secs(60), 16);
        let inherit = key(1, staging());
        let empty = EnvKey {
            home: SecretsHome::OwnOnly,
            ..inherit.clone()
        };
        cache.put(inherit.clone(), stamp(1), env("inherited"));
        assert!(cache.get(&empty, stamp(1)).is_none());
        cache.put(empty.clone(), stamp(1), env("own"));
        assert_eq!(
            cache.get(&inherit, stamp(1)).expect("hit").values["API_KEY"],
            "inherited"
        );
        assert_eq!(
            cache.get(&empty, stamp(1)).expect("hit").values["API_KEY"],
            "own"
        );
    }

    #[test]
    fn fallback_values_are_the_values_of_the_keys_read_through_the_fallback() {
        let resolved = ResolvedEnv {
            values: BTreeMap::from([
                ("OWN".to_string(), "own-value".to_string()),
                ("SHARED".to_string(), "prod-value".to_string()),
            ]),
            read_through_fallback: BTreeSet::from(["SHARED".to_string(), "GONE".to_string()]),
            shared_secrets: BTreeSet::new(),
        };
        assert_eq!(resolved.fallback_values(), vec!["prod-value".to_string()]);
        assert!(ResolvedEnv::default().fallback_values().is_empty());
    }

    #[test]
    fn an_entry_resolved_under_the_current_stamp_is_a_hit() {
        let cache = EnvCache::new(Duration::from_secs(60), 16);
        cache.put(key(1, staging()), stamp(1), env("v1"));
        let hit = cache.get(&key(1, staging()), stamp(1)).expect("hit");
        assert_eq!(hit.values["API_KEY"], "v1");
    }

    /// A rotation on another replica, or by a function's own
    /// `ctx.secrets.set`: no invalidation ran here, but the rows moved.
    #[test]
    fn a_moved_stamp_misses_even_within_the_ttl() {
        let cache = EnvCache::new(Duration::from_secs(60), 16);
        cache.put(key(1, staging()), stamp(1), env("v1"));
        assert!(cache.get(&key(1, staging()), stamp(2)).is_none());
        // A delete deactivates in place (same `updated_at` semantics), but a
        // changed row count is a changed stamp too.
        cache.put(key(1, staging()), stamp(1), env("v1"));
        let fewer = Stamp {
            rows: 0,
            ..stamp(1)
        };
        assert!(cache.get(&key(1, staging()), fewer).is_none());
    }

    /// Sharing, unsharing or rotating a workspace secret shared with the app
    /// moves only the grant half of the stamp; that is a miss too, so the
    /// next call reads the new value on every replica.
    #[test]
    fn a_moved_shared_half_of_the_stamp_misses() {
        let cache = EnvCache::new(Duration::from_secs(60), 16);
        cache.put(key(1, AppEnvironment::Production), stamp(1), env("v1"));
        let shared = Stamp {
            shared_rows: 1,
            ..stamp(1)
        };
        assert!(
            cache
                .get(&key(1, AppEnvironment::Production), shared)
                .is_none()
        );
        cache.put(key(1, AppEnvironment::Production), shared, env("v1"));
        let rotated = Stamp {
            shared_newest: stamp(2).newest,
            ..shared
        };
        assert!(
            cache
                .get(&key(1, AppEnvironment::Production), rotated)
                .is_none()
        );
    }

    /// The write path on this replica: visible to the next call at once,
    /// before any stamp is compared.
    #[test]
    fn a_production_write_drops_every_environment_of_the_app_and_no_other_app() {
        let cache = EnvCache::new(Duration::from_secs(60), 16);
        let sandbox = AppEnvironment::Dev {
            handle: "a1".into(),
        };
        cache.put(key(1, AppEnvironment::Production), stamp(1), env("v1"));
        cache.put(key(1, staging()), stamp(1), env("v1"));
        cache.put(key(1, sandbox.clone()), stamp(1), env("v1"));
        cache.put(key(2, AppEnvironment::Production), stamp(1), env("other"));

        cache.invalidate(Uuid::from_u128(1), &AppEnvironment::Production);

        assert!(
            cache
                .get(&key(1, AppEnvironment::Production), stamp(1))
                .is_none()
        );
        assert!(
            cache.get(&key(1, staging()), stamp(1)).is_none(),
            "staging reads production through the shared fallback"
        );
        assert!(cache.get(&key(1, sandbox), stamp(1)).is_none());
        assert!(
            cache
                .get(&key(2, AppEnvironment::Production), stamp(1))
                .is_some(),
            "another app's entry is untouched"
        );
    }

    #[test]
    fn an_environments_write_drops_that_environment_alone() {
        let cache = EnvCache::new(Duration::from_secs(60), 16);
        cache.put(key(1, AppEnvironment::Production), stamp(1), env("v1"));
        cache.put(key(1, staging()), stamp(1), env("v1"));

        cache.invalidate(Uuid::from_u128(1), &staging());

        assert!(cache.get(&key(1, staging()), stamp(1)).is_none());
        assert!(
            cache
                .get(&key(1, AppEnvironment::Production), stamp(1))
                .is_some(),
            "production never reads a staging key"
        );
    }

    /// The backstop: a stamp that did not move still expires.
    #[test]
    fn an_entry_past_the_ttl_misses_under_the_same_stamp() {
        let cache = EnvCache::new(Duration::from_millis(0), 16);
        cache.put(key(1, staging()), stamp(1), env("v1"));
        assert!(cache.get(&key(1, staging()), stamp(1)).is_none());
    }

    #[test]
    fn the_shared_keys_are_part_of_the_key() {
        let cache = EnvCache::new(Duration::from_secs(60), 16);
        cache.put(key(1, staging()), stamp(1), env("v1"));
        let mut other_build = key(1, staging());
        other_build.shared.insert("API_KEY".into());
        assert!(
            cache.get(&other_build, stamp(1)).is_none(),
            "a build sharing a different key set resolves its own env"
        );
    }
}
