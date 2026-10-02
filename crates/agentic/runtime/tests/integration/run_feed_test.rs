//! The coordinator run feed (`list_runs_filtered`, `list_active_runs`) never
//! shows staff's runs to the customer, and no read a workspace's members have
//! returns a custom-app check run queued outside production.
//!
//! A held preview procedure run is saved as an ordinary `workflow` run with
//! `metadata.trigger = "preview"`; it is a staffer running an unmerged branch,
//! so it must stay out of the feed even with `include_system` (the customer's
//! own toggle). By id it stays readable: the staffer watches it there.
//!
//! A check run in `staging` or a sandbox is an ordinary `app_function` run
//! with `metadata.environment` naming the environment. It carries the
//! function's answer and error, so it is hidden from the feed **and** from
//! every read by id, the live snapshot and the recovery stats. A production
//! `app_function` run carries no environment and stays visible everywhere.
//!
//! The queue health a workspace's members read (`get_queue_stats`) is the
//! feed's rule again: a staff run's task is neither counted nor listed, in any
//! queue state — a dead sandbox check is not the tenant's dead job, and the
//! listing would hand out the run id every other route hides.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(run_feed_test)'

use agentic_runtime::crud;
use sea_orm::DatabaseConnection;
use serde_json::json;
use uuid::Uuid;

use crate::integration_tests::test_db;

struct Seeded {
    workspace: Uuid,
    customer: String,
    unlabelled: String,
    preview: String,
    /// A production `app_function` run: Run now, no environment stamped.
    production_fn: String,
    /// An `app_function` run that names production outright.
    named_production_fn: String,
    /// Check runs queued in `staging` and in a sandbox.
    staging_check: String,
    sandbox_check: String,
}

/// Root runs in a fresh workspace, all still running. Three `workflow` runs:
/// the customer's own, one with no metadata at all, and a preview dry run.
/// Four `app_function` runs: two in production and two check runs outside it.
async fn seed(db: &DatabaseConnection) -> Seeded {
    let s = Seeded {
        workspace: Uuid::new_v4(),
        customer: format!("feed-{}", Uuid::new_v4()),
        unlabelled: format!("feed-{}", Uuid::new_v4()),
        preview: format!("feed-{}", Uuid::new_v4()),
        production_fn: format!("feed-{}", Uuid::new_v4()),
        named_production_fn: format!("feed-{}", Uuid::new_v4()),
        staging_check: format!("feed-{}", Uuid::new_v4()),
        sandbox_check: format!("feed-{}", Uuid::new_v4()),
    };
    let preview = json!({ "trigger": "preview", "workflow_ref": "preview:x" });
    let rows = [
        (
            &s.customer,
            "workflow",
            Some(json!({ "trigger": "schedule" })),
        ),
        (&s.unlabelled, "workflow", None),
        (&s.preview, "workflow", Some(preview)),
        (
            &s.production_fn,
            "app_function",
            Some(json!({ "trigger": "manual" })),
        ),
        (
            &s.named_production_fn,
            "app_function",
            Some(json!({ "trigger": "manual", "environment": "production" })),
        ),
        (
            &s.staging_check,
            "app_function",
            Some(json!({ "trigger": "manual", "environment": "staging" })),
        ),
        (
            &s.sandbox_check,
            "app_function",
            Some(json!({ "trigger": "manual", "environment": "dev-a1" })),
        ),
    ];
    for (id, source_type, metadata) in rows {
        crud::insert_run(db, id, "q", None, source_type, metadata, s.workspace)
            .await
            .expect("insert_run");
    }
    s
}

fn ids(runs: Vec<agentic_runtime::entity::run::Model>) -> Vec<String> {
    let mut ids: Vec<String> = runs.into_iter().map(|r| r.id).collect();
    ids.sort();
    ids
}

