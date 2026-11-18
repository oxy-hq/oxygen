//! Which functions listen to which events, per environment.
//!
//! A function declares `on: "<app>/<event>"` in `oxy-app.json`. The platform
//! keeps one `app_event_subscriptions` row per `(environment, app, function,
//! event type)`, reconciled from each environment's **live** build exactly as
//! function schedules are (`custom_apps_env_schedules::reconcile`): production's
//! at a promote, an environment's at each publish to it, staging's at a move.
//! A draft build registers nothing in production. Rows are what the relay finds
//! listeners by, with one indexed query ([`listeners`]), instead of reading
//! every app's manifest.
//!
//! Each reconcile owns the rows of its own `(app, environment)` and no others,
//! and holds a lock on that pair while it resolves the live build and writes,
//! so concurrent reconciles end with the live build's rows.
//! A row that is kept has `updated_at` set: it is when the row was last
//! reconciled, which the staff endpoints show.
//!
//! A delivery asks the live build itself ([`listener_subscribes`]) when it
//! runs: the listener may have stopped subscribing since the relay read these
//! rows.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::Utc;
use entity::{app_builds, app_event_schemas, app_event_subscriptions as subs, app_functions, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::events::catalog::{EventCatalog, function_subscribes};
use oxy_custom_apps::events::table::{function_table_watches, is_table_event_type};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use super::table_watch;
use crate::custom_apps_env_resolve::resolve_function_environment;
use crate::custom_apps_sandboxes::lock::lock_key;

/// What one function of a build listens to: `(function, event types)` — the
/// qualified `<app>/<event>` types it names and the `table:` type of each
/// table it watches — and the warnings for the `on` that could not be read.
fn wanted(specs: &[(String, Value)]) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    for (name, spec) in specs {
        let read = function_subscribes(spec).and_then(|mut types| {
            types.extend(
                function_table_watches(spec)?
                    .iter()
                    .map(|table| table.event_type()),
            );
            Ok(types)
        });
        match read {
            Ok(types) if !types.is_empty() => {
                let unique: BTreeSet<String> = types.into_iter().collect();
                rows.push((name.clone(), unique.into_iter().collect()));
            }
            Ok(_) => {}
            Err(e) => warnings.push(format!(
                "function '{name}' declares an `on` that could not be read ({e}); it listens to \
                 nothing until a publish fixes it"
            )),
        }
    }
    (rows, warnings)
}

/// The lock of one `(app, environment)`'s subscription rows, held for a
/// reconcile's whole read-resolve-write: the same mechanism as the schedules'
/// (`custom_apps_env_schedules::lock`) under a key of its own, so the two never
/// wait on each other. A transaction-scoped advisory lock on a pooled
/// connection; ending the transaction (or dropping the connection) releases it.
struct Lock {
    txn: sqlx::Transaction<'static, sqlx::Postgres>,
}

impl Lock {
    async fn acquire(
        db: &DatabaseConnection,
        app_id: Uuid,
        environment: &str,
    ) -> Result<Self, DbErr> {
        let failed = |step: &str, e: sqlx::Error| DbErr::Custom(format!("{step}: {e}"));
        let mut txn = db
            .get_postgres_connection_pool()
            .begin()
            .await
            .map_err(|e| failed("open the subscriptions lock", e))?;
        sqlx::query("SET LOCAL lock_timeout = '10s'")
            .execute(&mut *txn)
            .await
            .map_err(|e| failed("bound the subscriptions lock", e))?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(lock_key(app_id, &format!("subscriptions:{environment}")))
            .execute(&mut *txn)
            .await
            .map_err(|e| failed("take the subscriptions lock", e))?;
        Ok(Self { txn })
    }

    async fn release(self) {
        let _ = self.txn.rollback().await;
    }
}

/// Production's reconcile at a promote or a rollback. `moved_to` is the build
/// the move just made live; the rows follow the build production serves when
/// the lock is held, so a reconcile that waited behind a newer move writes the
/// newer build's rows, not its own. Best-effort: a failure is a publish
/// warning, never a failed publish.
pub async fn reconcile_production(
    db: &DatabaseConnection,
    app: &apps::Model,
    moved_to: &app_builds::Model,
) -> Vec<String> {
    match reconcile(db, app, &AppEnvironment::Production, Some(moved_to.id)).await {
        Ok(warnings) => warnings,
        Err(e) => vec![unread_warning("production", &e)],
    }
}

