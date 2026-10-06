//! The sandbox maintenance sweep (`custom_apps_sandboxes::maintenance`): it
//! expires a sandbox idle past its TTL and spares one used since — selecting
//! exactly what the API's `expires_at` says has passed — and queues again a
//! teardown stuck for six hours, never one still on its way.

use chrono::{DateTime, Duration, Utc};
use entity::apps;
use oxy_app::server::api::custom_apps_sandboxes::maintenance::{stale_teardown, sweep};
use oxy_app::server::api::custom_apps_sandboxes::teardown::run_id_of;
use oxy_app::server::api::custom_apps_sandboxes::{TeardownReason, activity, idle_ttl, ops};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, publish_app, seeded_tenant};
use crate::sandbox_teardown::sandbox;

/// A published app with the sandboxes `handles`, and its build.
pub(crate) async fn app_with(t: &Tenant, slug: &str, handles: &[&str]) -> (apps::Model, Uuid) {
    let noop = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    let app_id = publish_app(t, slug, demo_workspace_id(), &[noop])
        .await
        .app_id;
    let app = apps::Entity::find_by_id(app_id)
        .one(&t.db)
        .await
        .expect("read the app")
        .expect("the app");
    let build = entity::app_builds::Entity::find()
        .filter(entity::app_builds::Column::AppId.eq(app_id))
        .one(&t.db)
        .await
        .expect("read the build")
        .expect("the build")
        .id;
    for handle in handles {
        ops::create(&t.db, &app, &sandbox(handle), &t.guest())
            .await
            .expect("create the sandbox");
    }
    (app, build)
}

/// Backdate a sandbox's `column` (`updated_at` or `deleting_at`) to `at`.
pub(crate) async fn set(
    db: &DatabaseConnection,
    app: Uuid,
    name: &str,
    column: &str,
    at: DateTime<Utc>,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("UPDATE app_environments SET {column} = $1 WHERE app_id = $2 AND name = $3"),
        [at.into(), app.into(), name.into()],
    ))
    .await
    .expect("backdate the sandbox");
}

async fn invoked_at(
    db: &DatabaseConnection,
    app: Uuid,
    build: Uuid,
    name: &str,
    at: DateTime<Utc>,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_function_invocations \
           (id, app_id, build_id, function_name, mode, status, environment, created_at) \
         VALUES ($1, $2, $3, 'noop', 'route', 'success', $4, $5)",
        [
            Uuid::new_v4().into(),
            app.into(),
            build.into(),
            name.into(),
            at.into(),
        ],
    ))
    .await
    .expect("record an invocation");
}

/// `(name, is deleting)` of the app's sandboxes, by name.
pub(crate) async fn sandboxes(db: &DatabaseConnection, app: Uuid) -> Vec<(String, bool)> {
    let mut rows = entity::app_environments::Entity::find()
        .filter(entity::app_environments::Column::AppId.eq(app))
        .filter(entity::app_environments::Column::Kind.eq("dev"))
        .all(db)
        .await
        .expect("read the sandboxes");
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows.into_iter()
        .map(|row| (row.name, row.deleting_at.is_some()))
        .collect()
}

/// The queued teardowns of the app: `(sandbox, reason)`, oldest first.
pub(crate) async fn teardowns(db: &DatabaseConnection, app: Uuid) -> Vec<(String, String)> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT spec->'payload'->>'environment' AS environment, \
                spec->'payload'->>'reason' AS reason \
           FROM agentic_task_queue \
          WHERE queue_status = 'queued' AND spec->>'kind' = 'custom_app_sandbox_teardown' \
            AND spec->'payload'->>'app_id' = $1 \
          ORDER BY created_at, task_id",
        [app.to_string().into()],
    ))
    .await
    .expect("read the queue")
    .iter()
    .map(|row| {
        (
            row.try_get("", "environment").expect("environment"),
            row.try_get("", "reason").expect("reason"),
        )
    })
    .collect()
}

