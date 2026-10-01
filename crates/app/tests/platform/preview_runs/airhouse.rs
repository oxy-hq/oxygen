//! Phase 2b S8: a preview procedure's managed-Airhouse writes land in the
//! preview's own schemas, and its reads see them — end to end through the
//! route, the real driver and `PreviewRunResolver`, with in-process DuckDB
//! standing in for Airhouse behind every preview port
//! (`previews::airhouse_duckdb`). The rewrite, the verifier, the registry,
//! copy-on-write and the shadow map all run for real.

use std::sync::{Arc, Mutex};

use agentic_pipeline::platform::RunPlatformResolver;
use oxy_app::server::previews::airhouse_duckdb::DuckDbAirhouse;
use oxy_app::server::previews::namespace::preview_key;
use oxy_app::server::previews::runtime::PreviewRunResolver;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{Value, json};

use super::world::{self, run_to_the_end_with};
use crate::preview_routes::fixture::{BRANCH, Fx};

/// Three live orders worth 60.
const LIVE: &str = "CREATE SCHEMA toast_pos; \
    CREATE TABLE toast_pos.orders (id INTEGER, amount INTEGER); \
    INSERT INTO toast_pos.orders VALUES (1, 10), (2, 20), (3, 30);";

pub(super) struct Lake(pub(super) Arc<Mutex<duckdb::Connection>>);

/// Which stand-in Airhouse a run gets.
pub(super) type Ports = fn(Arc<Mutex<duckdb::Connection>>) -> Result<DuckDbAirhouse, String>;

impl Lake {
    pub(super) fn seeded() -> Self {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute_batch(LIVE).unwrap();
        Self(Arc::new(Mutex::new(conn)))
    }

    pub(super) fn int(&self, sql: &str) -> i64 {
        self.0
            .lock()
            .unwrap()
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    /// Live orders as `(rows, total)`: what no preview may change.
    pub(super) fn live_orders(&self) -> (i64, i64) {
        (
            self.int("SELECT count(*) FROM toast_pos.orders"),
            self.int("SELECT sum(amount) FROM toast_pos.orders"),
        )
    }

    pub(super) fn schema_exists(&self, schema: &str) -> bool {
        self.int(&format!(
            "SELECT count(*) FROM information_schema.schemata WHERE schema_name = '{schema}'"
        )) > 0
    }

    fn resolver(&self, fx: &Fx, old: bool) -> Arc<dyn RunPlatformResolver> {
        self.resolver_on(
            fx,
            if old {
                DuckDbAirhouse::old
            } else {
                DuckDbAirhouse::new
            },
        )
    }

    pub(super) fn resolver_on(&self, fx: &Fx, ports: Ports) -> Arc<dyn RunPlatformResolver> {
        Arc::new(PreviewRunResolver::with_airhouse(
            fx.db.clone(),
            ports(self.0.clone()).unwrap().shared(),
        ))
    }
}

pub(super) fn sql_step(name: &str, sql: &str) -> Value {
    json!({ "name": name, "type": "execute_sql", "database": "airhouse", "sql_query": sql })
}

pub(super) fn schema_of(fx: &Fx, live: &str) -> String {
    format!("preview_{}__{live}", preview_key(fx.ws, BRANCH))
}

pub(super) async fn rows(fx: &Fx, sql: &str) -> Vec<sea_orm::QueryResult> {
    fx.db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [fx.ws.into()],
        ))
        .await
        .unwrap()
}

/// `(live_schema, table_name, state, last_run_id)` of the workspace's shadow map.
pub(super) async fn shadow(fx: &Fx) -> Vec<(String, String, String, String)> {
    let sql = "SELECT live_schema, table_name, state, last_run_id FROM workspace_preview_tables \
               WHERE workspace_id = $1 ORDER BY 1, 2";
    rows(fx, sql)
        .await
        .iter()
        .map(|r| {
            let get = |c: &str| r.try_get::<String>("", c).unwrap();
            (
                get("live_schema"),
                get("table_name"),
                get("state"),
                get("last_run_id"),
            )
        })
        .collect()
}