/// An environment's reconcile: the rows the build it serves calls for, none
/// when it serves no build (a sandbox being deleted serves none).
pub async fn reconcile_environment(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Vec<String>, DbErr> {
    reconcile(db, app, environment, None).await
}

/// Under the environment's lock: resolve the build it serves now (`fallback`
/// only when it serves none and the caller names one), then make its rows match.
async fn reconcile(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    fallback: Option<Uuid>,
) -> Result<Vec<String>, DbErr> {
    let name = environment.name();
    let lock = Lock::acquire(db, app.id, &name).await?;
    let done = reconcile_locked(db, app, environment, &name, fallback).await;
    lock.release().await;
    done
}

async fn reconcile_locked(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    name: &str,
    fallback: Option<Uuid>,
) -> Result<Vec<String>, DbErr> {
    let resolved = resolve_function_environment(db, app, environment).await?;
    let build = match resolved.build_id.or(fallback) {
        Some(id) => app_builds::Entity::find_by_id(id).one(db).await?,
        None => None,
    };
    let Some(build) = build else {
        retire(db, app.id, name).await?;
        return Ok(Vec::new());
    };
    let functions = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build.id))
        .all(db)
        .await?;
    let specs: Vec<(String, Value)> = functions
        .into_iter()
        .filter_map(|f| Some((f.name, f.manifest_json?)))
        .collect();
    apply(
        db,
        app,
        name,
        build.id,
        &specs,
        build.manifest_json.as_ref(),
    )
    .await
}

/// Remove every row of `environment` of `app_id`: a sandbox's teardown, which
/// runs whether or not the app row is still there. `Err` fails the teardown's
/// step, so it is retried rather than leaving rows nothing else removes.
pub async fn retire_environment(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<usize, String> {
    let name = environment.name();
    let failed = |e: DbErr| format!("could not retire its event subscriptions: {e}");
    let lock = Lock::acquire(db, app_id, &name).await.map_err(failed)?;
    let done = retire(db, app_id, &name).await;
    lock.release().await;
    done.map_err(failed)
}

async fn retire(db: &DatabaseConnection, app_id: Uuid, environment: &str) -> Result<usize, DbErr> {
    let done = subs::Entity::delete_many()
        .filter(subs::Column::AppId.eq(app_id))
        .filter(subs::Column::Environment.eq(environment))
        .exec(db)
        .await?;
    if let Some(app) = apps::Entity::find_by_id(app_id).one(db).await? {
        table_watch::cursors::reconcile(db, app.org_id, environment).await?;
    }
    Ok(done.rows_affected as usize)
}

/// Make `environment`'s rows of `app` exactly what `specs` declare: insert the
/// missing, refresh the kept (`build_id`, `updated_at`), delete the rest.
/// Called with the environment's lock held, so the final rows are those of the
/// build resolved under it; the upsert on the unique key is a second guard
/// against a duplicate. Warnings: an `on` that cannot be read, and a type no app in the
/// org declares yet; neither stops the row.
async fn apply(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &str,
    build_id: Uuid,
    specs: &[(String, Value)],
    manifest: Option<&Value>,
) -> Result<Vec<String>, DbErr> {
    let (rows, mut warnings) = wanted(specs);
    let mut keep: HashSet<(String, String)> = HashSet::new();
    let now = Utc::now();
    for (function, types) in &rows {
        for event_type in types {
            keep.insert((function.clone(), event_type.clone()));
            let row = subs::ActiveModel {
                id: Set(Uuid::new_v4()),
                org_id: Set(app.org_id),
                workspace_id: Set(app.project_id),
                environment: Set(environment.to_string()),
                app_id: Set(app.id),
                function_name: Set(function.clone()),
                event_type: Set(event_type.clone()),
                build_id: Set(build_id),
                created_at: Set(now.into()),
                updated_at: Set(now.into()),
                // A reconcile never resumes a paused subscription: the
                // upsert below leaves these columns alone.
                paused_at: Set(None),
                paused_reason: Set(None),
                resumed_at: Set(None),
            };
            subs::Entity::insert(row)
                .on_conflict(
                    OnConflict::columns([
                        subs::Column::Environment,
                        subs::Column::AppId,
                        subs::Column::FunctionName,
                        subs::Column::EventType,
                    ])
                    .update_columns([subs::Column::BuildId, subs::Column::UpdatedAt])
                    .to_owned(),
                )
                .exec_without_returning(db)
                .await?;
        }
    }
    let existing = subs::Entity::find()
        .filter(subs::Column::AppId.eq(app.id))
        .filter(subs::Column::Environment.eq(environment))
        .all(db)
        .await?;
    for row in existing {
        if !keep.contains(&(row.function_name.clone(), row.event_type.clone())) {
            subs::Entity::delete_by_id(row.id).exec(db).await?;
        }
    }
    let types: BTreeSet<&String> = rows.iter().flat_map(|(_, t)| t).collect();
    for event_type in types.into_iter().filter(|t| !is_table_event_type(t)) {
        if !declared_in_org(db, app, event_type, manifest).await? {
            warnings.push(format!(
                "`{event_type}` is not declared by any app in this org yet"
            ));
        }
    }
    // The cursor of each table the org's subscriptions in this environment
    // watch: made for a new watch, dropped with the last subscription to it.
    table_watch::cursors::reconcile(db, app.org_id, environment).await?;
    Ok(warnings)
}

/// Whether some app of `app`'s org declares `<slug>/<event>`: this build's own
/// catalogue, or a stored revision of the named app.
async fn declared_in_org(
    db: &DatabaseConnection,
    app: &apps::Model,
    qualified: &str,
    manifest: Option<&Value>,
) -> Result<bool, DbErr> {
    let Some((slug, event)) = qualified.split_once('/') else {
        return Ok(false);
    };
    if slug == app.slug
        && EventCatalog::from_manifest(manifest).is_ok_and(|c| c.events.contains_key(event))
    {
        return Ok(true);
    }
    let Some(owner) = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(app.org_id))
        .filter(apps::Column::Slug.eq(slug))
        .one(db)
        .await?
    else {
        return Ok(false);
    };
    Ok(app_event_schemas::Entity::find()
        .filter(app_event_schemas::Column::AppId.eq(owner.id))
        .filter(app_event_schemas::Column::EventName.eq(event))
        .one(db)
        .await?
        .is_some())
}

