//! An environment's warehouse overrides
//! (`internal-docs/2026-11-02-custom-app-branches-p1-plan.md`, B5):
//! `planes.warehouse = {"mode": "mapped", "destinations": {<db>: <mapped>}}`.
//!
//! The route checks each override as a publish checks `nonProduction.destinations`
//! against the workspace's compiled config (`400 invalid_planes`, row
//! untouched), the bare `"mapped"` needs no such read, and the policy the
//! runtime builds from the stored row sends a write to the mapped database.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use oxy_api_custom_apps::custom_apps_functions::env_policy::EnvPolicy;
use oxy_api_custom_apps::custom_apps_planes::environment_planes;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, publish_app, seeded_tenant};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::sandbox_routes::{audit_rows, send, staffed_app};
use crate::staging_homes_fixture::postgres_entry_as_own_role;
use crate::staging_homes_fixture::{exec, host, workspace};
use crate::warehouse_writes_on_engines::postgres_entry;

const APP: &str = "ovr";

/// `pg` (production), `pg_alias` (production's host and user: refused as a
/// destination), and `pg_staging` and `pg_other` (a login role of their own
/// each: accepted).
async fn databases() -> String {
    format!(
        "{}{}{}{}",
        postgres_entry("pg").await,
        postgres_entry("pg_alias").await,
        postgres_entry_as_own_role("pg_staging").await,
        postgres_entry_as_own_role("pg_other").await,
    )
}

/// The guest made Oxy staff, as `staffed_app` does, for an app published into `workspace`.
async fn staffed_in(t: &Tenant, workspace: Uuid) -> Uuid {
    let noop = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    let app = publish_app(t, APP, workspace, &[noop]).await.app_id;
    // SAFETY: nextest runs each test in its own process; set after the
    // publish (authorized as the org Owner), before any console request.
    unsafe {
        std::env::set_var("OXY_OWNER", LOCAL_GUEST_EMAIL);
        std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com");
    }
    app
}

fn overrides(destinations: Value) -> Value {
    json!({ "warehouse": { "mode": "mapped", "destinations": destinations } })
}

async fn put_planes(app: Uuid, planes: Value) -> (StatusCode, Value) {
    send(
        "PUT",
        &format!("{app}/environments/dev-a1/planes"),
        Some(json!({ "planes": planes })),
    )
    .await
}

async fn create_sandbox(app: Uuid, body: Value) -> (StatusCode, Value) {
    send("POST", &format!("{app}/environments"), Some(body)).await
}

async fn stored_planes(db: &DatabaseConnection, app: Uuid) -> Value {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT planes FROM app_environments WHERE app_id = $1 AND name = 'dev-a1'",
        [app.into()],
    ))
    .await
    .expect("read the row")
    .expect("the sandbox row")
    .try_get("", "planes")
    .expect("planes")
}

fn invalid_planes(status: StatusCode, body: &Value) -> String {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_planes", "{body}");
    body["message"].as_str().expect("a message").to_string()
}

/// The override is stored as sent, shown in `planes_set`, the plane still
/// reads `mapped`, and the policy built from the row sends the write to the
/// mapped database — `pg` stays empty.
#[tokio::test]
async fn an_override_is_stored_and_a_write_goes_to_the_mapped_database() {
    let t = seeded_tenant().await;
    let config = databases().await;
    let root = write_config(&config);
    let app = staffed_in(&t, compile(&t, root.path()).await).await;
    let (status, created) = create_sandbox(app, json!({ "name": "dev-a1" })).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let choice = overrides(json!({ "pg": "pg_staging" }));
    let (status, shown) = put_planes(app, choice.clone()).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["planes_set"], choice);
    assert_eq!(shown["planes"]["warehouse"], "mapped");
    assert_eq!(stored_planes(&t.db, app).await, choice);

    let environment = AppEnvironment::parse("dev-a1").expect("a sandbox");
    let planes = environment_planes(&t.db, app, &environment)
        .await
        .expect("readable")
        .expect("a sandbox has planes");
    assert_eq!(
        planes.warehouse.destinations,
        BTreeMap::from([("pg".to_string(), "pg_staging".to_string())])
    );

    // The build declares no mapping of its own: the override alone gives `pg` a home.
    let ws = workspace(&config).await;
    for database in ["pg", "pg_staging"] {
        exec(
            &*ws.connector(database).await,
            "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT)",
        )
        .await;
    }
    let policy = EnvPolicy::for_environment(environment)
        .with_planes(planes)
        .with_destinations(BTreeMap::new());
    let sandbox = host(
        ws.ctx(),
        &["pg", "pg_staging"],
        &["pg", "pg_staging"],
        policy,
    );
    sandbox
        .warehouse_write(
            "insert".into(),
            json!({ "database": "pg", "table": "t", "rows": [{ "a": 1, "b": "x" }] }),
        )
        .await
        .expect("the write lands in the mapped database");
    assert_eq!(ws.ids("pg_staging").await, vec![1]);
    assert!(
        ws.ids("pg").await.is_empty(),
        "production's table is untouched"
    );
}