async fn step_results(fx: &Fx, run_id: &str) -> Value {
    fx.db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT results FROM agentic_workflow_state WHERE run_id = $1",
            [run_id.into()],
        ))
        .await
        .unwrap()
        .and_then(|r| r.try_get::<Option<Value>>("", "results").unwrap())
        .unwrap_or(Value::Null)
}

/// The run's writes land in `preview_<key>__toast_pos` / `__gl`, never live;
/// the next step reads the preview's copy; the registry and shadow map say
/// so; and the run report lists every redirect.
#[tokio::test]
async fn airhouse_writes_land_in_the_preview_and_reads_see_them() {
    let fx = world::world(json!([
        sql_step("add", "INSERT INTO toast_pos.orders VALUES (4, 40)"),
        sql_step(
            "roll",
            "CREATE SCHEMA IF NOT EXISTS gl; CREATE OR REPLACE TABLE gl.daily AS \
             SELECT count(*) AS n, sum(amount) AS total FROM toast_pos.orders"
        ),
        sql_step("read", "SELECT n, total FROM gl.daily"),
    ]))
    .await;
    let lake = Lake::seeded();
    let detail = run_to_the_end_with(&fx, lake.resolver(&fx, false)).await;
    let run_id = detail["run_id"].as_str().unwrap().to_string();

    assert_eq!(detail["outcome"], "succeeded", "{detail}");
    assert_eq!(detail["held_count"], 0, "{detail}");
    assert_eq!(lake.live_orders(), (3, 60), "a live table changed");
    assert!(!lake.schema_exists("gl"), "a live schema was created");
    let (orders, gl) = (schema_of(&fx, "toast_pos"), schema_of(&fx, "gl"));
    assert_eq!(
        lake.int(&format!("SELECT count(*) FROM \"{orders}\".orders")),
        4,
        "copied, then written"
    );
    assert_eq!(
        lake.int(&format!("SELECT total FROM \"{gl}\".daily")),
        100,
        "the rollup read the preview's orders"
    );
    let read = &step_results(&fx, &run_id).await["read"];
    assert_eq!(read["rows"][0][1].as_f64(), Some(100.0), "{read}");

    let steps = detail["steps"].as_array().unwrap();
    let add = &steps[0]["redirected"];
    assert_eq!(steps[0]["status"], "succeeded");
    assert_eq!(add["writes"][0]["live"], "toast_pos.orders", "{add}");
    assert_eq!(add["writes"][0]["preview"], format!("{orders}.orders"));
    assert_eq!(add["copies"][0]["state"], "shadow", "{add}");
    assert!(
        steps[2]["redirected"]["reads"]
            .to_string()
            .contains(&format!("{gl}.daily")),
        "{detail}"
    );

    let created = "SELECT schema_name FROM workspace_preview_schemas \
                   WHERE workspace_id = $1 AND schema_created_at IS NOT NULL ORDER BY 1";
    let names: Vec<String> = rows(&fx, created)
        .await
        .iter()
        .map(|r| r.try_get("", "schema_name").unwrap())
        .collect();
    assert_eq!(names, vec![gl.clone(), orders.clone()]);
    let recorded = shadow(&fx).await;
    let expected = [("gl", "daily"), ("toast_pos", "orders")];
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    for ((schema, table, state, last_run), (s, t)) in recorded.iter().zip(expected) {
        assert_eq!((schema.as_str(), table.as_str()), (s, t));
        assert_eq!((state.as_str(), last_run), ("shadow", &run_id));
    }
}

