//! The loop again, for the app's own database: on an app with an OLTP store,
//! in an org with a staging branch, each sandbox works in a schema of its own
//! inside that branch (`internal-docs/per-org-oltp-postgres.md` → Sandbox
//! schemas on the staging branch).
//!
//! By route where the loop is by route — create, show, delete — with the
//! publish, the queued schema task and the teardown run as production runs
//! them. In order:
//!
//! - both sandboxes are created and published to, and `GET …/environments`
//!   shows each one's `oltp_schema` go from absent to `ready`;
//! - a row written in one is read back there, and seen by neither the other,
//!   staging, nor production;
//! - one is deleted and torn down: its schema is gone, the other's and
//!   staging's rows are intact;
//! - the freed name, created and published to again, starts from staging's
//!   rows — it inherits nothing the old sandbox wrote.
//!
//! **Needs** Postgres only: `LocalProvider` cuts the branch on the test
//! cluster.

use agentic_core::delegation::TaskOutcome;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::sandbox_oltp_isolation::{
    functions, publish, run_in, run_oltp_tasks_done, schema_of, with_two_sandboxes,
};
use crate::sandbox_routes::send;
use crate::sandbox_teardown_task::{run_queued, use_scratch_homes};
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

const IDS: [&str; 3] = ["ids", "query", "select id from orders order by id"];

fn ids(got: &Value) -> Vec<i64> {
    let rows = got["ids"]["ok"]
        .as_array()
        .unwrap_or_else(|| panic!("{got}"));
    rows.iter()
        .map(|row| row["id"].as_i64().expect("an id"))
        .collect()
}

async fn shown(app_id: uuid::Uuid, name: &str) -> Value {
    let (status, body) = send("GET", &format!("{app_id}/environments/{name}"), None).await;
    assert_eq!(status, StatusCode::OK, "{name}: {body}");
    body
}

#[tokio::test]
async fn the_loop_keeps_each_sandboxs_app_database_apart() {
    use_scratch_homes();
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let row = with_two_sandboxes(&app).await;
        let db = &app.t.db;

        // Created with no schema; a publish queues it; the task makes it.
        assert_eq!(shown(row.id, "dev-a1").await["oltp_schema"], Value::Null);
        publish(&app, "a1", "loop-a", &[]).await;
        publish(&app, "b2", "loop-b", &[]).await;
        run_oltp_tasks_done(db, row.id).await;
        for handle in ["a1", "b2"] {
            let oltp = shown(row.id, &format!("dev-{handle}")).await["oltp_schema"].clone();
            assert_eq!(oltp["status"], "ready", "{oltp}");
            assert_eq!(oltp["schema"], schema_of(&app, handle).name(), "{oltp}");
            assert_eq!(oltp["structure_only"], json!([]), "{oltp}");
        }
        assert_eq!(shown(row.id, "staging").await["oltp_schema"], Value::Null);

        // A write in each lands in its own copy.
        let write = |id: i64| format!("insert into orders (id) values ({id})");
        let (in_a, in_b) = (write(100), write(200));
        assert_eq!(
            ids(&run_in(&app, "dev-a1", &[["w", "exec", &in_a], IDS]).await),
            [1, 100]
        );
        assert_eq!(
            ids(&run_in(&app, "dev-b2", &[["w", "exec", &in_b], IDS]).await),
            [1, 200]
        );
        assert_eq!(ids(&run_in(&app, "staging", &[IDS]).await), [1]);
        assert_eq!(ids(&run_in(&app, "production", &[IDS]).await), [1]);

        // Deleted by route, torn down by the executor production registers.
        let (status, deleting) =
            send("DELETE", &format!("{}/environments/dev-a1", row.id), None).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{deleting}");
        let run_id = deleting["teardown_run_id"]
            .as_str()
            .expect("a teardown run");
        let outcome = run_queued(db, run_id).await;
        let TaskOutcome::Done { answer, .. } = outcome else {
            panic!("the teardown failed: {outcome:?}");
        };
        assert!(answer.contains("OLTP schema dropped"), "{answer}");
        assert_eq!(
            ids(&run_in(&app, "dev-b2", &[IDS]).await),
            [1, 200],
            "dev-b2 is intact"
        );
        assert_eq!(ids(&run_in(&app, "staging", &[IDS]).await), [1]);

        // The freed name starts from staging's rows, not the old sandbox's.
        let (status, again) = send(
            "POST",
            &format!("{}/environments", row.id),
            Some(json!({ "name": "dev-a1" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "the name is free: {again}");
        assert_eq!(again["oltp_schema"], Value::Null, "and records no schema");
        publish(&app, "a1", "loop-a2", &[]).await;
        run_oltp_tasks_done(db, row.id).await;
        assert_eq!(
            ids(&run_in(&app, "dev-a1", &[IDS]).await),
            [1],
            "nothing inherited"
        );
    })
    .await;
}
