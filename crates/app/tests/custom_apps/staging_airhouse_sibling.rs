//! Staging's `ctx.airhouse` writes land in the app schema's sibling,
//! `app_<writer>__staging`, and never in the app's own schema; its reads stay
//! on production's — through the real host, on a real engine (environments
//! design §8, the Airhouse half of the differential test).
//!
//! One in-memory DuckDB plays the workspace's Airhouse. The app's connection
//! is the only seam replaced, and it records which schema each connection was
//! scoped to, so a test sees both where a statement landed and which
//! credential it would have gone out on.

use std::sync::{Arc, Mutex};

use agentic_connector::{DatabaseConnector, DuckDbConnector};
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::api::custom_apps_functions::seam::FunctionProjectContext;
use serde_json::json;

use crate::staging_homes_fixture::{
    APP_SCHEMA, SIBLING, Workspace, duck, exec, host, ids, production, staging, workspace,
};

/// The workspace's Airhouse, played by one in-memory DuckDB. Every connection
/// the host asks for is recorded with the schema it was to be scoped to.
struct Lake {
    inner: Arc<OxyProjectContext>,
    db: Mutex<duckdb::Connection>,
    asked: Mutex<Vec<String>>,
}

impl Lake {
    fn connector(&self) -> Arc<dyn DatabaseConnector> {
        let conn = self.db.lock().unwrap().try_clone().expect("clone");
        Arc::new(DuckDbConnector::new(conn))
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
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

async fn lake() -> (Arc<Lake>, Workspace) {
    let ws = workspace(&duck("unused")).await;
    let lake = Arc::new(Lake {
        inner: ws.project.clone(),
        db: Mutex::new(duckdb::Connection::open_in_memory().expect("in-memory DuckDB")),
        asked: Mutex::new(Vec::new()),
    });
    let setup = lake.connector();
    for schema in [APP_SCHEMA, SIBLING] {
        exec(&*setup, &format!("CREATE SCHEMA {schema}")).await;
        exec(
            &*setup,
            &format!("CREATE TABLE {schema}.events (a INTEGER, note VARCHAR)"),
        )
        .await;
    }
    (lake, ws)
}

/// Appends and execs from staging land in the sibling; production's schema
/// holds production's alone; each ran on a connection scoped to where it wrote.
#[tokio::test]
async fn a_staging_airhouse_write_lands_in_the_sibling_and_never_in_the_apps_schema() {
    let (lake, _ws) = lake().await;
    let as_ctx = || lake.clone() as Arc<dyn FunctionProjectContext>;
    let stg = host(as_ctx(), &[], &[], staging(&[]));
    stg.airhouse(
        "append".into(),
        json!({ "table": "events", "rows": [{ "a": 1, "note": "append" }] }),
    )
    .await
    .expect("staging append");
    stg.airhouse(
        "exec".into(),
        json!({ "sql": format!(
            "INSERT INTO {APP_SCHEMA}.events SELECT a + 1, 'exec' FROM {APP_SCHEMA}.events"
        ) }),
    )
    .await
    .expect("staging exec");
    assert_eq!(lake.asked(), vec![SIBLING.to_string()]);

    let prod = host(as_ctx(), &[], &[], production(&[]));
    prod.airhouse(
        "append".into(),
        json!({ "table": "events", "rows": [{ "a": 100, "note": "append" }] }),
    )
    .await
    .expect("production append");

    let read = lake.connector();
    assert_eq!(
        ids(&*read, &format!("{SIBLING}.events")).await,
        vec![1, 2],
        "the exec read the sibling's own rows, not production's"
    );
    assert_eq!(
        ids(&*read, &format!("{APP_SCHEMA}.events")).await,
        vec![100],
        "production's schema holds production's write alone"
    );
    assert_eq!(
        lake.asked(),
        vec![SIBLING.to_string(), APP_SCHEMA.to_string()]
    );
}

/// `ctx.airhouse.query` in staging reads production's schema, on the
/// connection scoped to it — the same code runs in both environments.
#[tokio::test]
async fn a_staging_airhouse_query_reads_production() {
    let (lake, _ws) = lake().await;
    exec(
        &*lake.connector(),
        &format!("INSERT INTO {APP_SCHEMA}.events VALUES (99, 'production')"),
    )
    .await;
    let stg = host(
        lake.clone() as Arc<dyn FunctionProjectContext>,
        &[],
        &[],
        staging(&[]),
    );
    let answer = stg
        .airhouse(
            "query".into(),
            json!({ "sql": format!("SELECT a FROM {APP_SCHEMA}.events") }),
        )
        .await
        .expect("staging query");
    assert_eq!(answer["rows"][0]["a"], 99, "{answer}");
    assert_eq!(lake.asked(), vec![APP_SCHEMA.to_string()]);
}

/// Staging refuses what production refuses — naming the sibling directly
/// included, since only the app's own schema is its to name.
#[tokio::test]
async fn a_staging_airhouse_write_production_would_refuse_is_refused() {
    let (lake, _ws) = lake().await;
    let stg = host(
        lake.clone() as Arc<dyn FunctionProjectContext>,
        &[],
        &[],
        staging(&[]),
    );
    for sql in [
        format!("INSERT INTO {SIBLING}.events VALUES (1, 'x')"),
        "INSERT INTO other_app.events VALUES (1, 'x')".to_string(),
    ] {
        stg.airhouse("exec".into(), json!({ "sql": sql }))
            .await
            .expect_err("refused as in production");
    }
    assert!(lake.asked().is_empty(), "nothing connected");
}
