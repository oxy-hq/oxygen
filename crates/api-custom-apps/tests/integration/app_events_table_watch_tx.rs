//! A `ctx.tx` write to a database a function is woken by is held to the same
//! rules as a direct write (app events design §7), through the real host on a
//! real Postgres (`staging_homes_fixture`; no V8): the loop guard refuses a
//! write to the table that woke the run, on the statement; the tracker is
//! nudged once the transaction has committed; a rollback nudges nothing.

use std::sync::Arc;

use oxy_api_custom_apps::custom_apps_functions::host::into_arc;
use oxy_api_custom_apps::custom_apps_functions::runtime::{EventCause, FunctionHost};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::json;
use uuid::Uuid;

use crate::app_events_table_watch::{Tw, with_tw, world};
use crate::staging_homes_fixture::{exec, host_in, production, workspace};
use crate::warehouse_writes_on_engines::postgres_entry;

/// The table the function in these tests is woken by.
const WOKEN_BY: &str = "table:pg:public.t";

/// A host for `tw`'s org and workspace, run by a change to `pg:public.t`, over
/// a Postgres `pg` holding empty tables `t` and `u`.
async fn host_woken_by_t(tw: &Tw) -> (Arc<dyn FunctionHost>, impl Sized) {
    let ws = workspace(&postgres_entry("pg").await).await;
    for table in ["t", "u"] {
        let create = format!("CREATE TABLE {table} (a INTEGER PRIMARY KEY)");
        exec(&*ws.connector("pg").await, &create).await;
    }
    let cause = EventCause {
        id: Uuid::new_v4(),
        event_type: WOKEN_BY.into(),
        root_id: Uuid::new_v4(),
        depth: 0,
        actor: None,
    };
    let host = host_in(
        tw.t.db.clone(),
        (tw.t.org_id, tw.workspace),
        ws.ctx(),
        &["pg"],
        &["pg"],
        production(&[]),
    )
    .with_event_cause(Some(cause));
    (into_arc(host), ws)
}

/// Park every track task the org has queued, so the next one is the nudge's.
async fn park_queued_tracks(db: &DatabaseConnection, org: Uuid) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status = 'dead' \
          WHERE task_id LIKE $1 AND queue_status = 'queued'",
        [format!("app_table_watch:{org}:%").into()],
    ))
    .await
    .expect("park the queued passes");
}

async fn queued_tracks(db: &DatabaseConnection, org: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT COUNT(*) AS n FROM agentic_task_queue \
          WHERE task_id LIKE $1 AND queue_status = 'queued'",
        [format!("app_table_watch:{org}:%").into()],
    ))
    .await
    .expect("count")
    .expect("a row")
    .try_get("", "n")
    .expect("n")
}

/// An org with a table watch (so a nudge has a cursor to queue a pass for).
async fn with_a_watch<F, Fut>(body: F)
where
    F: FnOnce(Arc<Tw>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    with_tw(|tw| async move {
        let _ = world(|_| {});
        tw.watcher(
            &format!("{}-a", tw.slug()),
            json!({ "table": "wh:bookkeeping.lines" }),
            "export default async () => Response.json({});",
            json!({}),
        )
        .await;
        body(tw).await;
    })
    .await;
}

#[tokio::test]
async fn a_tx_write_to_the_table_that_woke_the_run_is_refused_and_another_is_not() {
    with_a_watch(|tw| async move {
        let (host, _ws) = host_woken_by_t(&tw).await;
        let id = host
            .tx("begin".into(), json!({ "database": "pg" }))
            .await
            .expect("begin")["id"]
            .clone();
        let refused = host
            .tx(
                "exec".into(),
                json!({ "id": id, "sql": "INSERT INTO public.t VALUES (1)" }),
            )
            .await
            .expect_err("a write to the woken-by table");
        assert!(refused.starts_with("EventLoop:"), "{refused}");
        // Refused before it was sent: the handle goes on, and another table is
        // not the loop's.
        host.tx(
            "exec".into(),
            json!({ "id": id, "sql": "INSERT INTO public.u VALUES (1)" }),
        )
        .await
        .expect("another table");
        host.tx("rollback".into(), json!({ "id": id }))
            .await
            .expect("rollback");
    })
    .await;
}

#[tokio::test]
async fn a_committed_tx_write_nudges_the_tracker_and_a_rolled_back_one_does_not() {
    with_a_watch(|tw| async move {
        let (host, _ws) = host_woken_by_t(&tw).await;
        let (db, org) = (&tw.t.db, tw.org());
        let write =
            |id: &serde_json::Value| json!({ "id": id, "sql": "INSERT INTO public.u VALUES (1)" });

        park_queued_tracks(db, org).await;
        let id = host
            .tx("begin".into(), json!({ "database": "pg" }))
            .await
            .expect("begin")["id"]
            .clone();
        host.tx("exec".into(), write(&id)).await.expect("write");
        assert_eq!(queued_tracks(db, org).await, 0, "nothing before the commit");
        host.tx("rollback".into(), json!({ "id": id }))
            .await
            .expect("rollback");
        assert_eq!(queued_tracks(db, org).await, 0, "a rollback nudges nothing");

        let id = host
            .tx("begin".into(), json!({ "database": "pg" }))
            .await
            .expect("begin")["id"]
            .clone();
        host.tx("exec".into(), write(&id)).await.expect("write");
        host.tx("commit".into(), json!({ "id": id }))
            .await
            .expect("commit");
        // One pass per environment the workspace tracks.
        assert!(queued_tracks(db, org).await > 0, "the commit nudges");
    })
    .await;
}
