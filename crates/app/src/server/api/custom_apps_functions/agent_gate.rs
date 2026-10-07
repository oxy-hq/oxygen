//! A **sandbox agent token** (`oxy_sbx_`) at the function gate (sandbox agent
//! credential design §1 row F1, §3.3).
//!
//! The token calls a function in a sandbox it created itself, of an app it is
//! granted, and nowhere else. `/fn` names its app by slugs, which the route
//! allow-list cannot check, so the binding is made here: [`entrance`] asks
//! oxy-authz about **this app and this environment**
//! (`may_open_environment`), which holds only for a granted app and an own
//! sandbox. `environment_gate::admit` then refuses the token's entrance
//! everywhere else, production included.
//!
//! **Staging, for a token granted it.** The serve tree's fence runs before
//! authentication and can only read the header's shape, so it lets a token
//! name `staging`; whether **this** token holds the app's staging is decided
//! here, twice: the credential's own grant for this app ([`holds_staging`]),
//! and oxy-authz for the minter's reach. A token minted without staging gets
//! the answer it always did.

use chrono::{DateTime, Utc};
use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::environment_gate::Entrance;
use crate::server::api::custom_apps_agent::holds_staging;
use crate::server::api::custom_apps_env_resolve::{ResolvedEnvironment, may_open_environment};
use crate::server::api::custom_apps_sandbox_instance::own_since;
use oxy_server_authz::Caller;

/// The route template a sandbox agent token's `/fn` call is counted under.
pub(crate) const FN_ROUTE: &str = "/customer-apps/{org_slug}/{app_slug}/fn/{name}";

/// The sandbox agent token `caller` authenticated with, if that is its
/// credential: what an invocation row and a held-write row are stamped with.
pub(crate) fn token_of(caller: &Caller) -> Option<Uuid> {
    caller.sandbox_agent().map(|reach| reach.token_id)
}

/// The idempotency key a call is remembered under. A sandbox agent token's
/// is bound to the sandbox it has now: the stored key carries that sandbox's
/// start, so a key spent in an earlier sandbox that had the same name — whose
/// invocation row outlives it, under the same environment name — is neither
/// replayed here nor in the way (`custom_apps_sandbox_instance`). Every
/// other caller's key is its own, unchanged and with no read.
///
/// `None` for a token whose sandbox row is not its own any more: the gate
/// admitted the call a moment ago, and it runs unkeyed rather than under a
/// name that is someone else's.
pub(crate) async fn instance_key(
    db: &sea_orm::DatabaseConnection,
    app: &entity::apps::Model,
    resolved: &ResolvedEnvironment,
    caller: &Caller,
    key: Option<String>,
) -> Result<Option<String>, sea_orm::DbErr> {
    let (Some(key), Some(token)) = (key.as_deref(), token_of(caller)) else {
        return Ok(key);
    };
    let since = own_since(db, app.id, &resolved.environment, token).await?;
    Ok(since.map(|since| bound_key(key, since)))
}

/// `key`, bound to the sandbox that started at `since`.
fn bound_key(key: &str, since: DateTime<Utc>) -> String {
    format!("{key}@{}", since.timestamp_micros())
}

/// The token's entrance to `resolved` of `app`. A `dev-*` sandbox is asked
/// about, and staging only for a token whose own grant names this app's
/// staging. Production is not the token's, whatever a decision about it would
/// say.
pub(crate) async fn entrance(
    db: &sea_orm::DatabaseConnection,
    resolved: &ResolvedEnvironment,
    caller: &Caller,
    app: &entity::apps::Model,
) -> Entrance {
    let environment = &resolved.environment;
    match environment {
        AppEnvironment::Dev { .. } => Entrance::SandboxAgent {
            own_sandbox: may_open_environment(db, caller, app, environment).await,
        },
        AppEnvironment::Staging => Entrance::StagingAgent {
            granted: holds_staging(caller, app)
                && may_open_environment(db, caller, app, environment).await,
        },
        AppEnvironment::Production => Entrance::SandboxAgent { own_sandbox: false },
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    /// The same key in two sandboxes that had the same name is two keys: the
    /// stored key carries the start of the sandbox it was spent in.
    #[test]
    fn a_key_is_bound_to_the_sandbox_it_was_spent_in() {
        let earlier = Utc.with_ymd_and_hms(2026, 10, 6, 9, 0, 0).unwrap();
        let later = earlier + chrono::Duration::microseconds(1);
        assert_eq!(bound_key("retry-1", earlier), bound_key("retry-1", earlier));
        assert_ne!(bound_key("retry-1", earlier), bound_key("retry-1", later));
        assert_ne!(bound_key("retry-1", earlier), "retry-1");
        assert!(bound_key("retry-1", earlier).starts_with("retry-1@"));
    }
}
