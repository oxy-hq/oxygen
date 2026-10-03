//! `/admin/internal-jobs/*` — the task queue. A queue row has no org column; its
//! tenant is the org of the workspace its run belongs to, and its payload is that
//! tenant's.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use oxy_app::server::api::admin::internal_jobs::{
    DeadLetterQuery, LimitQuery, delete_dead, list_dead_letter, list_workers, queue_stats,
    recent_failures, reenqueue_dead, run_reaper, run_retention,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use uuid::Uuid;

use super::fixture::{World, as_actor, reply, world};

/// A task on the queue in `status`, belonging (through its run) to `workspace`.
async fn task(
    db: &DatabaseConnection,
    workspace: Uuid,
    status: &str,
    worker: Option<&str>,
) -> String {
    let id = format!("task-{}", Uuid::new_v4());
    agentic_runtime::crud::runs::insert_run(
        db,
        &id,
        "compile main",
        None,
        "compile",
        None,
        workspace,
    )
    .await
    .expect("seed run");
    let spec = TaskSpec::Compile {
        workspace_id: workspace,
        git_sha: None,
        branch: None,
        promote: false,
        kind: Some("main".to_string()),
        owner_user_id: None,
    };
    agentic_runtime::crud::queue::enqueue_task(db, &id, &id, None, &spec, None, TaskScope::Global)
        .await
        .expect("enqueue task");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET queue_status = $2, worker_id = $3, \
             claimed_at = CASE WHEN $3::text IS NULL THEN NULL ELSE now() END, \
             updated_at = now() \
         WHERE task_id = $1",
        [
            id.clone().into(),
            status.into(),
            worker.map(str::to_string).into(),
        ],
    ))
    .await
    .expect("set task status");
    id
}

#[derive(FromQueryResult)]
struct StatusRow {
    queue_status: String,
}

async fn status_of(db: &DatabaseConnection, task_id: &str) -> Option<String> {
    StatusRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT queue_status FROM agentic_task_queue WHERE task_id = $1",
        [task_id.into()],
    ))
    .one(db)
    .await
    .expect("read task status")
    .map(|r| r.queue_status)
}

struct Dead {
    a: String,
    b: String,
    /// Its run's workspace belongs to no org.
    orphan: String,
    /// Its run names a workspace that does not exist — a system job.
    system: String,
}

/// One dead task per owner. Org A's is the OLDEST, so a listing that pages before it
/// narrows shows the bounded caller someone else's row first.
async fn seed_dead(w: &World) -> Dead {
    Dead {
        a: task(&w.db, w.ws_a, "dead", None).await,
        b: task(&w.db, w.ws_b, "dead", None).await,
        orphan: task(&w.db, w.ws_orphan, "dead", None).await,
        system: task(&w.db, Uuid::new_v4(), "dead", None).await,
    }
}