/// What the customer's feed lists: their own runs and production's
/// app-function runs — no preview dry run, no check run outside production.
fn expected(s: &Seeded) -> Vec<String> {
    let mut want = vec![
        s.customer.clone(),
        s.unlabelled.clone(),
        s.production_fn.clone(),
        s.named_production_fn.clone(),
    ];
    want.sort();
    want
}

/// What a read that is not the feed returns: the feed, plus the preview dry
/// run its staffer watches by id.
fn expected_outside_the_feed(s: &Seeded) -> Vec<String> {
    let mut want = expected(s);
    want.push(s.preview.clone());
    want.sort();
    want
}

fn all_ids(s: &Seeded) -> Vec<String> {
    let mut all = expected_outside_the_feed(s);
    all.extend([s.staging_check.clone(), s.sandbox_check.clone()]);
    all
}

fn assert_no_check_run(s: &Seeded, got: &[String], read: &str) {
    for check in [&s.staging_check, &s.sandbox_check] {
        assert!(!got.contains(check), "{read} returned a check run: {got:?}");
    }
}

#[tokio::test]
async fn list_runs_filtered_hides_preview_runs() {
    let Some(db) = test_db().await else { return };
    let s = seed(&db).await;
    for include_system in [false, true] {
        let (runs, total) =
            crud::list_runs_filtered(&db, s.workspace, None, None, None, include_system, 0, 50)
                .await
                .expect("list_runs_filtered");
        assert_eq!(total, 4, "include_system={include_system}");
        let got = ids(runs);
        assert!(!got.contains(&s.preview), "preview run leaked: {got:?}");
        assert_no_check_run(&s, &got, "list_runs_filtered");
        assert_eq!(got, expected(&s), "include_system={include_system}");
    }
}

#[tokio::test]
async fn list_active_runs_hides_preview_runs() {
    let Some(db) = test_db().await else { return };
    let s = seed(&db).await;
    for include_system in [false, true] {
        let runs = crud::list_active_runs(&db, s.workspace, include_system)
            .await
            .expect("list_active_runs");
        let got = ids(runs);
        assert!(!got.contains(&s.preview), "preview run leaked: {got:?}");
        assert_no_check_run(&s, &got, "list_active_runs");
        assert_eq!(got, expected(&s), "include_system={include_system}");
    }
}

/// The filter is the environment, not the source type: asking the feed for
/// `app_function` runs lists production's and only production's.
#[tokio::test]
async fn the_feed_lists_production_app_function_runs_and_no_check_run() {
    let Some(db) = test_db().await else { return };
    let s = seed(&db).await;
    let (runs, total) = crud::list_runs_filtered(
        &db,
        s.workspace,
        None,
        Some("app_function"),
        None,
        false,
        0,
        50,
    )
    .await
    .expect("list_runs_filtered");
    let mut want = vec![s.production_fn.clone(), s.named_production_fn.clone()];
    want.sort();
    assert_eq!((ids(runs), total), (want, 2));
}

/// A run id is the caller's to type, so a check run must not be readable by
/// it: the same `None` as an id from another workspace or one that does not
/// exist. Everything else in the workspace reads back, the preview included.
#[tokio::test]
async fn a_read_by_id_finds_every_run_but_a_check_run_outside_production() {
    let Some(db) = test_db().await else { return };
    let s = seed(&db).await;
    for id in expected_outside_the_feed(&s) {
        let run = crud::get_run_in_workspace(&db, s.workspace, &id)
            .await
            .expect("get_run_in_workspace");
        assert_eq!(run.map(|r| r.id), Some(id.clone()), "{id} must read back");
        let tree = crud::load_task_tree_in_workspace(&db, s.workspace, &id)
            .await
            .expect("load_task_tree_in_workspace");
        assert_eq!(ids(tree), vec![id]);
    }
    for check in [&s.staging_check, &s.sandbox_check] {
        let run = crud::get_run_in_workspace(&db, s.workspace, check)
            .await
            .expect("get_run_in_workspace");
        assert!(run.is_none(), "a check run was read by id: {run:?}");
        let tree = crud::load_task_tree_in_workspace(&db, s.workspace, check)
            .await
            .expect("load_task_tree_in_workspace");
        assert!(tree.is_empty(), "a check run's tree was read: {tree:?}");
        // Staff's own read is unscoped and still finds it.
        let unscoped = crud::get_run(&db, check).await.expect("get_run");
        assert!(unscoped.is_some(), "the staff read lost the check run");
    }
    let foreign = crud::get_run_in_workspace(&db, Uuid::new_v4(), &s.customer)
        .await
        .expect("get_run_in_workspace");
    assert!(foreign.is_none(), "another workspace read the run");
}