/// A schema of the preview's name that someone else made is refused by the
/// registry: the step fails, nothing reaches that schema, and nothing live.
#[tokio::test]
async fn a_write_whose_schema_the_registry_refuses_fails_the_step_not_live() {
    let fx = world::world(json!([
        sql_step("add", "INSERT INTO toast_pos.orders VALUES (4, 40)"),
        { "name": "after", "type": "formatter", "template": "not reached" }
    ]))
    .await;
    let lake = Lake::seeded();
    let theirs = schema_of(&fx, "toast_pos");
    lake.0
        .lock()
        .unwrap()
        .execute_batch(&format!(
            "CREATE SCHEMA \"{theirs}\"; CREATE TABLE \"{theirs}\".orders (id INTEGER, amount INTEGER); \
             INSERT INTO \"{theirs}\".orders VALUES (99, 990);"
        ))
        .unwrap();

    let detail = run_to_the_end_with(&fx, lake.resolver(&fx, false)).await;

    assert_eq!(detail["outcome"], "failed", "{detail}");
    assert_eq!(detail["steps"][0]["status"], "failed", "{detail}");
    assert!(
        detail["error"].to_string().contains("did not create it"),
        "{detail}"
    );
    assert_eq!(lake.live_orders(), (3, 60), "a live table changed");
    assert_eq!(
        lake.int(&format!("SELECT sum(amount) FROM \"{theirs}\".orders")),
        990,
        "the schema the preview did not make was written"
    );
    let refused = "SELECT schema_name FROM workspace_preview_schemas \
                   WHERE workspace_id = $1 AND refused_at IS NOT NULL";
    assert_eq!(rows(&fx, refused).await.len(), 1);
}

/// The shadow map is written before the step's SQL is sent: a step whose SQL
/// fails at the engine (as a crash after the review would leave it) still has
/// its relation recorded, so the TTL drop can take it.
#[tokio::test]
async fn a_step_is_recorded_before_it_is_sent() {
    let fx = world::world(json!([sql_step(
        "build",
        "CREATE OR REPLACE TABLE toast_pos.summary AS SELECT * FROM toast_pos.no_such_table"
    )]))
    .await;
    let lake = Lake::seeded();
    let detail = run_to_the_end_with(&fx, lake.resolver(&fx, false)).await;
    let run_id = detail["run_id"].as_str().unwrap().to_string();

    assert_eq!(detail["outcome"], "failed", "{detail}");
    assert!(
        detail["error"].to_string().contains("no_such_table"),
        "the engine refused it, after the review: {detail}"
    );
    let recorded = shadow(&fx).await;
    assert_eq!(
        recorded,
        vec![(
            "toast_pos".to_string(),
            "summary".to_string(),
            "shadow".to_string(),
            run_id
        )],
        "recorded before it was sent"
    );
    assert_eq!(lake.live_orders(), (3, 60));
}

/// An Airhouse that cannot confine a preview Writer (older than 0.1.49) holds
/// every write, as phase 2a did: no preview schema, no registry row, nothing
/// live — and the report says why.
#[tokio::test]
async fn an_old_airhouse_holds_every_write() {
    let fx = world::world(json!([
        sql_step("add", "INSERT INTO toast_pos.orders VALUES (4, 40)"),
        sql_step("roll", "CREATE OR REPLACE TABLE gl.daily AS SELECT 1 AS n"),
    ]))
    .await;
    let lake = Lake::seeded();
    let detail = run_to_the_end_with(&fx, lake.resolver(&fx, true)).await;

    assert_eq!(detail["outcome"], "succeeded", "{detail}");
    assert_eq!(detail["held_count"], 2, "{detail}");
    let held = &detail["steps"][0]["held"];
    assert_eq!(held["verb"], "INSERT", "{held}");
    assert!(
        held["reason"]
            .as_str()
            .unwrap()
            .starts_with("Airhouse < 0.1.49: preview writes held"),
        "{held}"
    );
    assert_eq!(lake.live_orders(), (3, 60));
    assert!(!lake.schema_exists(&schema_of(&fx, "toast_pos")));
    assert!(!lake.schema_exists("gl"));
    let registered = "SELECT 1 FROM workspace_preview_schemas WHERE workspace_id = $1";
    assert!(rows(&fx, registered).await.is_empty());
    assert!(shadow(&fx).await.is_empty());
}
