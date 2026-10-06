//! The limit of 20 sandboxes per app (`MAX_SANDBOXES_PER_APP`), through the
//! management routes and under a race: one being torn down counts, and every
//! create takes the app row's lock before it counts.

use axum::http::StatusCode;
use oxy_app::server::api::custom_apps_sandboxes::{MAX_SANDBOXES_PER_APP, SandboxError, ops};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};
use serde_json::json;

use crate::app_environments::seed_sandbox;
use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_routes::{sandbox_rows, send, staffed_app};

/// An app has at most 20 sandboxes, and one still being torn down counts:
/// the 20th is created, the 21st is `409 environment_limit`.
#[tokio::test]
async fn the_21st_sandbox_is_refused_and_one_being_deleted_counts() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    assert_eq!(MAX_SANDBOXES_PER_APP, 20);
    for i in 0..19 {
        seed_sandbox(&t.db, app, &format!("dev-s{i}"), t.guest_id).await;
    }
    t.db.execute_unprepared(
        "UPDATE app_environments SET deleting_at = now() WHERE name = 'dev-s0'",
    )
    .await
    .expect("mark one deleting");

    let environments = format!("{app}/environments");
    let (status, twentieth) =
        send("POST", &environments, Some(json!({ "name": "dev-twenty" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{twentieth}");
    let (status, refused) = send("POST", &environments, Some(json!({ "name": "dev-over" }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "environment_limit");
    assert_eq!(sandbox_rows(&t.db, app).await.len(), 20);
}

/// Creates racing for the last free place: every create takes the app row's
/// lock before it counts, so exactly one wins and the limit holds.
///
/// The test holds that lock itself while six creates start, and waits until
/// all six are blocked on it — a create that did not take the lock would run
/// to completion instead, which is what the wait would see. Released, they
/// run one after another: one is created, five are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_cannot_pass_the_limit() {
    use sea_orm::TransactionTrait;
    let t = seeded_tenant().await;
    let app_id = staffed_app(&t).await;
    for i in 0..19 {
        seed_sandbox(&t.db, app_id, &format!("dev-s{i}"), t.guest_id).await;
    }
    let app = entity::apps::Entity::find_by_id(app_id)
        .one(&t.db)
        .await
        .expect("read")
        .expect("app");
    // A connection each: racers sharing one pooled connection would run one
    // after another and prove nothing.
    let url = std::env::var("OXY_DATABASE_URL").expect("test_db points the process at its db");
    let holder = sea_orm::Database::connect(&url).await.expect("connect");
    let held = holder.begin().await.expect("begin");
    held.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM apps WHERE id = $1 FOR UPDATE",
        [app_id.into()],
    ))
    .await
    .expect("hold the app row's lock");
    let mut racers = Vec::new();
    for i in 0..6 {
        let db = sea_orm::Database::connect(&url).await.expect("connect");
        let (app, owner) = (app.clone(), t.guest());
        racers.push(tokio::spawn(async move {
            let environment = AppEnvironment::parse(&format!("dev-race{i}")).unwrap();
            ops::create(&db, &app, &environment, &owner).await
        }));
    }
    // Until every create waits on the lock — or, had one not taken it, until
    // they have all finished without it.
    let waiting_on_a_lock = "SELECT count(*) AS n FROM pg_stat_activity \
         WHERE datname = current_database() AND wait_event_type = 'Lock'";
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let waiting = loop {
        let waiting: i64 =
            t.db.query_one_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                waiting_on_a_lock,
            ))
            .await
            .expect("read pg_stat_activity")
            .expect("a row")
            .try_get("", "n")
            .expect("n");
        if waiting >= 6 || racers.iter().all(|racer| racer.is_finished()) {
            break waiting;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "only {waiting} of 6 creates reached the app row's lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(
        waiting, 6,
        "every create must wait for the app row's lock before it counts"
    );
    assert_eq!(
        sandbox_rows(&t.db, app_id).await.len(),
        19,
        "none created yet"
    );
    held.commit().await.expect("release the lock");

    let results: Vec<_> = futures::future::join_all(racers)
        .await
        .into_iter()
        .map(|joined| joined.expect("a racer panicked"))
        .collect();
    let created = results.iter().filter(|r| r.is_ok()).count();
    let refused = results
        .iter()
        .filter(|r| matches!(r, Err(SandboxError::Limit(20))))
        .count();
    assert_eq!((created, refused), (1, 5), "{results:?}");
    assert_eq!(sandbox_rows(&t.db, app_id).await.len(), 20);
}
