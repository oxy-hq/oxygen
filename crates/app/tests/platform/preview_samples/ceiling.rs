//! The run ceiling and an Airway sample: a sample past the ceiling that still
//! holds its lease is asked to cancel and left to wind down — retiring it
//! would free its lease for the next sample of the pipeline while its engine
//! still loads — and is retired once the lease drops, or past the wind-down.

use agentic_airway::extension::pipeline_lease::{self, LeaseAcquisition};
use oxy_app::server::previews::runs::retire_overdue;
use serde_json::json;

use super::{ORDERS, count, submit, world};
use crate::preview_routes::fixture::{BRANCH, exec};

async fn overdue_sample(fx: &crate::preview_routes::fixture::Fx, minutes: i64) -> String {
    let (_, body) = submit(fx, ORDERS, json!({ "resources": ["orders"] })).await;
    assert_eq!(body["state"], "running", "{body}");
    let run_id = body["run_id"].as_str().unwrap().to_string();
    exec(
        &fx.db,
        "UPDATE workspace_preview_runs SET started_at = now() - make_interval(mins => $2) \
         WHERE run_id = $1",
        vec![run_id.clone().into(), (minutes as i32).into()],
    )
    .await;
    let key = oxy_app::server::previews::namespace::preview_key(fx.ws, BRANCH);
    let name = format!("preview:{key}:orders_api");
    let held = pipeline_lease::try_acquire(&fx.db, fx.ws, &name, &run_id, 3600)
        .await
        .unwrap();
    assert_eq!(held, LeaseAcquisition::Acquired);
    run_id
}

async fn state(fx: &crate::preview_routes::fixture::Fx, run_id: &str) -> String {
    super::one(
        fx,
        "SELECT state FROM workspace_preview_runs WHERE run_id = $1",
        vec![run_id.into()],
    )
    .await
    .unwrap()
    .try_get("", "state")
    .unwrap()
}

#[tokio::test]
async fn an_overdue_sample_holding_its_lease_is_left_to_wind_down() {
    let fx = world().await;
    let run_id = overdue_sample(&fx, 61).await;

    assert!(
        retire_overdue(&fx.db, 60).await.unwrap().is_empty(),
        "left to wind down"
    );
    assert_eq!(state(&fx, &run_id).await, "running");
    let failed = "SELECT count(*) AS n FROM agentic_runs WHERE id = $1 AND task_status = 'failed'";
    assert_eq!(count(&fx, failed, vec![run_id.clone().into()]).await, 0);

    // The engine let go of its lease: now it is retired.
    pipeline_lease::release_by_run(&fx.db, &run_id)
        .await
        .unwrap();
    assert_eq!(
        retire_overdue(&fx.db, 60).await.unwrap(),
        vec![run_id.clone()]
    );
    assert_eq!(state(&fx, &run_id).await, "finished");
    assert_eq!(count(&fx, failed, vec![run_id.into()]).await, 1);

    // One still holding its lease past the wind-down is retired all the same.
    let stuck = overdue_sample(&fx, 60 + 16).await;
    assert_eq!(retire_overdue(&fx.db, 60).await.unwrap(), vec![stuck]);
}
