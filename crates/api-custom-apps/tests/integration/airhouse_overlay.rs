//! `ctx.airhouse` under `airhouse=branch` (`internal-docs/2026-11-02-custom-app-
//! branches-p3-plan.md` task 4): a function reads back its own writes. The
//! first write to a table copies production's rows into the environment's
//! sibling, under a Postgres advisory lock, and records the table in the
//! shadow registry; a query then reads the sibling for the tables it holds and
//! production for the rest.
//!
//! One in-memory DuckDB plays the workspace's Airhouse, as in
//! `staging_airhouse_sibling`; the shadow registry is the real
//! `workspace_preview_tables` in the test's control-plane database. The
//! `snapshot` pin renders `AT (TIMESTAMP => …)`, which DuckDB does not take
//! on a plain table, so the pinned read is covered by the `airhouse` crate's
//! rewrite tests and not here.

use std::sync::{Arc, Mutex};

use agentic_connector::{DatabaseConnector, DuckDbConnector};
use airhouse::preview_sql::{ShadowState, registry};
use chrono::{TimeZone, Utc};
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::{OnMissing, WorkingCopy};
use oxy_api_custom_apps::custom_apps_functions::env_policy::EnvPolicy;
use oxy_api_custom_apps::custom_apps_functions::env_policy::airhouse_overlay::overlay_key;
use oxy_api_custom_apps::custom_apps_functions::runtime::FunctionHost;
use oxy_api_custom_apps::custom_apps_functions::seam::FunctionProjectContext;
use oxy_api_custom_apps::custom_apps_migrations::AirhouseDrop;
use oxy_api_custom_apps::custom_apps_planes::{AirhouseMode, PlaneSpec};
use oxy_api_custom_apps::custom_apps_sandboxes::airhouse_overlay::{
    airhouse_diff_over, reset_airhouse_overlay, reset_airhouse_overlay_over,
};
use oxy_app::surface::testing::agentic_wiring::OxyProjectContext;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::json;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, publish_app, seeded_tenant};
use crate::sandbox_teardown::standin;
use crate::staging_homes_fixture::{APP_SCHEMA, exec, host_on, ids};

/// The sandbox the hosts here run as, and its sibling.
const HANDLE: &str = "a1";
const SIBLING: &str = "app_receiving__dev_a1";

/// A policy for `environment` with `ctx.airhouse` in `mode`, every other
/// plane at its default.
fn policy_in(environment: AppEnvironment, mode: AirhouseMode) -> EnvPolicy {
    let planes = PlaneSpec {
        airhouse: Some(mode),
        ..PlaneSpec::default()
    }
    .resolve(&environment)
    .expect("a non-production environment has planes");
    EnvPolicy::for_environment(environment).with_planes(planes)
}

fn sandbox() -> AppEnvironment {
    AppEnvironment::Dev {
        handle: HANDLE.into(),
    }
}

/// The workspace's Airhouse, played by one in-memory DuckDB, in the demo
/// workspace (whose row the shadow registry's foreign key needs).
struct Lake {
    inner: Arc<OxyProjectContext>,
    db: Mutex<duckdb::Connection>,
    asked: Mutex<Vec<String>>,
    _root: tempfile::TempDir,
}

impl Lake {
    fn connector(&self) -> Arc<dyn DatabaseConnector> {
        let conn = self.db.lock().unwrap().try_clone().expect("clone");
        Arc::new(DuckDbConnector::new(conn))
    }
}

#[async_trait::async_trait]
impl FunctionProjectContext for Lake {
    fn workspace_manager(&self) -> &WorkspaceManager<WorkingCopy> {
        FunctionProjectContext::workspace_manager(&*self.inner)
    }

    async fn build_connector_for(
        &self,
        db_name: &str,
    ) -> Result<Arc<dyn DatabaseConnector>, oxy_shared::errors::OxyError> {
        FunctionProjectContext::build_connector_for(&*self.inner, db_name).await
    }

