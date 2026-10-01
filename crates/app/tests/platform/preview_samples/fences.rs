//! Fix round 1: a sample whose load would write airway's metadata into `main`
//! is refused at submit, and a sandbox naming an app-scoped rotator — as only
//! a write behind the save could leave it — never runs.

use std::sync::{Arc, Mutex};

use agentic_core::delegation::{TaskAssignment, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use axum::http::StatusCode;
use oxy_app::server::previews::airhouse_duckdb::DuckDbAirhouse;
use oxy_app::server::previews::sample::{PREVIEW_AIRWAY_SAMPLE_KIND, PreviewAirwaySampleExecutor};
use serde_json::json;

use super::{count, submit, world};
use crate::preview_routes::fixture::{exec, pipeline};

/// SHOULD-FIX 4: `replacing` tables fold through `main._aw_compaction_manifest`
/// and `main._aw_table_watermarks`, which a preview Writer cannot write.
#[tokio::test]
async fn a_sample_whose_load_writes_main_is_unsupported() {
    let fx = world().await;
    let events = json!({ "name": "events_api",
        "source": { "kind": "rest_api", "config": { "base_url": "https://api.example.test",
            "endpoints": [
                { "name": "events", "path": "/events", "write_disposition": "replacing",
                  "primary_key": ["id"] },
                { "name": "venues", "path": "/venues" } ] } },
        "destination": { "database": "airhouse", "dataset_name": "raw_events" } });
    pipeline(&fx.db, fx.staging, "airway/events.airway.yml", events).await;
    let (status, body) = submit(
        &fx,
        "airway/events.airway.yml",
        json!({ "resources": ["events"] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "sample_unsupported");
    assert!(
        body["message"].as_str().unwrap().contains("`main`"),
        "{body}"
    );
    let (status, body) = submit(
        &fx,
        "airway/events.airway.yml",
        json!({ "resources": ["venues"] }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "the append resource samples: {body}"
    );
}

/// BLOCKING (run time): a sources row naming `apps/<id>/…` — Pokehouse's
/// production rotator's grant — written behind the save: the sample's platform
/// is never built, nothing resolves, nothing is leased or written.
#[tokio::test]
async fn an_app_scoped_rotator_registered_behind_the_save_never_runs() {
    let fx = world().await;
    let app_token = format!("apps/{}/QB_REFRESH_TOKEN_EASTBAY", uuid::Uuid::new_v4());
    exec(
        &fx.db,
        "INSERT INTO workspace_preview_sources (workspace_id, pipeline_name, environment, overrides) \
         VALUES ($1, 'qb_eastbay', 'sandbox', $2)",
        vec![
            fx.ws.into(),
            json!({ "realm_id": "4620816365000000", "refresh_token_var": app_token,
                    "client_secret_var": "QB_SANDBOX_SECRET" })
            .into(),
        ],
    )
    .await;
    let (status, body) = submit(&fx, "airway/qb.airway.yml", json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let conn = Arc::new(Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
    let executor = PreviewAirwaySampleExecutor {
        db: fx.db.clone(),
        airhouse: Arc::new(DuckDbAirhouse::new(conn).unwrap()),
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
        Ok(_) => panic!("a sandbox naming an app-scoped token must never run"),
        Err(why) => why,
    };
    assert!(why.contains("reserved"), "{why}");
    for sql in [
        "SELECT count(*) AS n FROM airway_pipeline_leases WHERE workspace_id = $1",
        "SELECT count(*) AS n FROM workspace_preview_schemas WHERE workspace_id = $1",
    ] {
        assert_eq!(count(&fx, sql, vec![fx.ws.into()]).await, 0, "{sql}");
    }
}
