//! From a request to what it would move: the app, the environment that moves,
//! the build it moves to, and what it serves now (environments design §5.5a,
//! rulings 1, 2 and 6).
//!
//! - **Two promote paths, nothing else**: `dev-<handle>` → `staging` and
//!   `staging` → `production`. Any other pair is `400 promote_path_refused`
//!   — nothing promotes *to* a sandbox, and moving production backwards is a
//!   rollback.
//! - **A promote's build** is one the source has served — any
//!   `app_environment_events` row for that environment and build — named by
//!   its publish id or its UUID; without one, the source's current build.
//! - **A rollback's build** is one the environment served before; without
//!   one, the most recent such build that is not its current one (undo: twice
//!   swaps back). A pruned build's events carry no build, so it is never
//!   offered. A sandbox is not rolled back: it is published to again.

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_server_authz::Caller;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use super::blockers::Act;
use super::error::PromoteError;
use crate::custom_apps_build_history as history;
use crate::custom_apps_env_resolve::{may_open_non_production, resolve_environment, sandbox_row};

/// `POST /api/customer-apps/{id}/environments/{to}/promote`.
#[derive(Debug, Default, Deserialize)]
pub struct PromoteRequest {
    /// The source: `staging` (to production) or a `dev-<handle>` (to staging).
    pub from: String,
    /// The publish id or the build UUID; the source's current build if absent.
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub confirm_schedule_removals: Vec<String>,
    /// Ship over production listeners the build breaks (audited).
    #[serde(default, alias = "breakListeners")]
    pub break_listeners: bool,
    /// What the plan said the target serves (`null`: nothing). Absent: the
    /// act is checked against what this request reads.
    #[serde(default, deserialize_with = "present")]
    pub expect_current: Option<Option<Uuid>>,
}

/// `POST /api/customer-apps/{id}/environments/{name}/rollback`.
#[derive(Debug, Default, Deserialize)]
pub struct RollbackRequest {
    /// The publish id or the build UUID; the previous build if absent.
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub confirm_schedule_removals: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    pub expect_current: Option<Option<Uuid>>,
}

/// A field that is present — `null` included — is `Some`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<Uuid>>, D::Error> {
    Option::<Uuid>::deserialize(d).map(Some)
}

/// What a request would move.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub act: Act,
    pub app: apps::Model,
    /// A promote's source; `None` for a rollback.
    pub from: Option<AppEnvironment>,
    /// The environment whose pointer moves.
    pub to: AppEnvironment,
    pub build: app_builds::Model,
    /// What `to` serves now.
    pub current: Option<Uuid>,
}

pub fn parse_environment(name: &str) -> Result<AppEnvironment, PromoteError> {
    AppEnvironment::parse(name).ok_or_else(|| PromoteError::InvalidEnvironment(name.to_string()))
}

/// The two promote paths (ruling 1).
pub fn check_path(from: &AppEnvironment, to: &AppEnvironment) -> Result<(), PromoteError> {
    match (from, to) {
        (AppEnvironment::Dev { .. }, AppEnvironment::Staging)
        | (AppEnvironment::Staging, AppEnvironment::Production) => Ok(()),
        _ => Err(PromoteError::PathRefused {
            from: from.name(),
            to: to.name(),
        }),
    }
}

pub async fn resolve_promote(
    db: &DatabaseConnection,
    caller: &Caller,
    app_id: Uuid,
    to: &str,
    req: &PromoteRequest,
) -> Result<Resolved, PromoteError> {
    let to = parse_environment(to)?;
    let from = parse_environment(&req.from)?;
    check_path(&from, &to)?;
    let app = load_app(db, app_id).await?;
    let source = source_pointer(db, caller, &app, &from).await?;
    let build = match req.build.as_deref() {
        Some(named) => served_build(db, &app, &from, named).await?,
        None => {
            let current = source.ok_or_else(|| PromoteError::NothingToPromote(from.name()))?;
            super::production::load_build(db, app.id, current).await?
        }
    };
    let current = pointer(db, &app, &to).await?;
    Ok(Resolved {
        act: Act::Promote,
        app,
        from: Some(from),
        to,
        build,
        current,
    })
}

pub async fn resolve_rollback(
    db: &DatabaseConnection,
    app_id: Uuid,
    name: &str,
    req: &RollbackRequest,
) -> Result<Resolved, PromoteError> {
    let environment = parse_environment(name)?;
    if matches!(environment, AppEnvironment::Dev { .. }) {
        return Err(PromoteError::NotRollbackable(environment.name()));
    }
    let app = load_app(db, app_id).await?;
    let current = pointer(db, &app, &environment).await?;
    let build = match req.to.as_deref() {
        Some(named) => served_build(db, &app, &environment, named).await?,
        None => {
            let previous = history::previous_build(db, app.id, &environment, current)
                .await
                .map_err(|e| PromoteError::internal("read the environment's history", e))?
                .ok_or_else(|| PromoteError::NothingToRollBackTo(environment.name()))?;
            super::production::load_build(db, app.id, previous).await?
        }
    };
    Ok(Resolved {
        act: Act::Rollback,
        app,
        from: None,
        to: environment,
        build,
        current,
    })
}