    async fn build_app_airhouse_connector(
        &self,
        _app_slug: &str,
        schema: &str,
    ) -> Result<Arc<dyn DatabaseConnector>, oxy_shared::errors::OxyError> {
        self.asked.lock().unwrap().push(schema.to_string());
        Ok(self.connector())
    }

    async fn start_airway_seed(
        &self,
        _db: &sea_orm::DatabaseConnection,
        _request: agentic_pipeline::airway_run::StartAirwayRequest,
    ) -> Result<String, String> {
        Err("not exercised".into())
    }
}

/// A project context whose workspace is the demo one.
async fn demo_project() -> (Arc<OxyProjectContext>, tempfile::TempDir) {
    let root = tempfile::tempdir().expect("workspace dir");
    std::fs::write(root.path().join("config.yml"), "databases:\nmodels: []\n")
        .expect("write config.yml");
    let manager = WorkspaceBuilder::new(demo_workspace_id())
        .with_working_copy(root.path(), None, OnMissing::Fail)
        .await
        .expect("config.yml loads")
        .build()
        .await
        .expect("workspace manager");
    (Arc::new(OxyProjectContext::new(manager)), root)
}

/// Production's `events` holds 1 and 2 and its `other` holds 7; the sibling's
/// `events` is migrated with a column production lacks (`flag`), and empty.
async fn lake() -> Arc<Lake> {
    let (inner, root) = demo_project().await;
    let lake = Arc::new(Lake {
        inner,
        db: Mutex::new(duckdb::Connection::open_in_memory().expect("in-memory DuckDB")),
        asked: Mutex::new(Vec::new()),
        _root: root,
    });
    let setup = lake.connector();
    for sql in [
        format!("CREATE SCHEMA {APP_SCHEMA}"),
        format!("CREATE SCHEMA {SIBLING}"),
        format!("CREATE TABLE {APP_SCHEMA}.events (a INTEGER, note VARCHAR)"),
        format!("CREATE TABLE {APP_SCHEMA}.other (a INTEGER, note VARCHAR)"),
        format!(
            "CREATE TABLE {SIBLING}.events (a INTEGER, note VARCHAR, flag VARCHAR DEFAULT 'migrated')"
        ),
        format!("CREATE TABLE {SIBLING}.other (a INTEGER, note VARCHAR)"),
        format!("INSERT INTO {APP_SCHEMA}.events VALUES (1, 'prod'), (2, 'prod')"),
        format!("INSERT INTO {APP_SCHEMA}.other VALUES (7, 'prod')"),
    ] {
        exec(&*setup, &sql).await;
    }
    lake
}

struct World {
    lake: Arc<Lake>,
    host: Arc<dyn FunctionHost>,
    db: DatabaseConnection,
}

async fn world(mode: AirhouseMode) -> World {
    let tenant = Box::pin(seeded_tenant()).await;
    let lake = lake().await;
    let host = host_on(
        tenant.db.clone(),
        lake.clone() as Arc<dyn FunctionProjectContext>,
        &[],
        &[],
        policy_in(sandbox(), mode),
    );
    World {
        lake,
        host,
        db: tenant.db,
    }
}

impl World {
    async fn exec(&self, sql: &str) {
        self.host
            .airhouse("exec".into(), json!({ "sql": sql }))
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    /// The `a` column a function's query sees, sorted.
    async fn seen(&self, table: &str) -> Vec<i64> {
        let sql = format!("SELECT a FROM {APP_SCHEMA}.{table} ORDER BY a");
        let answer = self
            .host
            .airhouse("query".into(), json!({ "sql": sql }))
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        answer["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["a"].as_i64().expect("an integer"))
            .collect()
    }

    /// The shadow registry's rows for this environment: `schema.table:state`.
    async fn shadow(&self) -> Vec<String> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT live_schema || '.' || table_name || ':' || state AS t \
                 FROM workspace_preview_tables WHERE preview_key LIKE 'env:%' ORDER BY 1"
                    .to_string(),
            ))
            .await
            .expect("read the registry");
        rows.iter()
            .map(|r| r.try_get::<String>("", "t").expect("t"))
            .collect()
    }

    async fn sibling(&self, table: &str) -> Vec<i64> {
        ids(&*self.lake.connector(), &format!("{SIBLING}.{table}")).await
    }

    async fn production(&self, table: &str) -> Vec<i64> {
        ids(&*self.lake.connector(), &format!("{APP_SCHEMA}.{table}")).await
    }
}

