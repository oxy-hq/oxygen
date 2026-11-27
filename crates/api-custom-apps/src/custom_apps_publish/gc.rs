//! The build GC a publish runs after it lands: every build of the app beyond
//! retention goes, row before bytes, except a build anything still names.

use entity::{app_builds, apps};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use super::{retention, store};

/// How many builds to retain per app **in each retention window**
/// (`custom_apps_sandboxes::retention`). Older builds (not currently pointed
/// at by either channel) are GC'd from the DB and S3 after each publish.
const KEEP_BUILDS: usize = 10;

/// Delete builds beyond retention — the newest `KEEP_BUILDS` builds production
/// served, the newest `KEEP_BUILDS` only a sandbox ever served, and the newest
/// `KEEP_BUILDS` of the rest (`custom_apps_sandboxes::retention`) — never touching a build a channel pointer
/// or an `app_environments` row references. Row before bytes; best-effort on the S3 side.
pub(super) async fn gc_builds(db: &DatabaseConnection, app_id: Uuid, protect: &[Uuid]) {
    let Some(protect) = gc_protected(db, app_id, protect).await else {
        return;
    };
    let Some(beyond) = gc_beyond_retention(db, app_id).await else {
        return;
    };
    for build in beyond {
        if protect.contains(&build.id) {
            continue;
        }
        let (build_pk, build_label) = (build.id, build.build_id.clone());
        // Captured before the row goes: once it is deleted, this log line is the only
        // record of where an orphan from a failed byte delete lives.
        let s3_prefix = build.s3_prefix.clone();
        // Row first. If anything still names this build, the environment FK refuses
        // and the bytes are never touched; a failed byte delete afterwards only
        // leaves an orphan prefix, never a row pointing at deleted bytes.
        if let Err(e) = build.delete(db).await {
            tracing::warn!("gc_builds row delete failed ({build_label}): {e}");
            continue;
        }
        if let Err(e) = store::delete_build(app_id, &build_label).await {
            tracing::warn!("gc_builds S3 delete failed ({build_label}, prefix {s3_prefix}): {e}");
        }
        // The files its promote checks kept (pictures, a page's text) live
        // beside the build, keyed by its row, and go with it: the rows that
        // named them went with the build's (`ON DELETE CASCADE`).
        if let Err(e) = store::check_files::delete_build(app_id, build_pk).await {
            tracing::warn!("gc_builds check files delete failed ({build_label}): {e}");
        }
    }
}

/// Every build GC must leave alone: the caller's, the builds the live channels
/// point at, and the builds environment rows name. `None` — skip GC — when
/// any of them cannot be read: unknown is never taken for unprotected.
async fn gc_protected(
    db: &DatabaseConnection,
    app_id: Uuid,
    protect: &[Uuid],
) -> Option<Vec<Uuid>> {
    // Always protect the builds the live environments point at, regardless of
    // what the caller passed. GC that reaps the currently-served build is a
    // silent outage: the app 404s (`custom_apps_serve/sources.rs`) with no
    // publish having touched the live environment. The environments' rows are
    // read below, so the guarantee can't be lost by a caller forgetting to
    // thread them through; the app row is read only to skip GC for a gone app.
    let mut protect: Vec<Uuid> = protect.to_vec();
    match apps::Entity::find_by_id(app_id).one(db).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            tracing::warn!(
                "gc_builds: app {app_id} not found; skipping GC so a live build can't be reaped"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!("gc_builds: could not load app {app_id} ({e}); skipping GC");
            return None;
        }
    }
    // Environment rows name the builds — production's, staging's and every
    // sandbox's. Same fail-safe as above: unknown means skip GC.
    match crate::custom_apps_environments::protected_build_ids(db, app_id).await {
        Ok(ids) => protect.extend(ids),
        Err(e) => {
            tracing::warn!(
                "gc_builds: could not load environment builds for app {app_id} ({e}); skipping GC"
            );
            return None;
        }
    }
    Some(protect)
}

/// The app's builds beyond retention, newest first. Builds production served,
/// builds only a sandbox ever served, and the drafts left over are each kept
/// in a window of their own (`retention::beyond`), so neither an afternoon of
/// sandbox publishes nor any number of drafts pushes out a build production
/// would roll back to. `None` — skip GC — when the builds or their events
/// cannot be read.
async fn gc_beyond_retention(
    db: &DatabaseConnection,
    app_id: Uuid,
) -> Option<Vec<app_builds::Model>> {
    let builds = match app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .order_by_desc(app_builds::Column::CreatedAt)
        .all(db)
        .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("gc_builds list failed for app {app_id}: {e}");
            return None;
        }
    };
    let newest_first: Vec<Uuid> = builds.iter().map(|build| build.id).collect();
    let beyond = match retention::beyond(db, app_id, &newest_first, KEEP_BUILDS).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                "gc_builds: could not tell an app's builds apart for app {app_id} ({e}); skipping GC"
            );
            return None;
        }
    };
    Some(
        builds
            .into_iter()
            .filter(|build| beyond.contains(&build.id))
            .collect(),
    )
}
