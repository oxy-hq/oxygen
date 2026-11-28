//! The demo fixture (`oxy dev seed --fixtures`, `seed_fixture`) against a real
//! Postgres and the real publish, promote and queue code.
//!
//! The seed runs with `compiled: true` directly: `seed_demo` compiles nothing,
//! and the queued runs are only made when the compile succeeded. Nothing
//! executes them here (no worker); the cases count the queue. The last case
//! runs r4's `store` through the real runner, on a compiled Postgres workspace
//! with an `oxymart` table, to pin the seeded failure's fingerprint to the one
//! the runtime records.

use std::collections::HashMap;

use airhouse::LOCAL_ORG_ID;
use chrono::{Duration, Utc};
use entity::{app_builds, app_environment_events, app_environments, app_functions, apps};
// The demo seed with the example apps, `oxy-server`'s since decompose SD.
use oxy_api_custom_apps::seed_fixture::{
    self, Calendar, LIVE, Outcome, QUEUED_RUNS, RELEASES, bundle_releases, fixture_app_id,
    seeded_invocations,
};
use oxy_server::dev::seed::fixture as seed;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Statement, Value,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::{Schema, demo_workspace_id, examples_path, fresh_db, test_db_with};
use crate::custom_app_functions_fixture::{
    FunctionSpec, call_function_in, invocations, publish_app, seeded_tenant,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};

async fn seeded() -> DatabaseConnection {
    let db = test_db_with(Schema::All).await;
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("seed_demo");
    db
}

async fn build(db: &DatabaseConnection) -> seed_fixture::Built {
    build_at(db, Utc::now()).await
}

/// [`build`] at a given seed time. The history's calendar is cut from it, and
/// how many quarter-hour `snapshot` runs a stretch holds depends on that
/// minute, so a case comparing two seeds' rows gives both the same `now`.
async fn build_at(db: &DatabaseConnection, now: chrono::DateTime<Utc>) -> seed_fixture::Built {
    match seed_fixture::seed_at(db, demo_workspace_id(), true, seed::facts(), now)
        .await
        .expect("the fixture seeds")
    {
        Outcome::Built(built) => built,
        other => panic!("expected a built fixture, got {other:?}"),
    }
}

async fn count(db: &DatabaseConnection, sql: &str, values: Vec<Value>) -> i64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await
        .expect("count")
        .expect("one row");
    row.try_get("", "n").expect("n")
}

/// Every row count the fixture writes, keyed by what it counts.
async fn footprint(db: &DatabaseConnection, app: Uuid) -> HashMap<&'static str, i64> {
    let mut out = HashMap::new();
    for (name, table) in [
        ("builds", "app_builds"),
        ("functions", "app_functions"),
        ("events", "app_environment_events"),
        ("invocations", "app_function_invocations"),
        ("vitals", "custom_app_event"),
    ] {
        let sql = format!("SELECT count(*) AS n FROM {table} WHERE app_id = $1");
        out.insert(name, count(db, &sql, vec![app.into()]).await);
    }
    out.insert("queued", queued_runs(db, app).await);
    out.insert("schedules", schedules(db, app).await.len() as i64);
    out
}

async fn queued_runs(db: &DatabaseConnection, app: Uuid) -> i64 {
    count(
        db,
        "SELECT count(*) AS n FROM agentic_task_queue \
         WHERE spec->>'kind' = 'app_function' AND spec->'payload'->>'app_id' = $1",
        vec![app.to_string().into()],
    )
    .await
}

/// The function schedule rows naming the app, by `target_ref`.
async fn schedules(db: &DatabaseConnection, app: Uuid) -> Vec<String> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT target_ref FROM agentic_schedules WHERE target_ref LIKE $1 ORDER BY 1",
        vec![format!("{app}/%").into()],
    ))
    .await
    .expect("read schedules")
    .iter()
    .map(|r| r.try_get("", "target_ref").expect("target_ref"))
    .collect()
}