/// One pass expires the sandbox idle past the TTL — and only it: one
/// whose last publish is as old but that was invoked yesterday is spared,
/// as is one published to today. The expiry is a delete like any other
/// (marked, pointer cleared, teardown queued), audited once as the
/// system's. What the sweep selects is exactly what the API's
/// `expires_at` says has passed.
#[tokio::test]
async fn the_sweep_expires_an_idle_sandbox_and_spares_one_invoked_recently() {
    let t = seeded_tenant().await;
    let (app, build) = app_with(&t, "sbx-sweep", &["idle", "busy", "fresh"]).await;
    let now = Utc::now();
    let long_ago = now - idle_ttl() - Duration::days(1);
    for name in ["dev-idle", "dev-busy"] {
        set(&t.db, app.id, name, "updated_at", long_ago).await;
    }
    // Both were invoked once, long ago; dev-busy again yesterday.
    invoked_at(
        &t.db,
        app.id,
        build,
        "dev-idle",
        long_ago - Duration::days(3),
    )
    .await;
    invoked_at(
        &t.db,
        app.id,
        build,
        "dev-busy",
        long_ago - Duration::days(3),
    )
    .await;
    invoked_at(&t.db, app.id, build, "dev-busy", now - Duration::days(1)).await;

    // The SQL the sweep selects by and the rule the API shows agree.
    let idle = activity::idle_sandboxes(&t.db, now, idle_ttl(), 50)
        .await
        .expect("select the idle");
    assert_eq!(idle, vec![(app.id, "dev-idle".to_string())]);
    for shown in ops::list(&t.db, &app, &t.org_slug).await.expect("list") {
        let Some(expires_at) = shown.expires_at else {
            continue;
        };
        assert_eq!(
            expires_at < now,
            shown.name == "dev-idle",
            "{}: expires_at {expires_at}",
            shown.name
        );
    }

    assert_eq!(sweep(&t.db, now).await.expect("sweep"), 1);
    assert_eq!(
        sandboxes(&t.db, app.id).await,
        vec![
            ("dev-busy".to_string(), false),
            ("dev-fresh".to_string(), false),
            ("dev-idle".to_string(), true),
        ]
    );
    assert_eq!(
        teardowns(&t.db, app.id).await,
        vec![("dev-idle".to_string(), "expired".to_string())]
    );
    let audit = entity::audit_events::Entity::find()
        .filter(entity::audit_events::Column::Action.eq("app.environment.deleted"))
        .filter(entity::audit_events::Column::Environment.eq("dev-idle"))
        .all(&t.db)
        .await
        .expect("read the audit log");
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].actor_email, "system:sandbox-expiry");
    assert_eq!(audit[0].actor_type, "system");
    assert_eq!(audit[0].actor_user_id, None);
    assert_eq!(audit[0].reason.as_deref(), Some("expired"));

    // A second replica that selected dev-idle in the same instant deletes
    // it too: its teardown is already on its way, so nothing is queued.
    let again = ops::delete(&t.db, &app, &sandbox("idle"), None, TeardownReason::Expired)
        .await
        .expect("a second replica's delete");
    assert!(!again.queued, "{again:?}");
    assert_eq!(teardowns(&t.db, app.id).await.len(), 1);

    // The next pass finds nothing new: a sandbox just marked is not stale.
    assert_eq!(sweep(&t.db, now).await.expect("sweep again"), 0);
    assert_eq!(teardowns(&t.db, app.id).await.len(), 1);
}

