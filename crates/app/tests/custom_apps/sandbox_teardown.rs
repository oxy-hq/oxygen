//! Tearing a sandbox down (`internal-docs/custom-app-sandboxes.md` →
//! Lifecycle): each home is removed for that sandbox alone, and what is kept
//! stays.
//!
//! The Airhouse step runs over a connection the test hands it: a per-test
//! Postgres database stands in for the workspace's Airhouse, as in
//! `staging_airhouse_migrations`.

use entity::app_builds;
use oxy_app::server::api::custom_apps_migrations::{
    AirhouseDrop, AirhouseHome, MigrationTarget, apply_airhouse_over, declare_airhouse,
    drop_environment_schema_over, read_ledger,
};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;

use crate::common::{demo_workspace_id, empty_db};
use crate::custom_app_functions_fixture::{FunctionSpec, publish_app, seeded_tenant};

pub(crate) const AIRHOUSE_APP: &str = "store-ops";

pub(crate) const AIRHOUSE_FILES: &[(&str, &str)] = &[
    (
        "airhouse-migrations/0001_visits.sql",
        "CREATE SCHEMA IF NOT EXISTS app_store_ops;
         CREATE TABLE app_store_ops.visits (visit_id VARCHAR NOT NULL, store VARCHAR);",
    ),
    (
        "airhouse-migrations/0002_latest.sql",
        "CREATE VIEW app_store_ops.latest AS SELECT * FROM app_store_ops.visits;",
    ),
];

pub(crate) fn sandbox(handle: &str) -> AppEnvironment {
    AppEnvironment::Dev {
        handle: handle.into(),
    }
}

/// The Airhouse stand-in: a fresh database, and a client on it.
pub(crate) async fn standin() -> tokio_postgres::Client {
    let (_db, url) = empty_db().await;
    let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
        .await
        .expect("connect to the stand-in");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// The relations in `schema` on the stand-in, sorted.
pub(crate) async fn relations(client: &tokio_postgres::Client, schema: &str) -> Vec<String> {
    let rows = client
        .query(
            "SELECT table_name::text FROM information_schema.tables WHERE table_schema = $1 \
             ORDER BY table_name",
            &[&schema],
        )
        .await
        .expect("list relations");
    rows.iter().map(|r| r.get::<_, String>(0)).collect()
}

async fn schema_exists(client: &tokio_postgres::Client, schema: &str) -> bool {
    client
        .query_one(
            "SELECT count(*) FROM information_schema.schemata WHERE schema_name = $1",
            &[&schema],
        )
        .await
        .expect("read schemata")
        .get::<_, i64>(0)
        > 0
}

/// Dropping one sandbox's sibling removes its tables, its view, the schema
/// and its ledger rows — and nothing of production's schema, staging's
/// sibling, or a sandbox whose handle this one's is a prefix of. A second
/// drop is a no-op, and production's schema and staging's sibling are refused
/// outright.
#[tokio::test]
async fn dropping_a_sandbox_sibling_removes_that_schema_and_ledger_alone() {
    let t = seeded_tenant().await;
    let app_id = publish_app(
        &t,
        AIRHOUSE_APP,
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
    let bundle: Vec<(String, Vec<u8>)> = AIRHOUSE_FILES
        .iter()
        .map(|(path, sql)| (path.to_string(), sql.as_bytes().to_vec()))
        .collect();
    let declared = declare_airhouse(
        Some(&json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } })),
        &bundle,
        AIRHOUSE_APP,
    )
    .expect("the files pass production's rules");
    let client = standin().await;

    let home = |environment: &AppEnvironment| {
        AirhouseHome::for_environment(AIRHOUSE_APP, environment)
            .expect("a valid slug")
            .expect("a sibling")
    };
    let mine = home(&sandbox("a1"));
    let neighbour = home(&sandbox("a1-b"));
    let staging = home(&AppEnvironment::Staging);
    let production = AirhouseHome::production(AIRHOUSE_APP).expect("a valid slug");
    for target in [&mine, &neighbour, &staging, &production] {
        apply_airhouse_over(&t.db, app_id, build_pk, &declared, target, &client)
            .await
            .expect("apply");
    }
    assert_eq!(mine.schema(), "app_store_ops__dev_a1");
    assert_eq!(
        relations(&client, mine.schema()).await,
        vec!["latest", "visits"]
    );

    // A drop names the app and the sandbox, never a schema.
    let drop = AirhouseDrop {
        app_id,
        app_slug: AIRHOUSE_APP,
        workspace_id: demo_workspace_id(),
    };
    let outcome = drop_environment_schema_over(&t.db, drop, &sandbox("a1"), &client)
        .await
        .expect("drop the sandbox's sibling");
    assert_eq!(outcome.schema.as_deref(), Some("app_store_ops__dev_a1"));
    assert_eq!(outcome.relations_dropped, 2, "the view and the table");
    assert!(outcome.schema_dropped);
    assert_eq!(outcome.ledger_rows_cleared, 2);

    assert!(!schema_exists(&client, mine.schema()).await);
    assert!(
        read_ledger(&t.db, app_id, "airhouse", mine.target())
            .await
            .expect("ledger")
            .is_empty(),
        "a re-created sandbox of this name must plan every file again"
    );
    for kept in [&neighbour, &staging, &production] {
        assert_eq!(
            relations(&client, kept.schema()).await,
            vec!["latest", "visits"],
            "{} is untouched",
            kept.schema()
        );
        assert_eq!(
            read_ledger(&t.db, app_id, "airhouse", kept.target())
                .await
                .expect("ledger")
                .len(),
            2,
            "{}'s ledger is untouched",
            kept.schema()
        );
    }
    assert_eq!(production.target(), &MigrationTarget::Production);

    // Idempotent: the teardown task may run twice.
    let again = drop_environment_schema_over(&t.db, drop, &sandbox("a1"), &client)
        .await
        .expect("a second drop is a no-op");
    assert_eq!((again.relations_dropped, again.ledger_rows_cleared), (0, 0));

    // Only a sandbox's sibling is ever a drop target: production's schema and
    // staging's sibling are refused, each with everything still in it.
    for (fixed, kept) in [
        (AppEnvironment::Production, &production),
        (AppEnvironment::Staging, &staging),
    ] {
        let refused = drop_environment_schema_over(&t.db, drop, &fixed, &client)
            .await
            .expect_err("a fixed environment's schema is never dropped");
        assert!(
            refused.to_string().contains("refusing to drop"),
            "{fixed}: {refused}"
        );
        assert_eq!(
            relations(&client, kept.schema()).await,
            vec!["latest", "visits"],
            "{fixed}"
        );
        let ledger = read_ledger(&t.db, app_id, "airhouse", kept.target()).await;
        assert_eq!(ledger.expect("ledger").len(), 2, "{fixed}");
    }

    // A re-created sandbox of the same name starts from nothing and applies
    // every file again.
    let reapplied = apply_airhouse_over(&t.db, app_id, build_pk, &declared, &mine, &client)
        .await
        .expect("re-apply after the drop");
    assert_eq!(reapplied.applied.len(), 2);
}