#[tokio::test]
async fn demo_fixture_publishes_five_builds_and_leaves_the_bad_release_live() {
    let db = seeded().await;
    let before = Utc::now();
    let built = build(&db).await;
    let app = fixture_app_id(LOCAL_ORG_ID);
    assert_eq!(
        built.app_id, app,
        "the row keeps the derived id through the publishes"
    );
    assert!(!built.rebuilt);
    assert_eq!(built.builds.len(), RELEASES);
    assert_eq!(built.queued, Ok(QUEUED_RUNS));

    let releases = bundle_releases();
    let rows = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app))
        .order_by_asc(app_builds::Column::CreatedAt)
        .all(&db)
        .await
        .expect("builds");
    let ids: Vec<&str> = rows.iter().map(|b| b.build_id.as_str()).collect();
    let expected: Vec<&str> = releases.iter().map(|r| r.build_id.as_str()).collect();
    assert_eq!(ids, expected, "five builds, re-dated into release order");
    assert!(rows.iter().all(|b| b.validation_status == "passed"));

    let pointers: HashMap<String, Option<Uuid>> = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app))
        .all(&db)
        .await
        .expect("environments")
        .into_iter()
        .map(|e| (e.name, e.build_id))
        .collect();
    assert_eq!(
        pointers["production"],
        Some(built.builds[LIVE - 1]),
        "r4, the bad one, is live"
    );
    assert_eq!(
        pointers["staging"],
        Some(built.builds[RELEASES - 1]),
        "r5 is on staging"
    );

    for (k, build) in built.builds.iter().enumerate() {
        let n = app_functions::Entity::find()
            .filter(app_functions::Column::BuildId.eq(*build))
            .all(&db)
            .await
            .expect("functions")
            .len();
        assert_eq!(n, releases[k].functions().len(), "r{} functions", k + 1);
    }
    let app_row = apps::Entity::find_by_id(app)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(app_row.name, "Pulse (demo fixture)");
    assert!(app_row.published_at.is_some());

    assert_eq!(
        schedules(&db, app).await,
        vec![format!("{app}/digest"), format!("{app}/snapshot")],
        "production's schedules, registered by the real promotes"
    );
    assert_eq!(queued_runs(&db, app).await, QUEUED_RUNS as i64);
    let audits = count(
        &db,
        "SELECT count(*) AS n FROM audit_events WHERE target_id LIKE $1 \
         AND action IN ('app.environment.published', 'app.promoted')",
        vec![format!("{app}%").into()],
    )
    .await;
    assert_eq!(
        audits,
        (RELEASES + LIVE) as i64,
        "five publish rows and four promote rows"
    );
    assert_history_is_on_the_calendar(&db, app, &built, before).await;
}

async fn assert_history_is_on_the_calendar(
    db: &DatabaseConnection,
    app: Uuid,
    built: &seed_fixture::Built,
    before: chrono::DateTime<Utc>,
) {
    let events = app_environment_events::Entity::find()
        .filter(app_environment_events::Column::AppId.eq(app))
        .order_by_asc(app_environment_events::Column::At)
        .all(db)
        .await
        .expect("events");
    let production: Vec<_> = events
        .iter()
        .filter(|e| e.environment == "production")
        .collect();
    let promoted: Vec<Option<Uuid>> = production.iter().map(|e| e.build_id).collect();
    let want: Vec<Option<Uuid>> = built.builds[..LIVE].iter().copied().map(Some).collect();
    assert_eq!(promoted, want, "r1..r4 promoted in order, no rollback");
    assert!(production.iter().all(|e| e.action == "promote"));
    let span = before.fixed_offset() - events[0].at;
    assert!(span >= Duration::days(7) && span < Duration::days(7) + Duration::minutes(2));
    // The calendar is cut to the minute, so `before` is up to a minute past it.
    let live_for = before.fixed_offset() - production[LIVE - 1].at;
    assert!(live_for >= Duration::hours(6) && live_for < Duration::hours(6) + Duration::minutes(1));

    // The release timeline's numbers, read the way it reads them: per build.
    let rate = |failed: i64, calls: i64| failed as f64 / calls as f64;
    let mut sides = Vec::new();
    for build in &built.builds[..LIVE] {
        let calls = count(
            db,
            "SELECT count(*) AS n FROM app_function_invocations WHERE app_id = $1 AND build_id = $2",
            vec![app.into(), (*build).into()],
        )
        .await;
        let failed = count(
            db,
            "SELECT count(*) AS n FROM app_function_invocations WHERE app_id = $1 \
             AND build_id = $2 AND failure_fingerprint IS NOT NULL",
            vec![app.into(), (*build).into()],
        )
        .await;
        sides.push((calls, failed));
    }
    let (r3, r4) = (sides[2], sides[3]);
    assert!(r3.0 >= 50 && r4.0 >= 50, "{sides:?}");
    assert_eq!(r3.1, 0);
    assert!(
        r4.1 >= 3 && rate(r4.1, r4.0) > 2.0 * rate(r3.1, r3.0) + 0.01,
        "{sides:?}"
    );
    let unseeded = count(
        db,
        "SELECT count(*) AS n FROM app_function_invocations WHERE app_id = $1 AND created_at >= $2",
        vec![app.into(), before.fixed_offset().into()],
    )
    .await;
    assert_eq!(unseeded, 0, "every seeded row is in the past");
    let vitals_with_build = count(
        db,
        "SELECT count(*) AS n FROM custom_app_event WHERE app_id = $1 \
         AND event_name = 'oxy-web-vitals' AND payload->>'build' = $2 AND payload->>'path' = '/store'",
        vec![app.into(), built.builds[LIVE - 1].to_string().into()],
    )
    .await;
    assert!(
        vitals_with_build >= 30,
        "r4's /store vitals carry its build: {vitals_with_build}"
    );
}

