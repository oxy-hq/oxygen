//! Resolve the pin a non-production data-plane request reads, once the
//! request has named its app and its environment.
//!
//! The request-reading half (`x-oxy-app`, `Referer`, `Host`, `X-Oxy-App-Env`)
//! needs an axum `HeaderMap` and stays in `oxy_app::server::api::
//! custom_apps_staging_pin::request`; it hands the result to
//! [`pin_for_environment`]. Everything here reads the database only.

use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{EnvironmentPin, PinNotReady, effective_pin, environment_pin};

/// How a request named its app, before any database lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRef {
    Id(Uuid),
    Slugs {
        org: String,
        app: String,
    },
    /// `/a/<slug>/` on an org subdomain: the org comes from the host.
    OrgHostSlug {
        org: String,
        app: String,
    },
}

/// The pin for a request that named `app` and the non-production
/// `environment`: `Ok(None)` for every other request. `project_id` is the
/// workspace the request is addressed to: an app published from another
/// workspace is no match.
///
/// The viewer must be allowed to open non-production
/// (`may_open_non_production`, the rule that serves them that environment's
/// HTML). The pin is the **effective** one ([`effective_pin`]): the build
/// **that environment** serves pins its own, else the environment's own pin.
/// A sandbox whose build and environment pin nothing reads the promoted model;
/// it never borrows staging's pin. `Err` when the environment follows a
/// semantic branch with no ready compile: the request is refused, not served
/// another model.
pub async fn pin_for_environment(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    project_id: Uuid,
    app: &AppRef,
    environment: &AppEnvironment,
) -> Result<Option<Uuid>, PinNotReady> {
    let Some(app) = find_app(db, app).await else {
        return Ok(None);
    };
    if app.project_id != project_id || !may_open(db, caller, &app).await {
        return Ok(None);
    }
    let build_pin = match environment_build(db, &app, environment).await {
        Some(build_id) => super::pinned_revision_for(db, build_id).await,
        None => None,
    };
    // A build's own pin wins, so the environment's is not read for it.
    let env_pin = match build_pin {
        Some(_) => EnvironmentPin::Unset,
        None => environment_pin(db, app.id, &environment.name()).await,
    };
    effective_pin(build_pin, &env_pin)
}

async fn may_open(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &entity::apps::Model,
) -> bool {
    crate::non_production::may_open_non_production(db, caller, app).await
}

/// The build `environment` — staging or a sandbox — serves.
async fn environment_build(
    db: &DatabaseConnection,
    app: &entity::apps::Model,
    environment: &AppEnvironment,
) -> Option<Uuid> {
    crate::env_resolve::resolve_environment(db, app, environment)
        .await
        .ok()?
        .build_id
}

pub async fn find_app(db: &DatabaseConnection, r: &AppRef) -> Option<entity::apps::Model> {
    let found = match r {
        AppRef::Id(id) => entity::apps::Entity::find_by_id(*id).one(db).await,
        AppRef::Slugs { org, app } | AppRef::OrgHostSlug { org, app } => {
            let Ok(Some(org)) = entity::organizations::Entity::find()
                .filter(entity::organizations::Column::Slug.eq(org.as_str()))
                .one(db)
                .await
            else {
                return None;
            };
            entity::apps::Entity::find()
                .filter(entity::apps::Column::OrgId.eq(org.id))
                .filter(entity::apps::Column::Slug.eq(app.as_str()))
                .one(db)
                .await
        }
    };
    found.ok().flatten()
}
