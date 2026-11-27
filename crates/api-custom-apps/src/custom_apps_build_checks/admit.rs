//! Who a checks route answers, decided by `oxy-authz` and nothing else.
//!
//! A build's checks are a build's evidence (a check function's error, a page
//! error, a picture of a screen), so two doors exist and no third:
//!
//! - [`reader`], the console's routes (`show`, `rerun`, a kept file, `changes/seen`):
//!   a caller with non-production reach over the app,
//!   `Action::AppNonProduction` (`Caller::has_reach`), never a publish token.
//! - [`runner`] then [`runner_in`], the five runner routes: that same caller,
//!   OR the platform checker on the environment the attempt names
//!   (`oxy_custom_apps::app_check`): `Action::AppCheck` for a staging attempt,
//!   `Action::AppCheckProduction` for a production one. Never a publish token.
//!
//! **A runner route is decided twice**, because the environment is in the
//! request's body: [`runner`] before the body is read (a caller who may drive
//! no attempt of the build learns nothing from the shape of a body), and
//! [`runner_in`] once the body has named the environment. Both must pass.
//!
//! Everyone else is answered the bare not-found an unknown build gets:
//! holding an app id or a build id confirms nothing.

use entity::{app_builds, apps};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::environment::CheckEnvironment;
use super::error::{ChecksError, internal};
use crate::admin::apps::environment_scope::Caller;

/// The build `build` names in `app`: its row id or its `build_id`.
async fn build_of(
    db: &DatabaseConnection,
    app_id: Uuid,
    build: &str,
) -> Result<app_builds::Model, ChecksError> {
    let of_app = app_builds::Entity::find().filter(app_builds::Column::AppId.eq(app_id));
    let found = match Uuid::parse_str(build) {
        Ok(pk) => of_app.filter(app_builds::Column::Id.eq(pk)).one(db).await,
        Err(_) => {
            of_app
                .filter(app_builds::Column::BuildId.eq(build))
                .one(db)
                .await
        }
    };
    found.map_err(internal)?.ok_or(ChecksError::NotFound)
}

async fn app_of(db: &DatabaseConnection, app_id: Uuid) -> Result<apps::Model, ChecksError> {
    apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(internal)?
        .ok_or(ChecksError::NotFound)
}

/// The app and the build `build` names in it, for a caller who may read the
/// app's non-production rows.
pub(crate) async fn reader(
    db: &DatabaseConnection,
    caller: &Caller<'_>,
    app_id: Uuid,
    build: &str,
) -> Result<(apps::Model, app_builds::Model), ChecksError> {
    let app = app_of(db, app_id).await?;
    if !caller.has_reach(db, &app).await {
        return Err(ChecksError::NotFound);
    }
    let build = build_of(db, app_id, build).await?;
    Ok((app, build))
}

/// Whether `caller` is the platform checker on `app` in `environment`. A
/// publish token is never the checker, whoever minted it.
async fn checks(
    db: &DatabaseConnection,
    caller: &Caller<'_>,
    app: &apps::Model,
    environment: CheckEnvironment,
) -> bool {
    if caller.marker.is_some() {
        return false;
    }
    let asked = oxy_server_authz::Caller::from_user(caller.user);
    let environment = environment.app_environment();
    oxy_custom_apps::app_check::may_check(db, &asked, app, &environment).await
}

/// The app and the build `build` names in it, for a caller who may drive
/// some attempt of the build's checks: the platform checker on the app's
/// staging or its production, or a caller with non-production reach over the
/// app. Asked before a runner route reads its body; [`runner_in`] then holds
/// the caller to the environment the request names.
pub(crate) async fn runner(
    db: &DatabaseConnection,
    caller: &Caller<'_>,
    app_id: Uuid,
    build: &str,
) -> Result<(apps::Model, app_builds::Model), ChecksError> {
    let app = app_of(db, app_id).await?;
    let anywhere = checks(db, caller, &app, CheckEnvironment::Staging).await
        || checks(db, caller, &app, CheckEnvironment::Production).await
        || caller.has_reach(db, &app).await;
    if !anywhere {
        return Err(ChecksError::NotFound);
    }
    let build = build_of(db, app_id, build).await?;
    Ok((app, build))
}

/// Whether `caller` may drive an attempt of `app`'s checks in `environment`:
/// the platform checker there (`AppCheck` on staging, `AppCheckProduction` on
/// production), or a caller with non-production reach over the app.
pub(crate) async fn runner_in(
    db: &DatabaseConnection,
    caller: &Caller<'_>,
    app: &apps::Model,
    environment: CheckEnvironment,
) -> Result<(), ChecksError> {
    if checks(db, caller, app, environment).await || caller.has_reach(db, app).await {
        return Ok(());
    }
    Err(ChecksError::NotFound)
}