#[tokio::test]
async fn demo_fixture_a_second_run_finds_it_present_and_writes_nothing() {
    let db = seeded().await;
    build(&db).await;
    let app = fixture_app_id(LOCAL_ORG_ID);
    let first = footprint(&db, app).await;
    let again = seed_fixture::seed(&db, demo_workspace_id(), true, seed::facts())
        .await
        .expect("second run");
    assert!(matches!(again, Outcome::Present), "{again:?}");
    assert_eq!(
        footprint(&db, app).await,
        first,
        "nothing written, nothing queued"
    );
}

#[tokio::test]
async fn demo_fixture_a_partial_fixture_is_removed_and_rebuilt() {
    let db = seeded().await;
    // One seed time for both seeds: the same calendar, so the same rows.
    let now = Utc::now();
    let first = build_at(&db, now).await;
    let app = fixture_app_id(LOCAL_ORG_ID);
    let full = footprint(&db, app).await;
    // A crash after the publishes, before the history.
    for table in ["app_function_invocations", "custom_app_event"] {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("DELETE FROM {table} WHERE app_id = $1"),
            vec![app.into()],
        ))
        .await
        .expect("drop the history");
    }
    let rebuilt = build_at(&db, now).await;
    assert!(rebuilt.rebuilt);
    assert_ne!(
        rebuilt.builds, first.builds,
        "new build rows after the old app was removed"
    );
    let mut want = full.clone();
    want.insert("queued", full["queued"] * 2);
    assert_eq!(
        footprint(&db, app).await,
        want,
        "the same fixture again, nothing doubled"
    );
}

/// A seed that stopped after the first chunk of invocations: r1's rows are
/// there, the later chunks and every vitals row are not. Only the sentinel —
/// the last row a build writes — says the fixture is whole.
#[tokio::test]
async fn demo_fixture_a_seed_interrupted_mid_history_is_rebuilt() {
    let db = seeded().await;
    // One seed time for both seeds: the same calendar, so the same rows.
    let now = Utc::now();
    build_at(&db, now).await;
    let app = fixture_app_id(LOCAL_ORG_ID);
    let full = footprint(&db, app).await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM app_function_invocations WHERE app_id = $1 AND id NOT IN \
           (SELECT id FROM app_function_invocations WHERE app_id = $1 ORDER BY created_at LIMIT 500)",
        vec![app.into()],
    ))
    .await
    .expect("drop the later chunks");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM custom_app_event WHERE app_id = $1",
        vec![app.into()],
    ))
    .await
    .expect("drop the vitals");
    let rebuilt = build_at(&db, now).await;
    assert!(rebuilt.rebuilt, "a partial history is not present");
    let mut want = full.clone();
    want.insert("queued", full["queued"] * 2);
    assert_eq!(footprint(&db, app).await, want, "the whole fixture again");
}

#[tokio::test]
async fn demo_fixture_clear_retires_its_schedules_and_removes_every_row() {
    let db = seeded().await;
    build(&db).await;
    let app = fixture_app_id(LOCAL_ORG_ID);
    assert_eq!(schedules(&db, app).await.len(), 2);
    assert_eq!(seed_fixture::clear_fixture(&db).await.expect("clear"), 1);
    let left = footprint(&db, app).await;
    for (what, n) in &left {
        if *what != "queued" {
            assert_eq!(*n, 0, "{what} left behind");
        }
    }
    assert!(
        apps::Entity::find_by_id(app)
            .one(&db)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        seed_fixture::clear_fixture(&db).await.expect("clear again"),
        0
    );
}

