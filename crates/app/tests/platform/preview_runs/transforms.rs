//! Phase 2b S10 end to end: a branch changes a pure-Airhouse transform; its
//! change check lists it `auto` and queues a `transform_build`; the build runs
//! (real driver, DuckDB standing in for Airhouse) and writes the preview's
//! copies; a compare is queued once; it runs and the build's run report shows
//! counts, no values.

use std::sync::{Arc, Mutex};

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use axum::http::StatusCode;
use oxy_app::agentic_wiring::preview_airhouse::PreviewAirhousePorts;
use oxy_app::server::previews::airhouse_duckdb::DuckDbAirhouse;
use oxy_app::server::previews::analyze::{PREVIEW_ANALYZE_KIND, PreviewAnalyzeExecutor};
use oxy_app::server::previews::compare::{PREVIEW_COMPARE_KIND, PreviewCompareExecutor};
use oxy_app::server::previews::namespace::preview_key;
use oxy_app::server::previews::runtime::PreviewRunResolver;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

use super::world::{self, PROCEDURE, drive_with, enable_runs};
use crate::preview_routes::fixture::{BRANCH, exec, send_json};

const LIVE: &str = "CREATE SCHEMA toast_pos; \
    CREATE TABLE toast_pos.orders (id INTEGER, amount INTEGER); \
    INSERT INTO toast_pos.orders VALUES (1, 10), (2, 20), (3, 30); \
    CREATE TABLE toast_pos.orders_copy AS SELECT * FROM toast_pos.orders; \
    CREATE TABLE toast_pos.sales_daily AS SELECT count(*) AS n, sum(amount) AS total \
        FROM toast_pos.orders;";

fn step(name: &str, sql: &str) -> Value {
    json!({ "name": name, "type": "execute_sql", "database": "airhouse", "sql_query": sql })
}

/// Run a Custom task's executor by hand and record its `Done`, as the
/// coordinator would.
async fn run_custom(db: &DatabaseConnection, exec: &dyn TaskExecutor, kind: &str, run_id: &str) {
    let mut task = exec
        .execute(TaskAssignment {
            task_id: run_id.into(),
            parent_task_id: None,
            run_id: run_id.into(),
            spec: TaskSpec::Custom {
                kind: kind.into(),
                payload: json!({ "preview_run_id": run_id }),
            },
            policy: None,
        })
        .await
        .expect("the executor accepts its own kind");
    let TaskOutcome::Done { answer, metadata } = task.outcomes.recv().await.expect("an outcome")
    else {
        panic!("{kind} did not finish");
    };
    agentic_runtime::crud::update_run_done(db, run_id, &answer, metadata)
        .await
        .expect("record the outcome");
}