/// A sandbox left `deleting` for six hours with no teardown on its way —
/// its run failed, or was lost — is queued again, and marked now, so the
/// pass after leaves it alone. One marked an hour ago is still its first
/// run's; and one whose run is still waiting for a worker after six hours
/// is not given a second.
#[tokio::test]
async fn the_sweep_requeues_a_teardown_stuck_for_six_hours() {
    use chrono::SubsecRound;
    let t = seeded_tenant().await;
    let (app, _build) = app_with(&t, "sbx-retry", &["stuck", "recent", "waiting"]).await;
    let now = Utc::now();
    let long_ago = (now - stale_teardown() - Duration::hours(1)).trunc_subsecs(6);
    set(&t.db, app.id, "dev-stuck", "deleting_at", long_ago).await;
    set(
        &t.db,
        app.id,
        "dev-recent",
        "deleting_at",
        now - Duration::hours(1),
    )
    .await;
    // dev-waiting was deleted long ago too, and its run is still queued.
    let waiting = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("waiting"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-waiting");
    set(&t.db, app.id, "dev-waiting", "deleting_at", long_ago).await;
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET task_id = $1 WHERE task_id = $2",
        [
            run_id_of(app.id, "dev-waiting", long_ago.timestamp_micros()).into(),
            waiting.into(),
        ],
    ))
    .await
    .expect("backdate the queued run with its row");

    assert_eq!(sweep(&t.db, now).await.expect("sweep"), 1);
    assert_eq!(
        teardowns(&t.db, app.id).await,
        vec![
            ("dev-waiting".to_string(), "deleted".to_string()),
            ("dev-stuck".to_string(), "retried".to_string()),
        ],
        "dev-waiting keeps its one run; dev-stuck gets a new one"
    );
    assert_eq!(
        sandboxes(&t.db, app.id).await,
        vec![
            ("dev-recent".to_string(), true),
            ("dev-stuck".to_string(), true),
            ("dev-waiting".to_string(), true),
        ],
        "all still deleting: only the task removes a row"
    );
    // A retry is not a second deletion: nothing new in the audit log.
    let audited = entity::audit_events::Entity::find()
        .filter(entity::audit_events::Column::Action.eq("app.environment.deleted"))
        .filter(entity::audit_events::Column::Environment.eq("dev-stuck"))
        .all(&t.db)
        .await
        .expect("read the audit log");
    assert!(audited.is_empty(), "{audited:?}");

    assert_eq!(sweep(&t.db, Utc::now()).await.expect("sweep again"), 0);
    assert_eq!(teardowns(&t.db, app.id).await.len(), 2);
}

/// The sweep selects sandboxes and nothing else: staging and production,
/// however long untouched, are never on its list and are never marked.
#[tokio::test]
async fn the_sweep_never_selects_staging_or_production() {
    let t = seeded_tenant().await;
    let (app, _build) = app_with(&t, "sbx-fixed", &["idle"]).await;
    let now = Utc::now();
    let long_ago = now - idle_ttl() - Duration::days(30);
    for name in ["dev-idle", "staging", "production"] {
        set(&t.db, app.id, name, "updated_at", long_ago).await;
    }
    let untouched = "SELECT count(*) AS n FROM app_environments \
         WHERE app_id = $1 AND kind <> 'dev' AND updated_at = $2 AND deleting_at IS NULL";
    let fixed = |db: DatabaseConnection| async move {
        let n: i64 = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                untouched,
                [app.id.into(), long_ago.into()],
            ))
            .await
            .expect("count")
            .expect("a row")
            .try_get("", "n")
            .expect("n");
        n
    };
    assert_eq!(fixed(t.db.clone()).await, 2, "both fixed rows are as old");

    let idle = activity::idle_sandboxes(&t.db, now, idle_ttl(), 50)
        .await
        .expect("select the idle");
    assert_eq!(idle, vec![(app.id, "dev-idle".to_string())]);
    assert_eq!(sweep(&t.db, now).await.expect("sweep"), 1);
    assert_eq!(
        teardowns(&t.db, app.id).await,
        vec![("dev-idle".to_string(), "expired".to_string())]
    );
    assert_eq!(
        fixed(t.db.clone()).await,
        2,
        "neither fixed row was touched"
    );
}