/// The finding, on internal jobs: task payloads, run errors and the originating
/// user of every tenant's failed work.
#[tokio::test]
async fn a_bounded_grant_lists_only_its_own_orgs_failed_and_dead_tasks() {
    let w = world().await;
    let dead = seed_dead(&w).await;

    let failures =
        reply(recent_failures(as_actor(&w.bounded), Query(LimitQuery::default())).await).await;
    assert_eq!(failures.status, StatusCode::OK);
    assert_eq!(
        failures.column(None, "task_id"),
        vec![dead.a.clone()],
        "another tenant's (or a platform-level) failed task leaked"
    );
    assert!(
        !failures.body.to_string().contains(&w.ws_b.to_string()),
        "org B's task payload reached a grant bounded to org A"
    );

    let letters =
        reply(list_dead_letter(as_actor(&w.bounded), Query(DeadLetterQuery::default())).await)
            .await;
    assert_eq!(
        letters.column(Some("rows"), "task_id"),
        vec![dead.a.clone()]
    );
    assert_eq!(
        letters.body["total"], 1,
        "the dead-letter total counted other tenants' tasks"
    );

    // In the query, ahead of LIMIT: org A's task is the oldest of the four.
    let first = reply(
        list_dead_letter(
            as_actor(&w.bounded),
            Query(DeadLetterQuery {
                limit: Some(1),
                offset: None,
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        first.column(Some("rows"), "task_id"),
        vec![dead.a.clone()],
        "a page of one must be org A's task, not the newest task on the queue"
    );
}

/// Counts leak too: the queue depth and the fleet's in-flight load are every
/// tenant's volume.
#[tokio::test]
async fn a_bounded_grants_queue_stats_and_workers_count_only_its_own_orgs_tasks() {
    let w = world().await;
    task(&w.db, w.ws_a, "claimed", Some("worker-a")).await;
    task(&w.db, w.ws_b, "claimed", Some("worker-b")).await;
    task(&w.db, w.ws_b, "dead", None).await;
    task(&w.db, w.ws_orphan, "failed", None).await;

    let stats = reply(queue_stats(as_actor(&w.bounded)).await).await;
    assert_eq!(stats.status, StatusCode::OK);
    assert_eq!(stats.body["total"]["claimed"], 1, "{}", stats.body);
    assert_eq!(stats.body["total"]["dead"], 0, "{}", stats.body);
    assert_eq!(stats.body["total"]["failed"], 0, "{}", stats.body);

    let workers = reply(list_workers(as_actor(&w.bounded)).await).await;
    assert_eq!(
        workers.column(Some("workers"), "worker_id"),
        vec!["worker-a".to_string()],
        "a worker that only ever touched org B's tasks is listed"
    );
    assert_eq!(workers.body["workers"][0]["inflight_count"], 1);

    for (who, actor) in w.everything_readers() {
        let stats = reply(queue_stats(as_actor(actor)).await).await;
        assert_eq!(stats.body["total"]["claimed"], 2, "{who}");
        assert_eq!(stats.body["total"]["dead"], 1, "{who}");
        assert_eq!(stats.body["total"]["failed"], 1, "{who}");
        let workers = reply(list_workers(as_actor(actor)).await).await;
        assert_eq!(
            workers.column(Some("workers"), "worker_id").len(),
            2,
            "{who}"
        );
    }
}

/// By id: out of scope is **not found** — the same answer, status and body, a task
/// id that does not exist gets — and nothing is written.
#[tokio::test]
async fn a_bounded_grant_cannot_reenqueue_or_delete_another_orgs_dead_task() {
    let w = world().await;
    let dead = seed_dead(&w).await;

    let missing =
        reply(reenqueue_dead(as_actor(&w.bounded), Path("task-does-not-exist".into())).await).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    for id in [&dead.b, &dead.orphan, &dead.system] {
        let revived = reply(reenqueue_dead(as_actor(&w.bounded), Path(id.clone())).await).await;
        assert_eq!(revived.status, StatusCode::NOT_FOUND, "re-enqueued {id}");
        assert_eq!(
            revived.body, missing.body,
            "an out-of-scope task must be indistinguishable from a missing one"
        );
        let deleted = reply(delete_dead(as_actor(&w.bounded), Path(id.clone())).await).await;
        assert_eq!(deleted.status, StatusCode::NOT_FOUND, "deleted {id}");
        assert_eq!(
            status_of(&w.db, id).await.as_deref(),
            Some("dead"),
            "task {id} was written to by a grant that does not reach it"
        );
    }

    // Its own org's task is still its to act on.
    let revived = reply(reenqueue_dead(as_actor(&w.bounded), Path(dead.a.clone())).await).await;
    assert_eq!(revived.status, StatusCode::OK, "{}", revived.body);
    assert_eq!(status_of(&w.db, &dead.a).await.as_deref(), Some("queued"));
}

/// A fleet-wide action reaches every tenant at once, so it is platform-level.
#[tokio::test]
async fn a_bounded_grant_cannot_run_the_fleet_wide_reaper_or_retention_sweep() {
    let w = world().await;

    let reaper = reply(run_reaper(as_actor(&w.bounded)).await).await;
    assert_eq!(reaper.status, StatusCode::NOT_FOUND);
    let retention = reply(run_retention(as_actor(&w.bounded)).await).await;
    assert_eq!(retention.status, StatusCode::NOT_FOUND);

    for (who, actor) in w.everything_readers() {
        assert_eq!(
            reply(run_reaper(as_actor(actor)).await).await.status,
            StatusCode::OK,
            "{who}"
        );
        assert_eq!(
            reply(run_retention(as_actor(actor)).await).await.status,
            StatusCode::OK,
            "{who}"
        );
    }
}

/// Control: unbounded staff see and act on every tenant's tasks and the
/// platform-level ones, exactly as before.
#[tokio::test]
async fn unbounded_staff_see_and_act_on_every_task_including_platform_level() {
    let w = world().await;
    let dead = seed_dead(&w).await;
    let all = [&dead.a, &dead.b, &dead.orphan, &dead.system];

    for (who, actor) in w.everything_readers() {
        let failures =
            reply(recent_failures(as_actor(actor), Query(LimitQuery::default())).await).await;
        let ids = failures.column(None, "task_id");
        for id in all {
            assert!(ids.contains(id), "{who} no longer lists task {id}");
        }
        let letters =
            reply(list_dead_letter(as_actor(actor), Query(DeadLetterQuery::default())).await).await;
        assert_eq!(letters.body["total"], 4, "{who}");
    }

    let revived = reply(reenqueue_dead(as_actor(&w.unbounded), Path(dead.b.clone())).await).await;
    assert_eq!(revived.status, StatusCode::OK, "{}", revived.body);
    let deleted = reply(delete_dead(as_actor(&w.owner), Path(dead.system.clone())).await).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(status_of(&w.db, &dead.system).await, None);
}
