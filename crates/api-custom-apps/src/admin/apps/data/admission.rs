//! Who may read an app's own data, and in which environment.
//!
//! The strictest existing read of customer data in this crate is the live
//! tail of an app's function output (`custom_apps_tail::admission`, the same
//! gate `/logs` puts on every environment), and this is that gate, plus a
//! lockdown check. The console's layers have already run (owner-or-app-admin,
//! `Action::PlatformApps`, the grant's app scope, no active assume session);
//! then, in order:
//!
//! 1. **a publish token** is refused — before the database is reached;
//! 2. **a sandbox agent token** is refused outright. The tail admits one to a
//!    sandbox it created; reading business records is not part of the
//!    sandbox loop, so this read is stricter there;
//! 3. **a lockdown** refuses it: the app's workspace, or any workspace of its
//!    org, having locked Oxy staff out (`workspace_oxy_lockdown`). The org's
//!    clause is because the OLTP database is the org's, shared by every
//!    workspace in it. `Ring::AppAdmin` does not read lockdowns — staff reach
//!    an app's privileged surface through `manage_apps` alone — so without
//!    this an org that locked us out would still have its rows read;
//! 4. **every environment** needs the caller to administer the app
//!    (`resolve_app_role` = admin, oxy-authz `Ring::AppAdmin`);
//! 5. **staging or a sandbox** also needs `may_open_non_production`
//!    (`Action::AppNonProduction`), and a sandbox must exist.
//!
//! [`decide`] is the rule over facts already read, so every refusal is tested
//! without a database; [`admit`] reads the facts.

use entity::{app_members, apps, workspace_oxy_lockdown, workspaces};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use oxy_server_authz::Caller;
use sea_orm::sea_query::Query;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::super::environment_scope::{self, ScopeError};
use super::error::{self, Locked};
use crate::custom_apps_auth::{is_oxy_locked_down, resolve_app_role};
use crate::custom_apps_env_resolve::{may_open_non_production, sandbox_row};

/// A request the gate let through.
pub struct Admitted {
    pub db: DatabaseConnection,
    pub app: apps::Model,
    pub environment: AppEnvironment,
}

/// The facts the rule reads, each from its own source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    /// The app's own workspace has locked Oxy staff out.
    pub workspace_locked: bool,
    /// Some workspace of the app's org has.
    pub org_locked: bool,
    /// `Ring::AppAdmin` over the app.
    pub app_admin: bool,
    /// `Action::AppNonProduction` over the app. Read only outside production.
    pub non_production_reach: bool,
    /// The sandbox the environment names exists and is not being torn down.
    /// Read only for a sandbox.
    pub sandbox_exists: bool,
}

/// The credential half, decided before anything is read.
pub fn refuse_credential(publish_token: bool, sandbox_agent: bool) -> Result<(), ScopeError> {
    if publish_token {
        return Err(error::publish_token_refused());
    }
    if sandbox_agent {
        return Err(error::sandbox_token_refused());
    }
    Ok(())
}

/// May a caller with `facts` read `environment`'s data? See the module docs
/// for the order, which is the order of the checks here.
pub fn decide(environment: &AppEnvironment, facts: &Facts) -> Result<(), ScopeError> {
    if facts.workspace_locked {
        return Err(error::locked_out(Locked::Workspace));
    }
    if facts.org_locked {
        return Err(error::locked_out(Locked::Org));
    }
    if !facts.app_admin {
        return Err(error::app_admin_required());
    }
    match environment {
        AppEnvironment::Production => Ok(()),
        _ if !facts.non_production_reach => Err(error::non_production_refused(&environment.name())),
        AppEnvironment::Dev { .. } if !facts.sandbox_exists => {
            Err(error::environment_not_found(&environment.name()))
        }
        _ => Ok(()),
    }
}

/// Decide whether `user` may read app `app_id`'s data in the environment
/// `raw` names (production when absent). `publish_token` is whether the
/// request authenticated with one.
pub async fn admit(
    user: &AuthenticatedUser,
    publish_token: bool,
    app_id: Uuid,
    raw: Option<&str>,
) -> Result<Admitted, ScopeError> {
    // Credential-aware: a token that carries no staff standing is no staff.
    let caller = Caller::from_user(user);
    refuse_credential(publish_token, caller.is_sandbox_agent())?;
    let environment = environment_scope::parse(raw)?;
    let db = environment_scope::connect().await?;
    let app = environment_scope::load_app(&db, app_id).await?;
    let facts = read_facts(&db, &caller, &app, &environment)
        .await
        .map_err(|e| ScopeError::internal("app data admission", e))?;
    decide(&environment, &facts)?;
    Ok(Admitted {
        db,
        app,
        environment,
    })
}

async fn read_facts(
    db: &DatabaseConnection,
    caller: &Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Facts, DbErr> {
    let role = resolve_app_role(db, caller, app).await?;
    let production = *environment == AppEnvironment::Production;
    Ok(Facts {
        workspace_locked: is_oxy_locked_down(db, app.project_id).await?,
        org_locked: org_locked_down(db, app.org_id).await?,
        app_admin: role == Some(app_members::ROLE_ADMIN),
        non_production_reach: !production && may_open_non_production(db, caller, app).await,
        sandbox_exists: sandbox_row(db, app.id, environment).await?.is_some(),
    })
}

/// Whether any workspace of `org_id` has locked Oxy staff out. One query: a
/// lockdown row whose workspace belongs to the org.
async fn org_locked_down(db: &DatabaseConnection, org_id: Uuid) -> Result<bool, DbErr> {
    let in_org = Query::select()
        .column(workspaces::Column::Id)
        .from(workspaces::Entity)
        .and_where(workspaces::Column::OrgId.eq(org_id))
        .to_owned();
    Ok(workspace_oxy_lockdown::Entity::find()
        .filter(workspace_oxy_lockdown::Column::WorkspaceId.in_subquery(in_org))
        .one(db)
        .await?
        .is_some())
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;