#[tokio::test]
async fn demo_fixture_leaves_a_developers_own_pulse_demo_alone() {
    let db = seeded().await;
    let theirs = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    apps::ActiveModel {
        id: ActiveValue::Set(theirs),
        slug: ActiveValue::Set("pulse-demo".into()),
        name: ActiveValue::Set("Mine".into()),
        org_id: ActiveValue::Set(LOCAL_ORG_ID),
        project_id: ActiveValue::Set(demo_workspace_id()),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("me/mine".into()),
        status: ActiveValue::Set("created".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(json!({})),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("a developer's own app");
    let outcome = seed_fixture::seed(&db, demo_workspace_id(), true, seed::facts())
        .await
        .expect("seed");
    assert!(matches!(outcome, Outcome::Foreign), "{outcome:?}");
    assert_eq!(seed_fixture::clear_fixture(&db).await.expect("clear"), 0);
    let row = apps::Entity::find_by_id(theirs)
        .one(&db)
        .await
        .unwrap()
        .expect("still there");
    assert_eq!(row.name, "Mine");
    let builds = count(
        &db,
        "SELECT count(*) AS n FROM app_builds WHERE app_id = $1",
        vec![theirs.into()],
    )
    .await;
    assert_eq!(builds, 0);
}

/// r4's real `store`, run by the real runner on a stale store id, records the
/// fingerprint the seeded r4 failures carry — so Issues shows one issue, not
/// two, once the queued runs have run on a dev box.
#[tokio::test]
async fn demo_fixture_r4s_real_failure_has_the_seeded_fingerprint() {
    let t = seeded_tenant().await;
    let (pg, url) = fresh_db(Schema::Central).await;
    pg.execute_unprepared(
        "CREATE TABLE oxymart (\"Store\" int, \"Weekly_Sales\" numeric, \"Holiday_Flag\" int);
         INSERT INTO oxymart VALUES (7, 1000000, 0), (7, 1500000, 1), (8, 900000, 0);",
    )
    .await
    .expect("an oxymart table");
    let root = write_config(&postgres_entry(&url));
    let workspace = compile(&t, root.path()).await;

    let r4 = &bundle_releases()[LIVE - 1];
    let spec = |name: &'static str| FunctionSpec {
        name,
        manifest: r4.manifest()["functions"][name].clone(),
        js: Box::leak(
            String::from_utf8(r4.files[&format!("functions/{name}.js")].clone())
                .expect("UTF-8")
                .into_boxed_str(),
        ),
    };
    let published = publish_app(
        &t,
        "pulse-r4",
        workspace,
        &[spec("store"), spec("snapshot")],
    )
    .await;
    let call = |name: &'static str, body: serde_json::Value| {
        let org = t.org_slug.clone();
        async move { call_function_in(&org, "pulse-r4", name, body).await }
    };
    let snap = call("snapshot", json!({})).await;
    assert_eq!(
        snap.frame("done").and_then(|d| d["status"].as_u64()),
        Some(200),
        "{}",
        snap.raw
    );
    // The real store, reading the snapshot the real schedule writes: rank 1.
    let ok = call("store", json!({ "store": 7 })).await;
    assert_eq!(
        ok.frame("done").and_then(|d| d["status"].as_u64()),
        Some(200),
        "{}",
        ok.raw
    );
    assert_eq!(
        ok.frame("data").map(|d| d["rank"].clone()),
        Some(json!(1)),
        "{}",
        ok.raw
    );
    call("store", json!({ "store": 9999 })).await;

    let rows = invocations(&t.db, published.app_id, "store").await;
    let failed: Vec<_> = rows.iter().filter(|r| r.status == "error").collect();
    assert_eq!(
        failed.len(),
        1,
        "the stale id fails, the real one does not: {rows:?}"
    );
    let seeded = seeded_invocations(Uuid::new_v4(), &Calendar::at(Utc::now()), Uuid::nil())
        .into_iter()
        .find_map(|r| r.fingerprint)
        .expect("the history has r4's failure");
    assert_eq!(
        failed[0].failure_fingerprint.as_deref(),
        Some(seeded.as_str()),
        "the runtime recorded {:?}",
        failed[0].error
    );
}

/// `url` as a `config.yml` Postgres entry named `pg`, its password parked in
/// an env var (an inline one is redacted by the compile).
fn postgres_entry(url: &str) -> String {
    let url = url::Url::parse(url).expect("fresh_db hands back a URL");
    let var = format!("DEMO_FIXTURE_PG_PASSWORD_{}", Uuid::new_v4().simple());
    // SAFETY: nextest runs each test in its own process, and this is set before
    // any connector resolves it.
    unsafe { std::env::set_var(&var, url.password().unwrap_or_default()) };
    format!(
        "  - name: pg\n    type: postgres\n    host: {host}\n    port: \"{port}\"\n    \
         user: {user}\n    password_var: {var}\n    database: {database}\n",
        host = url.host_str().expect("host"),
        port = url.port().unwrap_or(5432),
        user = url.username(),
        database = url.path().trim_start_matches('/'),
    )
}