/// What a publish refuses, the route refuses: the row stays as it was and
/// nothing is audited. A create with the same override is refused before it
/// inserts a row.
#[tokio::test]
async fn an_override_a_publish_would_refuse_is_refused_and_leaves_the_row() {
    let t = seeded_tenant().await;
    let root = write_config(&databases().await);
    let app = staffed_in(&t, compile(&t, root.path()).await).await;
    create_sandbox(app, json!({ "name": "dev-a1" })).await;

    let cases = [
        (json!({ "pg": "pg_alias" }), "same host and user"),
        (json!({ "pg": "pg" }), "itself"),
        (json!({ "pg": "nope" }), "`nope`"),
        (json!({ "pg": "" }), "must name a database"),
        (
            json!({ "pg": "pg_staging", "pg_staging": "pg_alias" }),
            "production database of its own",
        ),
    ];
    for (destinations, says) in cases {
        let (status, body) = put_planes(app, overrides(destinations.clone())).await;
        let message = invalid_planes(status, &body);
        assert!(message.contains(says), "{destinations}: {message}");
    }
    assert_eq!(stored_planes(&t.db, app).await, json!({}));
    assert_eq!(
        audit_rows(&t.db, app, "app.environment.planes_set").await,
        0
    );

    let (status, body) = create_sandbox(
        app,
        json!({ "name": "dev-b2", "planes": overrides(json!({ "pg": "pg_alias" })) }),
    )
    .await;
    assert!(invalid_planes(status, &body).contains("same host and user"));
    let (status, _) = send("GET", &format!("{app}/environments/dev-b2"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no row was created");

    let (status, created) = create_sandbox(
        app,
        json!({ "name": "dev-c3", "planes": overrides(json!({ "pg": "pg_staging" })) }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["planes_set"],
        overrides(json!({ "pg": "pg_staging" }))
    );
}

/// The sandbox runs the app's published build, and that build declares
/// `nonProduction.destinations` in its manifest, as a publish stores it.
async fn declare_manifest_destinations(db: &DatabaseConnection, app: Uuid, destinations: Value) {
    for sql in [
        "UPDATE app_builds SET manifest_json = COALESCE(manifest_json, '{}'::jsonb) || $2 \
         WHERE app_id = $1",
        "UPDATE app_environments SET build_id = (SELECT id FROM app_builds WHERE app_id = $1 \
         LIMIT 1) WHERE app_id = $1 AND name = 'dev-a1' AND $2::jsonb IS NOT NULL",
    ] {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [
                app.into(),
                json!({ "nonProduction": { "destinations": destinations } }).into(),
            ],
        ))
        .await
        .expect("declare the build's mapping");
    }
}

/// The chain check runs on the map the runtime merges (the build's mapping
/// under the override, key by key), not on the override alone: `pg_staging`
/// is not a key of the override, but the merged map would send `pg_staging`
/// to `pg`, which the build maps as a production database of its own, and
/// the host would hold every write. Refused when it is set; a key the
/// override replaces is no longer a key of the merged map, so it passes.
#[tokio::test]
async fn an_override_that_chains_through_the_builds_mapping_is_refused() {
    let t = seeded_tenant().await;
    let root = write_config(&databases().await);
    let app = staffed_in(&t, compile(&t, root.path()).await).await;
    create_sandbox(app, json!({ "name": "dev-a1" })).await;
    declare_manifest_destinations(&t.db, app, json!({ "pg": "pg_staging" })).await;

    let (status, body) = put_planes(app, overrides(json!({ "pg_staging": "pg" }))).await;
    let message = invalid_planes(status, &body);
    assert!(
        message.contains("production database of its own"),
        "{message}"
    );
    assert_eq!(stored_planes(&t.db, app).await, json!({}));
    assert_eq!(
        audit_rows(&t.db, app, "app.environment.planes_set").await,
        0
    );

    let replaced = overrides(json!({ "pg": "pg_staging" }));
    let (status, shown) = put_planes(app, replaced.clone()).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(stored_planes(&t.db, app).await, replaced);
}

/// The chain check covers every pair of the merged map, not only the
/// override's own: the build maps `pg -> pg_staging`, and the override
/// `pg_staging -> pg_other` is no chain by itself, but it makes `pg` a chain
/// (`pg_staging` is now a key). The host would hold every write, so it is
/// refused when it is set.
#[tokio::test]
async fn an_override_that_turns_a_pair_of_the_build_into_a_chain_is_refused() {
    let t = seeded_tenant().await;
    let root = write_config(&databases().await);
    let app = staffed_in(&t, compile(&t, root.path()).await).await;
    create_sandbox(app, json!({ "name": "dev-a1" })).await;
    declare_manifest_destinations(&t.db, app, json!({ "pg": "pg_staging" })).await;

    let (status, body) = put_planes(app, overrides(json!({ "pg_staging": "pg_other" }))).await;
    let message = invalid_planes(status, &body);
    assert!(message.contains("maps `pg` to `pg_staging`"), "{message}");
    assert_eq!(stored_planes(&t.db, app).await, json!({}));
    assert_eq!(
        audit_rows(&t.db, app, "app.environment.planes_set").await,
        0
    );
}

/// A create with `from` is checked against the build the sandbox will copy
/// from its parent, not against no build: staging's build maps
/// `pg -> pg_staging`, and the override `pg_staging -> pg` would run as a
/// chain. Refused before any row exists, from staging and from a sandbox
/// alike; an override that replaces the build's pair is still taken.
#[tokio::test]
async fn a_create_from_a_parent_is_checked_against_the_parents_build() {
    let t = seeded_tenant().await;
    let root = write_config(&databases().await);
    let app = staffed_in(&t, compile(&t, root.path()).await).await;
    create_sandbox(app, json!({ "name": "dev-a1" })).await;
    declare_manifest_destinations(&t.db, app, json!({ "pg": "pg_staging" })).await;

    let chain = overrides(json!({ "pg_staging": "pg" }));
    for (name, parent) in [("dev-b2", "staging"), ("dev-c3", "dev-a1")] {
        let (status, body) = create_sandbox(
            app,
            json!({ "name": name, "from": parent, "planes": chain.clone() }),
        )
        .await;
        let message = invalid_planes(status, &body);
        assert!(
            message.contains("production database of its own"),
            "{parent}: {message}"
        );
        let (status, _) = send("GET", &format!("{app}/environments/{name}"), None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{parent}: no row was created"
        );
    }

    let replaced = overrides(json!({ "pg": "pg_staging" }));
    let (status, created) = create_sandbox(
        app,
        json!({ "name": "dev-d4", "from": "staging", "planes": replaced.clone() }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["planes_set"], replaced);
}

/// The bare `"mapped"` and an object with no overrides read nothing from the
/// workspace — an app whose workspace has compiled nothing still takes them;
/// an override there cannot be checked, so it is not stored.
#[tokio::test]
async fn without_a_compiled_workspace_only_a_choice_with_no_override_is_stored() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    create_sandbox(app, json!({ "name": "dev-a1" })).await;

    for bare in [
        json!({ "warehouse": "mapped" }),
        overrides(json!({})),
        json!({ "warehouse": { "mode": "mapped" } }),
    ] {
        let (status, shown) = put_planes(app, bare.clone()).await;
        assert_eq!(status, StatusCode::OK, "{bare}: {shown}");
        assert_eq!(shown["planes_set"], bare);
    }
    let (status, body) = put_planes(app, overrides(json!({ "pg": "pg_staging" }))).await;
    let message = invalid_planes(status, &body);
    assert!(message.contains("could not be checked"), "{message}");
    assert_eq!(
        stored_planes(&t.db, app).await,
        json!({ "warehouse": { "mode": "mapped" } }),
        "the last good choice stays"
    );
}
