//! Airway samples of a previewed branch's pipelines (phase 2b S11), through the
//! route, the queue and the worker-fleet executor, against the previews
//! fixture's workspace. No Airhouse that can confine a Writer is configured
//! here, so a sample that reached its destination is refused there — which is
//! itself one of the cases (nothing written). The engine's own path is covered
//! by `agentic-pipeline`'s `preview_airway_sample_test`. Database-backed
//! (`Schema::All`).

mod ceiling;
mod fences;

use std::sync::{Arc, Mutex};

use agentic_core::delegation::{TaskAssignment, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use axum::http::StatusCode;
use oxy_app::server::previews::airhouse_duckdb::DuckDbAirhouse;
use oxy_app::server::previews::sample::{
    PREVIEW_AIRWAY_SAMPLE_KIND, PreviewAirwaySampleExecutor, Recorded, record_sample,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{Value, json};

use crate::preview_routes::fixture::{BRANCH, Fx, exec, pipeline, send_json, setup};

pub(crate) const ORDERS: &str = "airway/orders.airway.yml";

fn rest_api(name: &str, endpoints: &[&str], database: &str, dataset: &str) -> Value {
    let endpoints: Vec<Value> = endpoints
        .iter()
        .map(|e| json!({ "name": e, "path": format!("/{e}") }))
        .collect();
    json!({ "name": name,
            "source": { "kind": "rest_api",
                        "config": { "base_url": "https://api.example.test", "endpoints": endpoints } },
            "destination": { "database": database, "dataset_name": dataset } })
}

/// The previews fixture, runs on, and four pipelines on the branch: a two-
/// resource API into the managed Airhouse, a QuickBooks one, an SP-API one,
/// and an API landing in ClickHouse.
pub(crate) async fn world() -> Fx {
    let fx = setup().await;
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_PREVIEW_RUNS", "1") };
    let databases = json!([
        { "name": "clickhouse", "type": "clickhouse", "host": "http://127.0.0.1:1",
          "user": "default", "database": "default" },
        { "name": "airhouse", "type": "airhouse_managed" }
    ]);
    exec(
        &fx.db,
        "UPDATE workspace_compiled_configs SET databases = $1 WHERE revision_id = $2",
        vec![databases.into(), fx.staging.into()],
    )
    .await;
    let rev = fx.staging;
    pipeline(
        &fx.db,
        rev,
        ORDERS,
        rest_api(
            "orders_api",
            &["orders", "customers"],
            "airhouse",
            "raw_orders",
        ),
    )
    .await;
    pipeline(
        &fx.db,
        rev,
        "airway/ch.airway.yml",
        rest_api("to_clickhouse", &["x"], "clickhouse", "raw_x"),
    )
    .await;
    let qb = json!({ "name": "qb_eastbay", "source": { "kind": "quickbooks", "config": {
        "client_id": "c", "client_secret_var": "QB_S", "refresh_token_var": "QB_R", "realm_id": "1" } },
        "destination": { "database": "airhouse", "dataset_name": "quickbooks" } });
    pipeline(&fx.db, rev, "airway/qb.airway.yml", qb).await;
    let sp = json!({ "name": "amazon", "source": { "kind": "sp_api", "config": {} },
        "destination": { "database": "airhouse", "dataset_name": "amazon" } });
    pipeline(&fx.db, rev, "airway/amazon.airway.yml", sp).await;
    fx
}

pub(crate) async fn submit(fx: &Fx, target_ref: &str, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({ "branch": BRANCH, "kind": "airway_sample", "ref": target_ref });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    send_json(
        &fx.staff,
        "POST",
        format!("/{}/previews/runs", fx.ws),
        Some(body),
    )
    .await
}

pub(crate) async fn one(
    fx: &Fx,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Option<sea_orm::QueryResult> {
    fx.db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await
        .expect(sql)
}

pub(crate) async fn count(fx: &Fx, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    one(fx, sql, values)
        .await
        .unwrap()
        .try_get("", "n")
        .unwrap()
}

#[tokio::test]
async fn a_sample_is_refused_or_queued_by_the_submit_rules() {
    let fx = world().await;
    for (target, extra, status, code) in [
        (
            "airway/amazon.airway.yml",
            json!({}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "sample_refused",
        ),
        (
            "airway/qb.airway.yml",
            json!({}),
            StatusCode::CONFLICT,
            "sandbox_required",
        ),
        (
            ORDERS,
            json!({}),
            StatusCode::BAD_REQUEST,
            "resources_required",
        ),
        (
            "airway/ch.airway.yml",
            json!({}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "sample_refused",
        ),
        (
            ORDERS,
            json!({ "resources": ["orders"], "window": {} }),
            StatusCode::BAD_REQUEST,
            "window_not_supported",
        ),
        (
            "airway/none.airway.yml",
            json!({}),
            StatusCode::NOT_FOUND,
            "ref_not_in_revision",
        ),
    ] {
        let (got, body) = submit(&fx, target, extra.clone()).await;
        assert_eq!(
            (got, body["code"].as_str()),
            (status, Some(code)),
            "{target} {extra}: {body}"
        );
    }
    let refused = "SELECT count(*) AS n FROM workspace_preview_runs WHERE workspace_id = $1";
    assert_eq!(
        count(&fx, refused, vec![fx.ws.into()]).await,
        0,
        "nothing refused was queued"
    );

    let (status, body) = submit(&fx, ORDERS, json!({ "resources": ["orders"] })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["state"], "running", "the queue was free");
    let run_id = body["run_id"].as_str().unwrap().to_string();

    let row = one(
        &fx,
        "SELECT kind, options FROM workspace_preview_runs WHERE run_id = $1",
        vec![run_id.clone().into()],
    )
    .await
    .unwrap();
    assert_eq!(row.try_get::<String>("", "kind").unwrap(), "airway_sample");
    let options: Value = row.try_get("", "options").unwrap();
    assert_eq!(options["pipeline_name"], "orders_api");
    assert_eq!(options["dataset_name"], "raw_orders");
    assert_eq!(options["resources"], json!(["orders"]));
    assert_eq!(options["wall_clock_capped"], true);

    let run = one(
        &fx,
        "SELECT source_type, metadata FROM agentic_runs WHERE id = $1",
        vec![run_id.clone().into()],
    )
    .await
    .expect("the run was seeded with the queue step");
    assert_eq!(
        run.try_get::<String>("", "source_type").unwrap(),
        PREVIEW_AIRWAY_SAMPLE_KIND
    );
    let metadata: Value = run.try_get("", "metadata").unwrap();
    assert_eq!(
        metadata["trigger"], "preview",
        "workspace health never counts it"
    );
    let queued = agentic_runtime::crud::get_queue_entry(&fx.db, &run_id)
        .await
        .unwrap()
        .expect("queued");
    let spec: TaskSpec = serde_json::from_value(queued.spec).unwrap();
    assert!(
        matches!(&spec, TaskSpec::Custom { kind, payload }
            if kind == PREVIEW_AIRWAY_SAMPLE_KIND && payload["preview_run_id"] == run_id.as_str()),
        "{spec:?}"
    );

    let (status, detail) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/runs/{run_id}", fx.ws),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["kind"], "airway_sample");
    assert_eq!(detail["sample"]["pipeline"], "orders_api");
    assert_eq!(detail["sample"]["resources"], json!(["orders"]));
}

/// P4/I4: with no Airhouse that confines a preview Writer (older than 0.1.49),
/// the sample is refused at its destination with the reason, and nothing is
/// written: no schema, no registry row, no lease, no state, no audit, no table.
#[tokio::test]
async fn no_scoped_writer_refuses_the_sample_and_writes_nothing() {
    let fx = world().await;
    let (status, body) = submit(&fx, ORDERS, json!({ "resources": ["orders"] })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let conn = Arc::new(Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
    let executor = PreviewAirwaySampleExecutor {
        db: fx.db.clone(),
        airhouse: Arc::new(DuckDbAirhouse::old(conn).unwrap()),
    };
    let started = executor
        .execute(TaskAssignment {
            task_id: run_id.clone(),
            parent_task_id: None,
            run_id: run_id.clone(),
            spec: TaskSpec::Custom {
                kind: PREVIEW_AIRWAY_SAMPLE_KIND.into(),
                payload: json!({ "preview_run_id": run_id }),
            },
            policy: None,
        })
        .await;
    let why = match started {
        Ok(_) => panic!("an Airhouse that cannot confine a Writer lands no sample"),
        Err(why) => why,
    };
    assert!(why.starts_with("Airhouse < 0.1.49"), "{why}");
    assert!(why.contains("nothing was written"), "{why}");

    let ws = || vec![fx.ws.into()];
    for (what, sql) in [
        (
            "registry",
            "SELECT count(*) AS n FROM workspace_preview_schemas WHERE workspace_id = $1",
        ),
        (
            "tables",
            "SELECT count(*) AS n FROM workspace_preview_tables WHERE workspace_id = $1",
        ),
        (
            "leases",
            "SELECT count(*) AS n FROM airway_pipeline_leases WHERE workspace_id = $1",
        ),
        (
            "state",
            "SELECT count(*) AS n FROM airway_workspace_pipeline_state \
             WHERE workspace_id = $1 AND pipeline_name LIKE 'preview:%'",
        ),
        (
            "audit",
            "SELECT count(*) AS n FROM airway_load_audit WHERE workspace_id = $1",
        ),
    ] {
        assert_eq!(count(&fx, sql, ws()).await, 0, "{what}");
    }
}

fn schema(name: &str, tables: Value) -> Value {
    json!({ "name": name, "version": 1, "version_hash": "", "engine_version": 1, "tables": tables })
}

fn column(name: &str, data_type: &str) -> Value {
    json!({ "name": name, "data_type": data_type })
}

/// After a sample: its stored schema compared with production's, and its
/// tables — with the `_raw` buffer of a `replacing` one — recorded in the
/// preview's shadow map as `sample`.
#[tokio::test]
async fn a_finished_sample_is_compared_with_live_and_its_tables_recorded() {
    let fx = setup().await;
    let key = oxy_app::server::previews::namespace::preview_key(fx.ws, BRANCH);
    let live = schema(
        "orders_api",
        json!({ "orders": { "name": "orders",
        "columns": { "id": column("id", "big_int") }, "write_disposition": "append" } }),
    );
    let sampled = schema(
        "orders_api",
        json!({
        "orders": { "name": "orders", "write_disposition": "append",
                    "columns": { "id": column("id", "big_int"), "note": column("note", "text") } },
        "refunds": { "name": "refunds", "write_disposition": "replacing",
                     "columns": { "id": column("id", "big_int") } } }),
    );
    let insert = "INSERT INTO airway_workspace_pipeline_state (workspace_id, pipeline_name, state, schema_json) \
                  VALUES ($1, $2, '{}'::jsonb, $3)";
    exec(
        &fx.db,
        insert,
        vec![fx.ws.into(), "orders_api".into(), live.into()],
    )
    .await;
    let preview_name = format!("preview:{key}:orders_api");
    exec(
        &fx.db,
        insert,
        vec![fx.ws.into(), preview_name.clone().into(), sampled.into()],
    )
    .await;

    let at = Recorded {
        workspace_id: fx.ws,
        preview_key: &key,
        run_id: "sample-run",
        pipeline: "orders_api",
        dataset: Some("raw_orders"),
    };
    let report = record_sample(&fx.db, &at).await.expect("recorded");
    assert_eq!(report.preview_pipeline, preview_name);
    assert_eq!(
        report.tables,
        vec!["orders".to_string(), "refunds".to_string()]
    );
    assert!(report.compared_with_live);
    let kinds: Vec<(&str, &str)> = report
        .findings
        .iter()
        .map(|f| (f.kind.as_str(), f.detail.as_str()))
        .collect();
    assert!(kinds.contains(&("ColumnAdded", "orders.note")), "{kinds:?}");
    assert!(kinds.contains(&("TableAdded", "refunds")), "{kinds:?}");

    let rows = fx
        .db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT live_schema || '.' || table_name || ':' || state AS t FROM workspace_preview_tables \
             WHERE workspace_id = $1 AND preview_key = $2 ORDER BY 1",
            [fx.ws.into(), key.clone().into()],
        ))
        .await
        .unwrap();
    let recorded: Vec<String> = rows.iter().map(|r| r.try_get("", "t").unwrap()).collect();
    assert_eq!(
        recorded,
        vec![
            "raw_orders.orders:sample",
            "raw_orders.refunds:sample",
            "raw_orders_raw.refunds:sample"
        ]
    );
}
