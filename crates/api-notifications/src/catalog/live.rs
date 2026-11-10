//! An app's catalogue as its live build declares it.
//!
//! "Live" is the build production's functions run
//! (`EnvironmentBuilds::production_runs`): production's build, or a
//! never-promoted app's staging build. That is the build whose `ctx.notify` calls are
//! being sent, so it is the one that describes them. Read from Oxy's database
//! (`app_builds.manifest_json`) — config, never the bundle or a working copy.

use std::collections::HashMap;

use entity::{app_builds, apps, notification_sends};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use serde_json::Value;
use uuid::Uuid;

use super::parse;
use super::platform::oxygen;
use super::source::{KindInfo, SourceCatalogue};
use oxy_custom_apps::agent_built::prime_marks;
use oxy_custom_apps::env_resolve::{
    EnvironmentBuilds, load_environment_builds, load_environment_builds_batch,
};

/// The catalogue a manifest declares, with the kinds its functions may send.
pub fn from_manifest(manifest: Option<&Value>) -> SourceCatalogue {
    SourceCatalogue::app(parse::catalogue(manifest), parse::declared_kinds(manifest))
}

/// The build production runs for `app`, if any.
pub async fn live_build(db: &DatabaseConnection, app: &apps::Model) -> Result<Option<Uuid>, DbErr> {
    load_environment_builds(db, app)
        .await?
        .production_runs(db)
        .await
}

/// `app`'s live catalogue. An app with no live build has an empty one.
pub async fn load(db: &DatabaseConnection, app: &apps::Model) -> Result<SourceCatalogue, DbErr> {
    let mut all = load_many(db, std::slice::from_ref(app)).await?;
    Ok(all.remove(&app.id).unwrap_or_else(|| from_manifest(None)))
}

/// The live catalogue of each of `apps`, reading every manifest in one query.
/// Each app's live build is `production_runs`' answer; the one read it can
/// make, a never-promoted app's staging mark, is made for all of them first,
/// after one read of every app's environments.
pub async fn load_many(
    db: &DatabaseConnection,
    apps: &[apps::Model],
) -> Result<HashMap<Uuid, SourceCatalogue>, DbErr> {
    let environments = load_environment_builds_batch(db, apps).await?;
    let none = EnvironmentBuilds::default();
    let fallbacks = apps
        .iter()
        .map(|a| environments.get(&a.id).unwrap_or(&none))
        .filter(|builds| builds.production.is_none())
        .filter_map(|builds| builds.staging);
    prime_marks(db, fallbacks).await?;
    let mut build_of: HashMap<Uuid, Uuid> = HashMap::new();
    for app in apps {
        let builds = environments.get(&app.id).unwrap_or(&none);
        if let Some(build) = builds.production_runs(db).await? {
            build_of.insert(build, app.id);
        }
    }
    let manifests: Vec<(Uuid, Option<Value>)> = if build_of.is_empty() {
        Vec::new()
    } else {
        app_builds::Entity::find()
            .select_only()
            .column(app_builds::Column::Id)
            .column(app_builds::Column::ManifestJson)
            .filter(app_builds::Column::Id.is_in(build_of.keys().copied()))
            .into_tuple()
            .all(db)
            .await?
    };
    let mut out: HashMap<Uuid, SourceCatalogue> =
        apps.iter().map(|a| (a.id, from_manifest(None))).collect();
    for (build, manifest) in manifests {
        if let Some(app) = build_of.get(&build) {
            out.insert(*app, from_manifest(manifest.as_ref()));
        }
    }
    Ok(out)
}

/// What a send's kind is, by the catalogue it is read against today: the
/// app's live build, or Oxygen's own. An app that no longer exists has an
/// empty catalogue — its kinds land in `general`.
pub async fn describe_send(
    db: &DatabaseConnection,
    send: &notification_sends::Model,
) -> Result<KindInfo, DbErr> {
    let Some(app_id) = send.app_id else {
        return Ok(oxygen().describe(&send.kind));
    };
    let catalogue = match apps::Entity::find_by_id(app_id).one(db).await? {
        Some(app) => load(db, &app).await?,
        None => from_manifest(None),
    };
    Ok(catalogue.describe(&send.kind))
}