#[tokio::test]
async fn a_changed_transform_is_built_in_the_preview_and_compared_with_live() {
    let old = step(
        "sales",
        "CREATE OR REPLACE TABLE toast_pos.sales_daily AS \
         SELECT count(*) AS n, sum(amount) AS total FROM toast_pos.orders",
    );
    let fx = world::world(json!([old])).await;
    let new = json!({ "name": "je", "tasks": [
        step("copy", "CREATE OR REPLACE TABLE toast_pos.orders_copy AS SELECT * FROM toast_pos.orders"),
        step("sales", "CREATE OR REPLACE TABLE toast_pos.sales_daily AS SELECT count(*) AS n, \
              sum(amount) AS total, max(amount) AS top FROM toast_pos.orders WHERE amount > 10"),
    ]});
    exec(
        &fx.db,
        "UPDATE automation_definitions SET definition = $1 WHERE revision_id = $2 AND file_path = $3",
        vec![new.into(), fx.staging.into(), PROCEDURE.into()],
    )
    .await;
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute_batch(LIVE).unwrap();
    let lake = Arc::new(Mutex::new(conn));
    let ports =
        || -> Arc<dyn PreviewAirhousePorts> { DuckDbAirhouse::new(lake.clone()).unwrap().shared() };
    enable_runs(true);
    // An earlier staff run left the preview holding `orders_copy`: the build
    // does not reset it, and its compare flags the table preexisting.
    exec(
        &fx.db,
        "INSERT INTO workspace_preview_tables \
             (workspace_id, preview_key, live_schema, table_name, state, last_run_id) \
         VALUES ($1, $2, 'toast_pos', 'orders_copy', 'shadow', 'an-earlier-run')",
        vec![fx.ws.into(), preview_key(fx.ws, BRANCH).into()],
    )
    .await;

    // The check lists the transform and queues its build.
    let check =
        oxy_app::server::previews::analyze::ensure_enqueued(&fx.db, fx.ws, BRANCH, fx.staging)
            .await
            .unwrap()
            .unwrap();
    let analyze = PreviewAnalyzeExecutor {
        db: fx.db.clone(),
        airhouse: ports(),
    };
    run_custom(&fx.db, &analyze, PREVIEW_ANALYZE_KIND, &check).await;
    let (status, checks) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/checks?branch=feat%2Fje-v2", fx.ws),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{checks}");
    let transform = &checks["transforms"][0];
    assert_eq!(
        (&transform["file_path"], &transform["build"]),
        (&json!(PROCEDURE), &json!("auto")),
        "{checks}"
    );
    let build = transform["build_run_id"]
        .as_str()
        .expect("queued")
        .to_string();

    // The build runs in the preview; its compare is queued once.
    let resolver = Arc::new(PreviewRunResolver::with_airhouse(fx.db.clone(), ports()));
    assert_eq!(drive_with(&fx, &build, resolver).await, "done");
    let queued = oxy_app::server::previews::maintenance::enqueue_compares(&fx.db)
        .await
        .unwrap();
    assert_eq!(queued.len(), 1, "one compare per build");
    assert!(
        oxy_app::server::previews::maintenance::enqueue_compares(&fx.db)
            .await
            .unwrap()
            .is_empty(),
        "and only one"
    );
    let compare = PreviewCompareExecutor {
        db: fx.db.clone(),
        airhouse: ports(),
    };
    run_custom(&fx.db, &compare, PREVIEW_COMPARE_KIND, &queued[0]).await;

    let (status, detail) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/runs/{build}", fx.ws),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["kind"], "transform_build");
    assert_eq!(detail["parent_run_id"], json!(check));
    let cmp = &detail["compare"];
    assert_eq!(cmp["outcome"], "succeeded", "{detail}");
    let tables = cmp["tables"].as_array().expect("tables");
    assert_eq!(tables.len(), 2, "{cmp}");
    let (copy, sales) = (&tables[0], &tables[1]);
    assert_eq!(copy["table"], "toast_pos.orders_copy");
    assert_eq!(copy["equal"], true, "{copy}");
    assert_eq!(copy["preexisting"], true, "{copy}");
    assert_eq!(sales["table"], "toast_pos.sales_daily");
    assert_eq!(sales["equal"], false, "{sales}");
    assert_eq!(sales["preexisting"], false, "{sales}");
    let caveats = cmp["caveats"].to_string();
    assert!(
        caveats.contains("inputs' age") && caveats.contains("preexisting"),
        "{cmp}"
    );
    assert_eq!(sales["columns_added"], json!(["top"]));
    assert_eq!(
        (&sales["only_in_preview"], &sales["only_in_live"]),
        (&json!(1), &json!(1)),
        "{sales}"
    );
    assert!(
        !cmp.to_string().contains("\"50\"") && !cmp.to_string().contains(":50"),
        "no values: {cmp}"
    );

    let (_, list) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/runs?branch=feat%2Fje-v2", fx.ws),
        None,
    )
    .await;
    let kinds: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["kind"].as_str())
        .collect();
    assert_eq!(kinds, vec!["compare", "transform_build"], "{list}");
}
