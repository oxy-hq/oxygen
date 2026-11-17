//! Which OLTP store an environment's data is in — decided exactly as a
//! function run's `ctx.oltp` home is (`environment_gate::with_planes` then
//! `with_oltp_home`), and connected through the same resolver the host uses
//! (`host::writer_connection`). So the browser reads what the app's own code
//! in that environment would read, and nothing else:
//!
//! - **production** — the app's writer schema on the org's database;
//! - **staging** — the app's writer schema on the org's staging branch;
//! - **a sandbox** — its own schema on the staging branch, or its own
//!   branch, as its planes say.
//!
//! The schema is derived from the app's slug (`app_writer_name`), never from
//! a request. Where an environment has no store of its own the answer is
//! [`Store::Absent`] with the reason, not production's data under another
//! name: a staging run with no staging branch reads production read-only,
//! and showing production's rows labelled "staging" would mislead.

use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::resolver::{ResolveError, WriterConnection, resolve_writer_connection_for_org};
use oxy_oltp::schema::{WriterRef, app_writer_name};
use sea_orm::DatabaseConnection;
use serde::Serialize;

use super::super::environment_scope::ScopeError;
use crate::custom_apps_env_resolve::ResolvedEnvironment;
use crate::custom_apps_functions::env_policy::{EnvPolicy, OltpHome};
use crate::custom_apps_functions::environment_gate::{
    Admission, GateError, with_oltp_home, with_planes,
};
use crate::custom_apps_functions::host::writer_connection;

/// Where an environment's data is.
#[derive(Debug)]
pub enum Store {
    /// A connection as the app's writer, its `schema` the app's (or the
    /// sandbox's) own.
    Ready(WriterConnection),
    Absent(Absent),
}

/// Why an environment has nothing to browse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Absent {
    pub code: &'static str,
    pub reason: String,
}

fn absent(code: &'static str, reason: impl Into<String>) -> Store {
    Store::Absent(Absent {
        code,
        reason: reason.into(),
    })
}

/// The store `environment` of `app` reads and writes.
pub async fn store_for(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Store, ScopeError> {
    let Some(writer) = app_writer_name(&app.slug) else {
        return Ok(absent(
            "no_schema_name",
            format!("the slug {:?} cannot name an OLTP schema", app.slug),
        ));
    };
    if *environment == AppEnvironment::Production {
        return production(db, app, &writer).await;
    }
    let home = match home_of(db, app, environment).await? {
        Ok(home) => home,
        Err(refused) => return Ok(refused),
    };
    if let Some(why) = unready(&home, environment) {
        return Ok(why);
    }
    Ok(
        match writer_connection(db, app.org_id, &writer, &home).await {
            Ok(connection) => Store::Ready(connection),
            Err(reason) => absent("store_unavailable", reason),
        },
    )
}

/// Production's store: the app's writer on the org's database.
async fn production(
    db: &DatabaseConnection,
    app: &apps::Model,
    writer: &str,
) -> Result<Store, ScopeError> {
    let writer = WriterRef::app(writer)
        .map_err(|e| ScopeError::internal("app data: name the app's writer", e))?;
    match resolve_writer_connection_for_org(db, app.org_id, &writer).await {
        Ok(connection) => Ok(Store::Ready(connection)),
        Err(ResolveError::NotProvisioned(_)) => Ok(absent(
            "org_not_provisioned",
            "this org has no OLTP database, so no app in it has stored anything with ctx.oltp",
        )),
        Err(ResolveError::WriterNotProvisioned { .. }) => Ok(absent(
            "app_not_provisioned",
            "this app has no OLTP store in production: its schema was never provisioned, so it \
             has not stored anything with ctx.oltp",
        )),
        Err(ResolveError::NotActive(_, status)) => Ok(absent(
            "store_not_active",
            format!("the org's OLTP database is not active (status: {status})"),
        )),
        Err(e @ (ResolveError::Db(_) | ResolveError::Crypto(_))) => Err(ScopeError::internal(
            "app data: resolve production's writer",
            e,
        )),
        Err(e) => Ok(absent("store_unavailable", e.to_string())),
    }
}

/// The home a function run in `environment` would be admitted to, or the
/// absence that says why it has none.
async fn home_of(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Result<OltpHome, Store>, ScopeError> {
    let admission = Admission {
        environment: ResolvedEnvironment {
            environment: environment.clone(),
            build_id: None,
        },
        policy: EnvPolicy::for_environment(environment.clone()),
    };
    let admission = match with_planes(db, admission, app).await {
        Ok(admission) => admission,
        Err(GateError::Refused(refused)) => {
            return Ok(Err(absent("planes_unreadable", refused.message())));
        }
        Err(GateError::Db(e)) => return Err(ScopeError::internal("app data: read planes", e)),
    };
    let admission = with_oltp_home(db, admission, app)
        .await
        .map_err(|e| ScopeError::internal("app data: decide the OLTP home", e))?;
    Ok(Ok(admission.policy.oltp_home().clone()))
}

/// The absence a non-production `home` stands for, if it is one.
fn unready(home: &OltpHome, environment: &AppEnvironment) -> Option<Store> {
    match home {
        OltpHome::Production => Some(absent(
            "no_branch",
            format!(
                "{environment} has no OLTP store of its own: the org has no staging branch, so \
                 its functions read production's data read-only and every write is held. Switch \
                 to production to browse that data"
            ),
        )),
        OltpHome::SandboxUnready(why) => Some(absent("sandbox_schema_unready", why.fix())),
        OltpHome::EnvironmentBranchUnready(why) => Some(absent("branch_unready", why.fix())),
        OltpHome::StagingBranch(_)
        | OltpHome::SandboxSchema(_)
        | OltpHome::EnvironmentBranch(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_apps_functions::env_policy::{EnvBranchUnready, SandboxUnready};

    fn code(store: Option<Store>) -> Option<&'static str> {
        match store {
            Some(Store::Absent(absent)) => Some(absent.code),
            _ => None,
        }
    }

    /// Staging with no branch runs on production read-only: that is no store
    /// of staging's own, and it says so rather than serving production's rows.
    #[test]
    fn a_non_production_environment_on_production_has_no_store_of_its_own() {
        let staging = AppEnvironment::Staging;
        assert_eq!(
            code(unready(&OltpHome::Production, &staging)),
            Some("no_branch")
        );
        let Some(Store::Absent(absent)) = unready(&OltpHome::Production, &staging) else {
            panic!("an absence");
        };
        assert!(absent.reason.contains("staging"), "{}", absent.reason);
        assert!(absent.reason.contains("production"), "{}", absent.reason);
    }

    #[test]
    fn an_unready_sandbox_says_what_puts_it_right() {
        let dev = AppEnvironment::parse("dev-ana").expect("a sandbox name");
        let schema = unready(&OltpHome::SandboxUnready(SandboxUnready::Seeding), &dev);
        let Some(Store::Absent(absent)) = schema else {
            panic!("an absence");
        };
        assert_eq!(absent.code, "sandbox_schema_unready");
        assert_eq!(absent.reason, SandboxUnready::Seeding.fix());
        let branch = OltpHome::EnvironmentBranchUnready(EnvBranchUnready::Cutting);
        assert_eq!(code(unready(&branch, &dev)), Some("branch_unready"));
    }

    #[test]
    fn a_branch_home_is_read() {
        let staging = AppEnvironment::Staging;
        let home = OltpHome::StagingBranch("br-1".into());
        assert!(unready(&home, &staging).is_none());
    }
}