/// The first write copies production's rows, then lands; a query reads
/// production's rows and the new one, from the sibling.
#[tokio::test]
async fn a_branch_write_copies_production_then_reads_back_its_own() {
    let w = world(AirhouseMode::Branch).await;
    assert_eq!(
        w.seen("events").await,
        vec![1, 2],
        "nothing held: production"
    );

    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.events (a, note) VALUES (3, 'branch')"
    ))
    .await;

    assert_eq!(w.seen("events").await, vec![1, 2, 3]);
    assert_eq!(w.sibling("events").await, vec![1, 2, 3]);
    assert_eq!(
        w.production("events").await,
        vec![1, 2],
        "production untouched"
    );
    assert_eq!(
        w.shadow().await,
        vec![format!("{APP_SCHEMA}.events:shadow")]
    );
}

/// A table only a newer migration made — production has none, so no copy
/// precedes the write — is recorded once the write lands, and reads back.
#[tokio::test]
async fn a_table_only_the_sibling_has_reads_back() {
    let w = world(AirhouseMode::Branch).await;
    exec(
        &*w.lake.connector(),
        &format!("CREATE TABLE {SIBLING}.fresh (a INTEGER, note VARCHAR)"),
    )
    .await;

    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.fresh (a, note) VALUES (5, 'branch')"
    ))
    .await;

    assert_eq!(w.seen("fresh").await, vec![5]);
    assert_eq!(w.shadow().await, vec![format!("{APP_SCHEMA}.fresh:shadow")]);
}

/// A `TRUNCATE` before any other write empties the sibling's migrated table
/// in place (its newer column survives), and the environment then reads it
/// empty while production keeps its rows.
#[tokio::test]
async fn a_first_truncate_empties_the_sibling_in_place() {
    let w = world(AirhouseMode::Branch).await;
    w.exec(&format!("TRUNCATE {APP_SCHEMA}.events")).await;

    assert_eq!(w.seen("events").await, Vec::<i64>::new());
    assert_eq!(w.production("events").await, vec![1, 2]);
    assert_eq!(
        w.shadow().await,
        vec![format!("{APP_SCHEMA}.events:shadow")]
    );
    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.events (a, note) VALUES (3, 'branch')"
    ))
    .await;
    let flag = w
        .lake
        .connector()
        .execute_query(&format!("SELECT flag FROM {SIBLING}.events"), 10)
        .await
        .expect("the migrated column is still there");
    assert_eq!(flag.result.rows.len(), 1);
}

/// A table already copied is not copied again: a row production gained since
/// stays out, and the environment's own rows stay in.
#[tokio::test]
async fn a_second_write_does_not_copy_again() {
    let w = world(AirhouseMode::Branch).await;
    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.events (a, note) VALUES (3, 'branch')"
    ))
    .await;
    exec(
        &*w.lake.connector(),
        &format!("INSERT INTO {APP_SCHEMA}.events VALUES (99, 'prod, later')"),
    )
    .await;

    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.events (a, note) VALUES (4, 'branch')"
    ))
    .await;

    assert_eq!(w.seen("events").await, vec![1, 2, 3, 4]);
}

/// A table the environment never wrote is production's, live.
#[tokio::test]
async fn a_table_never_written_reads_production() {
    let w = world(AirhouseMode::Branch).await;
    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.events (a, note) VALUES (3, 'branch')"
    ))
    .await;
    assert_eq!(w.seen("other").await, vec![7]);
    exec(
        &*w.lake.connector(),
        &format!("INSERT INTO {APP_SCHEMA}.other VALUES (8, 'prod, later')"),
    )
    .await;
    assert_eq!(w.seen("other").await, vec![7, 8], "live, not copied");
}

