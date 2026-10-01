//! Staging's Airhouse migrations run in the app schema's sibling and are
//! recorded under the sibling's own ledger target — so they never mark
//! production's files applied, and promote still plans production's DDL
//! (environments design §4.3, §8 "staging migrations don't mark production's
//! as applied").
//!
//! The apply runs over a connection the test hands it: a per-test Postgres
//! database stands in for the workspace's Airhouse (both speak the Postgres
//! wire protocol, and these files are plain `CREATE SCHEMA` / `CREATE TABLE`).
//! Everything else is the publish path's own code: `declare_airhouse` reads
//! the bundle, `AirhouseHome` names the schema and target, and
//! `apply_airhouse_over` runs, moves and records each file against the real
//! control-plane ledger.

use entity::app_builds;
use oxy_app::server::api::custom_apps_migrations::{
    AirhouseHome, MigrationTarget, apply_airhouse_over, apply_airhouse_over_until,
    declare_airhouse, read_ledger,
};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;

use crate::common::{demo_workspace_id, empty_db};
use crate::custom_app_functions_fixture::{FunctionSpec, publish_app, seeded_tenant};

const APP: &str = "store-ops";

const FILES: &[(&str, &str)] = &[
    (
        "airhouse-migrations/0001_visits.sql",
        "CREATE SCHEMA IF NOT EXISTS app_store_ops;
         CREATE TABLE app_store_ops.visits (visit_id VARCHAR NOT NULL, store VARCHAR);",
    ),
    (
        "airhouse-migrations/0002_daily.sql",
        "CREATE TABLE app_store_ops.daily (store VARCHAR, n INTEGER);
         INSERT INTO app_store_ops.daily SELECT store, count(*) FROM app_store_ops.visits GROUP BY store;",
    ),
];

/// The Airhouse stand-in: a fresh database, and a client on it.
async fn standin() -> tokio_postgres::Client {
    let (_db, url) = empty_db().await;
    let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
        .await
        .expect("connect to the stand-in");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// The tables in `schema` on the stand-in, sorted.
async fn tables(client: &tokio_postgres::Client, schema: &str) -> Vec<String> {
    let rows = client
        .query(
            "SELECT table_name::text FROM information_schema.tables WHERE table_schema = $1 \
             ORDER BY table_name",
            &[&schema],
        )
        .await
        .expect("list tables");
    rows.iter().map(|r| r.get::<_, String>(0)).collect()
}

#[tokio::test]
async fn staging_airhouse_migrations_run_in_the_sibling_and_never_mark_productions_applied() {
    let t = seeded_tenant().await;
    let app_id = publish_app(
        &t,
        APP,
        demo_workspace_id(),
        &[FunctionSpec {
            name: "noop",
            manifest: json!({ "route": true }),
            js: "export default async () => Response.json({});",
        }],
    )
    .await
    .app_id;
    let build_pk = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .one(&t.db)
        .await
        .expect("read the build")
        .expect("the publish recorded a build")
        .id;
    let bundle: Vec<(String, Vec<u8>)> = FILES
        .iter()
        .map(|(path, sql)| (path.to_string(), sql.as_bytes().to_vec()))
        .collect();
    let declared = declare_airhouse(
        Some(&json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } })),
        &bundle,
        APP,
    )
    .expect("the files pass production's rules");
    let client = standin().await;

    // A staging publish: the sibling is created and migrated, under its target.
    let staging = AirhouseHome::for_environment(APP, &AppEnvironment::Staging)
        .expect("a valid slug")
        .expect("staging has a sibling");
    let applied = apply_airhouse_over(&t.db, app_id, build_pk, &declared, &staging, &client)
        .await
        .expect("staging applies");
    assert_eq!(applied.applied, vec!["0001_visits.sql", "0002_daily.sql"]);
    assert_eq!(
        tables(&client, "app_store_ops__staging").await,
        vec!["daily", "visits"]
    );
    assert!(
        tables(&client, "app_store_ops").await.is_empty(),
        "staging's DDL must not touch production's schema"
    );

    // Production's ledger is untouched, so promote plans every file.
    let production_ledger = read_ledger(&t.db, app_id, "airhouse", &MigrationTarget::Production)
        .await
        .expect("read production's ledger");
    assert!(
        production_ledger.is_empty(),
        "staging's applied files read as production's: {production_ledger:?}"
    );
    let staging_ledger = read_ledger(&t.db, app_id, "airhouse", staging.target())
        .await
        .expect("read staging's ledger");
    assert_eq!(staging_ledger.len(), 2);
    assert_eq!(staging.target().as_key(), "schema:app_store_ops__staging");

    // The promote then applies production's own copy, as written.
    let production = AirhouseHome::production(APP).expect("a valid slug");
    let promoted = apply_airhouse_over(&t.db, app_id, build_pk, &declared, &production, &client)
        .await
        .expect("production applies");
    assert_eq!(
        promoted.applied,
        vec!["0001_visits.sql", "0002_daily.sql"],
        "promote must run production's DDL, not skip it as staging's"
    );
    assert_eq!(
        tables(&client, "app_store_ops").await,
        vec!["daily", "visits"]
    );

    // Each target now reads its own files as applied; neither re-runs.
    for home in [&staging, &production] {
        let again = apply_airhouse_over(&t.db, app_id, build_pk, &declared, home, &client)
            .await
            .expect("re-apply is a no-op");
        assert_eq!((again.applied.len(), again.already_applied), (0, 2));
    }
}

