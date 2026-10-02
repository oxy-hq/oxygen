//! Two sandboxes of one app write two distinct Airhouse siblings. On the
//! host seam over one in-memory DuckDB, as `staging_airhouse_sibling` does:
//! the app's connection is the only seam replaced, and it records which
//! schema each connection was scoped to.

use std::sync::{Arc, Mutex};

use agentic_connector::{DatabaseConnector, DuckDbConnector};
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::api::custom_apps_functions::env_policy::EnvPolicy;
use oxy_app::server::api::custom_apps_functions::seam::FunctionProjectContext;
use serde_json::json;

use crate::sandbox_isolation::sandbox;
use crate::staging_homes_fixture::{APP_SCHEMA, duck, exec, host, ids, workspace};

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

/// Two sandboxes append to "the app's" table: each write lands in that
/// sandbox's own sibling, on a connection scoped to it — never in the other's,
/// in staging's, or in the app's own schema.
#[tokio::test]
async fn two_sandboxes_append_to_two_distinct_airhouse_siblings() {
    let ws = workspace(&duck("unused")).await;
    let lake = Arc::new(Lake {
        inner: ws.project.clone(),
        db: Mutex::new(duckdb::Connection::open_in_memory().expect("in-memory DuckDB")),
        asked: Mutex::new(Vec::new()),
    });
    // The fixture's app is `receiving`; a handle's hyphen becomes `_`.
    let sibling_a = format!("{APP_SCHEMA}__dev_a1");
    let sibling_b = format!("{APP_SCHEMA}__dev_b_2");
    let staging = format!("{APP_SCHEMA}__staging");
    let setup = lake.connector();
    for schema in [APP_SCHEMA, &sibling_a, &sibling_b, &staging] {
        exec(&*setup, &format!("CREATE SCHEMA {schema}")).await;
        exec(
            &*setup,
            &format!("CREATE TABLE {schema}.events (a INTEGER, note VARCHAR)"),
        )
        .await;
    }
    let as_ctx = || lake.clone() as Arc<dyn FunctionProjectContext>;
    for (handle, a) in [("a1", 1), ("b-2", 2)] {
        let policy = EnvPolicy::for_environment(sandbox(handle));
        host(as_ctx(), &[], &[], policy)
            .airhouse(
                "append".into(),
                json!({ "table": "events", "rows": [{ "a": a, "note": handle }] }),
            )
            .await
            .expect("a sandbox append");
    }
    assert_eq!(
        *lake.asked.lock().unwrap(),
        vec![sibling_a.clone(), sibling_b.clone()],
        "each connected on a credential scoped to its own sibling"
    );
    let read = lake.connector();
    assert_eq!(ids(&*read, &format!("{sibling_a}.events")).await, vec![1]);
    assert_eq!(ids(&*read, &format!("{sibling_b}.events")).await, vec![2]);
    for untouched in [APP_SCHEMA, staging.as_str()] {
        assert!(
            ids(&*read, &format!("{untouched}.events")).await.is_empty(),
            "{untouched} holds nothing a sandbox wrote"
        );
    }
}