async fn load_app(db: &DatabaseConnection, app_id: Uuid) -> Result<apps::Model, PromoteError> {
    apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|e| PromoteError::internal("load the app", e))?
        .ok_or(PromoteError::AppNotFound)
}

/// What `environment` serves now: production and staging from the app's
/// pointers, as every reader resolves them.
async fn pointer(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Option<Uuid>, PromoteError> {
    Ok(resolve_environment(db, app, environment)
        .await
        .map_err(|e| PromoteError::internal("resolve the environment", e))?
        .build_id)
}

/// The source's current build. A sandbox source needs the caller's reach
/// over the app's non-production environments, asked before the sandbox is
/// looked for, and a sandbox that exists and is not being torn down.
async fn source_pointer(
    db: &DatabaseConnection,
    caller: &Caller,
    app: &apps::Model,
    from: &AppEnvironment,
) -> Result<Option<Uuid>, PromoteError> {
    if !matches!(from, AppEnvironment::Dev { .. }) {
        return pointer(db, app, from).await;
    }
    if !may_open_non_production(db, caller, app).await {
        return Err(PromoteError::NonProduction);
    }
    let row = sandbox_row(db, app.id, from)
        .await
        .map_err(|e| PromoteError::internal("read the sandbox", e))?
        .ok_or_else(|| PromoteError::EnvironmentNotFound(from.name()))?;
    Ok(row.build_id)
}

/// The build `named` — a publish id or a UUID — that `environment` has served.
async fn served_build(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    named: &str,
) -> Result<app_builds::Model, PromoteError> {
    let build = find_build(db, app.id, named).await?;
    let served = history::has_served(db, app.id, environment, build.id)
        .await
        .map_err(|e| PromoteError::internal("read the environment's history", e))?;
    if !served {
        return Err(PromoteError::NotServed {
            environment: environment.name(),
            build: build.build_id,
        });
    }
    Ok(build)
}

/// A build of `app_id` by UUID, else by publish id (a publish id may itself
/// parse as a UUID, so the UUID is only tried first).
async fn find_build(
    db: &DatabaseConnection,
    app_id: Uuid,
    named: &str,
) -> Result<app_builds::Model, PromoteError> {
    let by_id = match Uuid::parse_str(named) {
        Ok(id) => app_builds::Entity::find_by_id(id)
            .filter(app_builds::Column::AppId.eq(app_id))
            .one(db)
            .await
            .map_err(|e| PromoteError::internal("look the build up", e))?,
        Err(_) => None,
    };
    if let Some(build) = by_id {
        return Ok(build);
    }
    app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::BuildId.eq(named))
        .one(db)
        .await
        .map_err(|e| PromoteError::internal("look the build up", e))?
        .ok_or_else(|| PromoteError::BuildNotFound(named.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> AppEnvironment {
        AppEnvironment::parse(name).expect("a valid name")
    }

    #[test]
    fn only_a_sandbox_to_staging_and_staging_to_production_are_promote_paths() {
        assert!(check_path(&env("dev-a1"), &env("staging")).is_ok());
        assert!(check_path(&env("staging"), &env("production")).is_ok());
        for (from, to) in [
            ("dev-a1", "production"),
            ("production", "staging"),
            ("production", "production"),
            ("staging", "staging"),
            ("staging", "dev-a1"),
            ("dev-a1", "dev-b2"),
        ] {
            let refused = check_path(&env(from), &env(to)).expect_err(from);
            assert_eq!(refused.code(), "promote_path_refused", "{from} → {to}");
        }
    }

    #[test]
    fn expect_current_tells_null_from_absent() {
        let absent: PromoteRequest =
            serde_json::from_value(serde_json::json!({ "from": "staging" })).expect("parse");
        assert_eq!(absent.expect_current, None);
        let nothing: PromoteRequest = serde_json::from_value(
            serde_json::json!({ "from": "staging", "expect_current": null }),
        )
        .expect("parse");
        assert_eq!(nothing.expect_current, Some(None));
        let id = Uuid::from_u128(7);
        let some: RollbackRequest =
            serde_json::from_value(serde_json::json!({ "expect_current": id })).expect("parse");
        assert_eq!(some.expect_current, Some(Some(id)));
    }

    #[test]
    fn an_unparseable_environment_is_invalid() {
        let refused = parse_environment("prod").expect_err("not a name");
        assert_eq!(refused.code(), "invalid_environment");
    }
}