fn unread_warning(environment: &str, e: &DbErr) -> String {
    tracing::warn!("app events: {environment}'s subscriptions were not reconciled: {e}");
    format!(
        "{environment}'s event subscriptions could not be reconciled ({e}); its functions keep \
         their previous subscriptions until the next successful publish"
    )
}

/// One function listening to an event in one environment.
#[derive(Debug, Clone)]
pub struct Listener {
    pub app_id: Uuid,
    /// The listener app's workspace: a table event goes only to listeners in
    /// the workspace whose Airhouse the table is in.
    pub workspace_id: Uuid,
    /// The circuit breaker paused this subscription: its deliveries are held.
    pub paused: bool,
    pub app_slug: String,
    pub function_name: String,
    pub environment: String,
    /// The function's manifest entry in the build the subscription follows.
    pub function_manifest: Value,
}

/// The functions of `org_id` listening to `event_type` in `environment`: one
/// indexed read (`org_id, environment, event_type`), then each listener's app
/// and manifest. A subscription whose function or app is gone is left out.
pub async fn listeners(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    event_type: &str,
) -> Result<Vec<Listener>, DbErr> {
    let rows = subs::Entity::find()
        .filter(subs::Column::OrgId.eq(org_id))
        .filter(subs::Column::Environment.eq(environment))
        .filter(subs::Column::EventType.eq(event_type))
        .all(db)
        .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let app_ids: Vec<Uuid> = rows.iter().map(|r| r.app_id).collect();
    let slugs: HashMap<Uuid, String> = apps::Entity::find()
        .filter(apps::Column::Id.is_in(app_ids))
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.id, a.slug))
        .collect();
    let build_ids: Vec<Uuid> = rows.iter().map(|r| r.build_id).collect();
    let manifests: HashMap<(Uuid, String), Value> = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.is_in(build_ids))
        .all(db)
        .await?
        .into_iter()
        .filter_map(|f| Some(((f.build_id, f.name), f.manifest_json?)))
        .collect();
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(Listener {
                app_slug: slugs.get(&r.app_id)?.clone(),
                function_manifest: manifests
                    .get(&(r.build_id, r.function_name.clone()))?
                    .clone(),
                app_id: r.app_id,
                workspace_id: r.workspace_id,
                paused: r.paused_at.is_some(),
                function_name: r.function_name,
                environment: r.environment,
            })
        })
        .collect())
}

/// Whether `function_name` of `app_id`, in the build `environment` serves
/// now, still declares `event_type` in its `on`. No build, no such function or
/// an `on` that cannot be read all listen to nothing.
pub async fn listener_subscribes(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: &AppEnvironment,
    function_name: &str,
    event_type: &str,
) -> Result<bool, DbErr> {
    let Some(app) = apps::Entity::find_by_id(app_id).one(db).await? else {
        return Ok(false);
    };
    let Some(build_id) = resolve_function_environment(db, &app, environment)
        .await?
        .build_id
    else {
        return Ok(false);
    };
    let function = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build_id))
        .filter(app_functions::Column::Name.eq(function_name))
        .one(db)
        .await?;
    Ok(function
        .and_then(|f| f.manifest_json)
        .and_then(|manifest| {
            let mut types = function_subscribes(&manifest).ok()?;
            types.extend(
                function_table_watches(&manifest)
                    .ok()?
                    .iter()
                    .map(|table| table.event_type()),
            );
            Some(types)
        })
        .is_some_and(|types| types.iter().any(|t| t == event_type)))
}