/// The live snapshot and the recovery stats are counted from the same rows a
/// member may read: a failing sandbox check is not the customer's failure.
#[tokio::test]
async fn the_live_snapshot_and_the_recovery_stats_leave_check_runs_out() {
    let Some(db) = test_db().await else { return };
    let s = seed(&db).await;
    let mut live: Vec<String> = crud::runs_in_workspace(&db, s.workspace, &all_ids(&s))
        .await
        .expect("runs_in_workspace")
        .into_iter()
        .collect();
    live.sort();
    assert_no_check_run(&s, &live, "runs_in_workspace");
    assert_eq!(live, expected_outside_the_feed(&s));

    let recent = ids(crud::list_recent_runs(&db, s.workspace, 50)
        .await
        .expect("list_recent_runs"));
    assert_no_check_run(&s, &recent, "list_recent_runs");
    assert_eq!(recent, expected_outside_the_feed(&s));
}

/// A queue task for `run_id` in `status`. A `claimed` one last heartbeat an
/// hour ago, so it is stale. Scoped, so no other test's global claim loop in
/// this shared database picks it up.
async fn task_in(db: &DatabaseConnection, run_id: &str, status: &str) {
    use agentic_core::delegation::TaskSpec;
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let task_id = format!("{run_id}-{status}");
    let spec = TaskSpec::Custom {
        kind: "app_function".into(),
        payload: json!({}),
    };
    crud::enqueue_task(
        db,
        &task_id,
        run_id,
        None,
        &spec,
        None,
        crud::TaskScope::Scoped,
    )
    .await
    .expect("enqueue_task");
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET queue_status = $2, last_heartbeat = now() - interval '1 hour' \
         WHERE task_id = $1",
        [task_id.into(), status.into()],
    ))
    .await
    .expect("set the task's status");
}

/// Queue health is what a workspace member sees of the queue. For each queue
/// state, one production app-function task and one staff task sit side by
/// side: only production's is counted, and only production's run id is listed.
#[tokio::test]
async fn queue_health_neither_counts_nor_lists_a_staff_runs_task() {
    let Some(db) = test_db().await else { return };
    let s = seed(&db).await;
    for (production, staff) in [
        (&s.production_fn, &s.staging_check),
        (&s.named_production_fn, &s.sandbox_check),
        (&s.customer, &s.preview),
    ] {
        for status in ["queued", "claimed", "dead"] {
            task_in(&db, production, status).await;
            task_in(&db, staff, status).await;
        }
    }

    let stats = crud::get_queue_stats(&db, s.workspace)
        .await
        .expect("get_queue_stats");
    assert_eq!(
        (stats.queued, stats.claimed, stats.dead),
        (3, 3, 3),
        "one task per production run in each state, and none of staff's"
    );
    let mut want = vec![
        s.production_fn.clone(),
        s.named_production_fn.clone(),
        s.customer.clone(),
    ];
    want.sort();
    for (listed, tasks) in [("stale", stats.stale_tasks), ("dead", stats.dead_tasks)] {
        let mut runs: Vec<String> = tasks.into_iter().map(|t| t.run_id).collect();
        runs.sort();
        assert_eq!(runs, want, "{listed} tasks list production's runs only");
    }
}