/// A table over `OXY_PREVIEW_COW_MAX_ROWS` is copied empty and recorded
/// `partial`: the environment sees only what it wrote.
#[tokio::test]
async fn a_table_over_the_cap_is_copied_empty_and_partial() {
    // SAFETY: nextest runs each test in its own process, and this is set
    // before the host reads it.
    unsafe { std::env::set_var("OXY_PREVIEW_COW_MAX_ROWS", "1") };
    let w = world(AirhouseMode::Branch).await;

    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.events (a, note) VALUES (3, 'branch')"
    ))
    .await;

    assert_eq!(w.seen("events").await, vec![3]);
    assert_eq!(
        w.shadow().await,
        vec![format!("{APP_SCHEMA}.events:partial")]
    );
    assert_eq!(w.production("events").await, vec![1, 2]);
}

/// The sibling's table was made by the app's migrations, so the refill keeps
/// its columns: a column production lacks survives, with its default.
#[tokio::test]
async fn a_migrated_sibling_column_survives_the_refill() {
    let w = world(AirhouseMode::Branch).await;
    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.events (a, note) VALUES (3, 'branch')"
    ))
    .await;

    let answer = w
        .host
        .airhouse(
            "query".into(),
            json!({ "sql": format!("SELECT a, flag FROM {APP_SCHEMA}.events ORDER BY a") }),
        )
        .await
        .expect("query the migrated column");
    let flags: Vec<&str> = answer["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["flag"].as_str().expect("a flag"))
        .collect();
    assert_eq!(flags, vec!["migrated"; 3], "{answer}");
}

/// `write-only` is today's behaviour: writes go to the sibling, queries read
/// production, and no shadow row is ever written.
#[tokio::test]
async fn write_only_never_reads_the_sibling() {
    let w = world(AirhouseMode::WriteOnly).await;
    w.exec(&format!(
        "INSERT INTO {APP_SCHEMA}.events (a, note) VALUES (3, 'sandbox')"
    ))
    .await;

    assert_eq!(
        w.seen("events").await,
        vec![1, 2],
        "the query reads production"
    );
    assert_eq!(w.sibling("events").await, vec![3], "the write landed alone");
    assert!(w.shadow().await.is_empty());
}

/// The overlay of staging's sibling, with a stand-in Postgres for Airhouse:
/// `events` is held, `other` is not.
async fn stand_in_with_a_held_table() -> tokio_postgres::Client {
    let client = standin().await;
    client
        .batch_execute(
            "CREATE SCHEMA app_receiving__staging; \
             CREATE TABLE app_receiving__staging.events (a INTEGER); \
             CREATE TABLE app_receiving__staging.other (a INTEGER); \
             INSERT INTO app_receiving__staging.events VALUES (1), (2); \
             INSERT INTO app_receiving__staging.other VALUES (7);",
        )
        .await
        .expect("seed the stand-in");
    client
}

async fn count(client: &tokio_postgres::Client, table: &str) -> i64 {
    client
        .query_one(
            &format!("SELECT count(*) FROM app_receiving__staging.{table}"),
            &[],
        )
        .await
        .expect("count")
        .get(0)
}

async fn pin_of(db: &DatabaseConnection, app_id: uuid::Uuid) -> Option<chrono::DateTime<Utc>> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT airhouse_pinned_at FROM app_environments WHERE app_id = $1 AND name = 'staging'",
            [app_id.into()],
        ))
        .await
        .expect("read the pin")
        .expect("staging's row");
    row.try_get("", "airhouse_pinned_at").expect("pin")
}