/// A file that breaks DuckLake's rules is refused for the sibling exactly as
/// it is for production, before anything runs there.
#[tokio::test]
async fn a_file_production_refuses_is_refused_for_the_sibling_before_it_runs() {
    let staging = AirhouseHome::for_environment(APP, &AppEnvironment::Staging)
        .unwrap()
        .unwrap();
    let bundle = vec![(
        "airhouse-migrations/0001_bad.sql".to_string(),
        b"CREATE TABLE app_store_ops.visits (visit_id VARCHAR PRIMARY KEY);".to_vec(),
    )];
    let refused = declare_airhouse(
        Some(&json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } })),
        &bundle,
        APP,
    )
    .expect_err("a key is refused at publish");
    assert!(refused.is_author_fault(), "{refused}");
    assert_eq!(staging.schema(), "app_store_ops__staging");
}

/// The apply's deadline is checked between files: a file in progress when it
/// passes finishes and is recorded — its tenant `COMMIT` and its ledger row
/// are never split — and the files after it are deferred to the next apply.
#[tokio::test]
async fn a_deadline_passing_mid_file_lets_that_file_finish_and_defers_the_rest() {
    let t = seeded_tenant().await;
    let app_id = publish_app(
        &t,
        APP,
        demo_workspace_id(),
        &[FunctionSpec {
            name: "noop",
            manifest: json!({ "route": true }),
            js: "export default async () => Response.json({});",
        }],
    )
    .await
    .app_id;
    let build_pk = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .one(&t.db)
        .await
        .expect("read the build")
        .expect("a build")
        .id;
    let bundle: Vec<(String, Vec<u8>)> = [
        (
            "airhouse-migrations/0001_slow.sql",
            "CREATE TABLE app_store_ops.visits (visit_id VARCHAR NOT NULL);
             SELECT pg_sleep(1);",
        ),
        (
            "airhouse-migrations/0002_next.sql",
            "CREATE TABLE app_store_ops.daily (store VARCHAR);",
        ),
    ]
    .iter()
    .map(|(path, sql)| (path.to_string(), sql.as_bytes().to_vec()))
    .collect();
    let declared = declare_airhouse(
        Some(&json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } })),
        &bundle,
        APP,
    )
    .expect("the files pass");
    let client = standin().await;
    let staging = AirhouseHome::for_environment(APP, &AppEnvironment::Staging)
        .unwrap()
        .unwrap();

    // The deadline passes while 0001 sleeps.
    let until = tokio::time::Instant::now() + std::time::Duration::from_millis(200);
    let first = apply_airhouse_over_until(
        &t.db,
        app_id,
        build_pk,
        &declared,
        &staging,
        &client,
        Some(until),
    )
    .await
    .expect("apply");
    assert_eq!(
        first.applied,
        vec!["0001_slow.sql"],
        "the file in progress finished"
    );
    assert_eq!(
        first.deferred,
        vec!["0002_next.sql"],
        "the next never started"
    );
    let ledger = read_ledger(&t.db, app_id, "airhouse", staging.target())
        .await
        .expect("ledger");
    assert_eq!(ledger.len(), 1, "the finished file is recorded: {ledger:?}");
    assert_eq!(
        tables(&client, "app_store_ops__staging").await,
        vec!["visits"]
    );

    let second = apply_airhouse_over(&t.db, app_id, build_pk, &declared, &staging, &client)
        .await
        .expect("the next apply");
    assert_eq!((second.applied.len(), second.already_applied), (1, 1));
    assert!(second.deferred.is_empty());
}
