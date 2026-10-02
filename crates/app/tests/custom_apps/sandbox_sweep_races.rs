//! What the sandbox sweep does when the world moves between its selection and
//! its delete (review round 2, n2). The pass reads a list and then deletes
//! one sandbox at a time, so a later name on the list can change while an
//! earlier one is being handled: published to, or torn down and created
//! again. Under the row's lock the sweep must look again and leave it alone.
//!
//! Each test makes that window real: it holds the row lock of the **first**
//! sandbox on the list, starts a pass, waits until the pass is blocked on
//! that lock, changes the **second**, and lets go.

use chrono::{Duration, Utc};
use oxy_app::server::api::custom_apps_environments::{self as envs, EnvAction};
use oxy_app::server::api::custom_apps_sandboxes::maintenance::{stale_teardown, sweep};
use oxy_app::server::api::custom_apps_sandboxes::{activity, idle_ttl, ops};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement, TransactionTrait};
use uuid::Uuid;

use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_publish_refusals::until_blocked;
use crate::sandbox_sweep::{app_with, sandboxes, set, teardowns};
use crate::sandbox_teardown::sandbox;

/// A transaction of its own holding `name`'s row lock; committing lets go.
async fn hold_row(app: Uuid, name: &str) -> DatabaseTransaction {
    let url = std::env::var("OXY_DATABASE_URL").expect("test_db points the process at its db");
    let holder = sea_orm::Database::connect(&url).await.expect("connect");
    let held = holder.begin().await.expect("begin");
    held.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM app_environments WHERE app_id = $1 AND name = $2 FOR UPDATE",
        [app.into(), name.into()],
    ))
    .await
    .expect("hold the row's lock");
    held
}

/// A publish that lands on an idle sandbox after the sweep selected it, and
/// before the sweep reached it, saves it: only the sandbox still idle when
/// its turn comes is expired.
#[tokio::test]
async fn a_publish_between_the_sweeps_selection_and_its_delete_saves_the_sandbox() {
    let t = seeded_tenant().await;
    let (app, build) = app_with(&t, "sbx-race", &["first", "second"]).await;
    let now = Utc::now();
    let long_ago = now - idle_ttl() - Duration::days(1);
    // Both idle; dev-first the older, so the pass reaches it first.
    set(
        &t.db,
        app.id,
        "dev-first",
        "updated_at",
        long_ago - Duration::hours(1),
    )
    .await;
    set(&t.db, app.id, "dev-second", "updated_at", long_ago).await;
    let idle = activity::idle_sandboxes(&t.db, now, idle_ttl(), 50).await;
    assert_eq!(idle.expect("select the idle").len(), 2, "both are selected");

    let held = hold_row(app.id, "dev-first").await;
    let db = t.db.clone();
    let pass = tokio::spawn(async move { sweep(&db, now).await });
    until_blocked(&t.db, || pass.is_finished()).await;
    envs::record_move(
        &t.db,
        app.id,
        &sandbox("second"),
        Some(build),
        EnvAction::Publish,
        Some(t.guest_id),
    )
    .await
    .expect("publish to dev-second while the sweep waits");
    held.commit().await.expect("let the sweep go on");

    let queued = pass.await.expect("the pass").expect("sweep");
    assert_eq!(
        sandboxes(&t.db, app.id).await,
        vec![
            ("dev-first".to_string(), true),
            ("dev-second".to_string(), false)
        ],
        "dev-second was published to a moment ago and must not be expired"
    );
    assert_eq!(queued, 1);
    assert_eq!(
        teardowns(&t.db, app.id).await,
        vec![("dev-first".to_string(), "expired".to_string())]
    );
}

/// A stale teardown that finishes, and whose name is created again, after the
/// sweep selected it for a retry: the retry must not delete the new sandbox.
#[tokio::test]
async fn a_retry_never_deletes_a_sandbox_created_again_under_the_name() {
    let t = seeded_tenant().await;
    let (app, _build) = app_with(&t, "sbx-retry-race", &["first", "second"]).await;
    let now = Utc::now();
    let long_ago = now - stale_teardown() - Duration::hours(1);
    // Both stuck deleting, with no run on its way; dev-first the older.
    set(
        &t.db,
        app.id,
        "dev-first",
        "deleting_at",
        long_ago - Duration::hours(1),
    )
    .await;
    set(&t.db, app.id, "dev-second", "deleting_at", long_ago).await;

    let held = hold_row(app.id, "dev-first").await;
    let db = t.db.clone();
    let pass = tokio::spawn(async move { sweep(&db, now).await });
    until_blocked(&t.db, || pass.is_finished()).await;
    // dev-second's own teardown finishes, and the name is created again.
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM app_environments WHERE app_id = $1 AND name = 'dev-second'",
        [app.id.into()],
    ))
    .await
    .expect("the stale teardown removes its row");
    ops::create(&t.db, &app, &sandbox("second"), t.guest_id)
        .await
        .expect("create dev-second again");
    held.commit().await.expect("let the sweep go on");

    let queued = pass.await.expect("the pass").expect("sweep");
    assert_eq!(
        sandboxes(&t.db, app.id).await,
        vec![
            ("dev-first".to_string(), true),
            ("dev-second".to_string(), false)
        ],
        "the new dev-second is nobody's to retry"
    );
    assert_eq!(queued, 1);
    assert_eq!(
        teardowns(&t.db, app.id).await,
        vec![("dev-first".to_string(), "retried".to_string())]
    );
}