/// A reset deletes the rows of the tables the overlay holds and keeps the
/// tables, forgets them in the registry (this environment's rows alone), and
/// re-pins under `snapshot`; `diff` reports them first.
#[tokio::test]
async fn a_reset_empties_the_held_tables_and_forgets_them() {
    let t = Box::pin(seeded_tenant()).await;
    let app_id = Box::pin(publish_app(
        &t,
        "receiving",
        demo_workspace_id(),
        &[FunctionSpec {
            name: "noop",
            manifest: json!({ "route": true }),
            js: "export default async () => Response.json({});",
        }],
    ))
    .await
    .app_id;
    t.db.execute_unprepared(&format!(
        "INSERT INTO app_environments (app_id, name, kind) VALUES ('{app_id}', 'staging', 'staging') \
         ON CONFLICT DO NOTHING"
    ))
    .await
    .expect("staging's row");
    let client = stand_in_with_a_held_table().await;
    let ws = demo_workspace_id();
    let env = AppEnvironment::Staging;
    let neighbour = overlay_key(app_id, &sandbox());
    for (key, table) in [
        (overlay_key(app_id, &env), "events"),
        (neighbour.clone(), "other"),
    ] {
        registry::upsert_shadow(
            &t.db,
            ws,
            &key,
            "run",
            &[(
                (APP_SCHEMA.to_string(), table.to_string()),
                ShadowState::Shadow,
            )],
        )
        .await
        .expect("record");
    }
    let run = AirhouseDrop {
        app_id,
        app_slug: "receiving",
        workspace_id: ws,
    };

    let diff = airhouse_diff_over(&t.db, run, &env, AirhouseMode::Branch, &client)
        .await
        .expect("diff");
    assert_eq!(
        diff,
        json!({ "mode": "branch", "pinned_at": null, "tables": [
            { "schema": APP_SCHEMA, "table": "events", "state": "shadow", "rows": 2 }
        ] })
    );

    let at = Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap();
    let outcome = reset_airhouse_overlay_over(&t.db, run, &env, true, Some(at), &client)
        .await
        .expect("reset");
    assert_eq!(outcome.tables_emptied, vec![format!("{APP_SCHEMA}.events")]);
    assert_eq!(outcome.shadow_rows_cleared, 1);
    assert_eq!(
        (
            count(&client, "events").await,
            count(&client, "other").await
        ),
        (0, 1)
    );
    assert!(
        registry::load_shadow(&t.db, ws, &overlay_key(app_id, &env))
            .await
            .expect("load")
            .0
            .is_empty()
    );
    assert_eq!(
        registry::load_shadow(&t.db, ws, &neighbour)
            .await
            .expect("load")
            .0
            .len(),
        1,
        "another environment's rows stay"
    );
    assert_eq!(pin_of(&t.db, app_id).await, Some(at));

    reset_airhouse_overlay_over(&t.db, run, &env, false, None, &client)
        .await
        .expect("a second reset is a no-op");
    assert_eq!(
        pin_of(&t.db, app_id).await,
        None,
        "outside snapshot there is no pin"
    );
}

/// An app whose slug backs no Airhouse schema has no sibling, so its overlay
/// holds nothing: a reset still forgets, re-pins and is done, never an error.
#[tokio::test]
async fn a_reset_with_no_sibling_only_forgets_and_re_pins() {
    let t = Box::pin(seeded_tenant()).await;
    let app_id = Box::pin(publish_app(
        &t,
        "receiving",
        demo_workspace_id(),
        &[FunctionSpec {
            name: "noop",
            manifest: json!({ "route": true }),
            js: "export default async () => Response.json({});",
        }],
    ))
    .await
    .app_id;
    t.db.execute_unprepared(&format!(
        "INSERT INTO app_environments (app_id, name, kind) VALUES ('{app_id}', 'staging', 'staging') \
         ON CONFLICT DO NOTHING"
    ))
    .await
    .expect("staging's row");
    let run = AirhouseDrop {
        app_id,
        app_slug: "9-backs-no-schema",
        workspace_id: demo_workspace_id(),
    };
    let at = Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap();
    let outcome = reset_airhouse_overlay(&t.db, run, &AppEnvironment::Staging, true, Some(at))
        .await
        .expect("nothing to empty is not an error");
    assert!(outcome.tables_emptied.is_empty());
    assert_eq!(pin_of(&t.db, app_id).await, Some(at));
}
