//! A parent whose children have all reported is continued **once**: by the
//! resume that carries their answer.
//!
//! When the last child of a delegation finishes, its outcome is on record
//! before the parent is resumed. A driver that dies in between leaves a parent
//! parked at its delegation with every answer already in. Recovery rebuilds
//! the coordinator, which finds such a parent and resumes it with the
//! aggregated answer (`process_pending_resumes`). The tree walk used to
//! re-launch the same parent from its suspension as well — a second
//! continuation of the run, and one that carried an empty answer.
//!
//! Three shapes, because the walk reaches the parent through different arms:
//! a `delegating` root (a queued run a latency worker picks up), a
//! `needs_resume` root (a chat run after `cleanup_stale_runs` has stamped it at
//! boot), and an automation root, where both continuations are decision tasks
//! and the second one delegates the finished step again.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(children_done_resume_test)'

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agentic_core::delegation::TaskSpec;
use agentic_core::human_input::SuspendedRunData;
use agentic_runtime::crud;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::automation_recovery_test::test_db;
use crate::suspended_root_requeue_test::{
    PROBE_KIND, Probe, assert_still_parked, checkpoint, latency_pass, requeue_with, seed,
    startup_pass, wait_for_the_workers,
};

const CHILD_ANSWER: &str = "store 7 sold the most";

/// The metadata a coordinator writes on a parent waiting on `children`.
fn waiting_on(children: &[&str]) -> Value {
    json!({
        "child_task_ids": children,
        "completed": {},
        "failure_policy": "fail_fast",
    })
}

/// Park `run_id` on one child that has finished and reported `CHILD_ANSWER`:
/// the state between the child's outcome and the parent's resume.
async fn park_on_a_finished_child(db: &DatabaseConnection, run_id: &str) {
    let child = format!("{run_id}.1");
    crud::suspend_with_data_txn(
        db,
        run_id,
        "delegating",
        Some(waiting_on(&[&child])),
        "Delegation to weekly report",
        &[],
        &checkpoint("delegating"),
    )
    .await
    .expect("park the run");
    crud::insert_child_run(db, &child, run_id, "weekly report", "analytics", 0, None)
        .await
        .expect("insert child");
    crud::complete_child_done_txn(db, &child, &child, run_id, CHILD_ANSWER)
        .await
        .expect("finish the child");
}

/// A root no queue entry describes: how an interactive run is driven.
async fn seed_unqueued(db: &DatabaseConnection, ws: Uuid) -> String {
    let run_id = Uuid::new_v4().to_string();
    crud::insert_run(db, &run_id, "q", None, PROBE_KIND, None, ws)
        .await
        .expect("insert run");
    run_id
}

async fn payloads(db: &DatabaseConnection, run_id: &str, event_type: &str) -> Vec<Value> {
    crud::get_all_events(db, run_id)
        .await
        .expect("events")
        .into_iter()
        .filter(|e| e.event_type == event_type)
        .map(|e| e.payload)
        .collect()
}

/// Wait for the worker to claim `run_id`'s entry, or for the run to be closed
/// with its entry never claimed (which is how the walk's own re-launch ends
/// for a run this test cannot actually resume).
async fn wait_until_claimed_or_retired(db: &DatabaseConnection, run_id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if !payloads(db, run_id, "worker_task_claimed").await.is_empty() {
            break;
        }
        let entry = crud::get_queue_entry(db, run_id).await.expect("entry");
        let run = crud::get_run(db, run_id).await.unwrap().expect("run");
        let closed = run.task_status.as_deref() == Some("failed");
        let unclaimable = entry.is_none_or(|e| e.queue_status == "cancelled");
        if (closed && unclaimable) || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Whatever was going to be written about this run has been.
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// The root got one continuation, and it is the resume carrying its
/// children's answer.
///
/// A probe-kind run has no pipeline behind it, so it ends `failed` whoever
/// resumes it. What differs is who tried, and with what: the coordinator's
/// resume writes `input_resolved` with the answer and queues a `Resume` the
/// worker claims; the walk's re-launch does neither, and fails the recovery
/// before a coordinator exists.
async fn assert_resumed_once_with_the_answer(db: &DatabaseConnection, probe: &Probe, run_id: &str) {
    let resolved = payloads(db, run_id, "input_resolved").await;
    assert_eq!(
        resolved.len(),
        1,
        "the parent is handed its children's answer exactly once, got {resolved:?}"
    );
    assert_eq!(resolved[0]["answer"], CHILD_ANSWER);

    let entry = crud::get_queue_entry(db, run_id)
        .await
        .unwrap()
        .expect("the resume is a queue entry");
    match serde_json::from_value::<TaskSpec>(entry.spec).expect("spec") {
        TaskSpec::Resume {
            answer,
            resume_data,
            ..
        } => {
            assert_eq!(answer, CHILD_ANSWER, "the resume carries the answer");
            assert_eq!(
                serde_json::to_value(resume_data).unwrap(),
                serde_json::to_value(checkpoint("delegating")).unwrap(),
                "and resumes from the checkpoint the parent is at"
            );
        }
        other => panic!("the root's entry should be its resume, got {other:?}"),
    }
    assert_eq!(
        payloads(db, run_id, "worker_task_claimed").await.len(),
        1,
        "the worker ran that resume once"
    );
    assert!(!probe.started(run_id), "a resume is not a fresh start");
}

/// A queued run parked on its children when its driver died, picked up by a
/// latency worker with the last child's answer already in.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegating_root_whose_children_finished_is_resumed_once() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let root = seed(&db, ws).await;
    park_on_a_finished_child(&db, &root).await;

    latency_pass(&db, ws, &probe).await;
    wait_until_claimed_or_retired(&db, &root).await;

    assert_resumed_once_with_the_answer(&db, &probe, &root).await;
}

/// The chat shape. An interactive root has no queue entry, and the boot that
/// follows its driver's death stamps it `needs_resume` (`cleanup_stale_runs`)
/// before the startup pass selects it — so the walk meets it as "was running",
/// not as `delegating`.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_root_whose_children_finished_before_the_restart_is_resumed_once() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let root = seed_unqueued(&db, ws).await;
    park_on_a_finished_child(&db, &root).await;
    crud::update_task_status(&db, &root, "needs_resume", None)
        .await
        .expect("stamp the run as the boot cleanup does");

    startup_pass(&db, ws, &probe).await;
    wait_until_claimed_or_retired(&db, &root).await;

    assert_resumed_once_with_the_answer(&db, &probe, &root).await;
}

/// The root *was* resumed with its children's answer, and its driver died
/// while it was finishing: its entry is that `Resume`, handed back `queued`.
/// The entry is the continuation. The coordinator rebuilt from the rows still
/// sees "every child in" and must not assign the resume a second time over the
/// one the worker is claiming.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_whose_resume_is_already_queued_is_not_resumed_again() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let root = seed_unqueued(&db, ws).await;
    park_on_a_finished_child(&db, &root).await;
    crud::update_task_status(&db, &root, "needs_resume", None)
        .await
        .expect("stamp the run as the boot cleanup does");
    requeue_with(
        &db,
        &root,
        &TaskSpec::Resume {
            run_id: root.clone(),
            resume_data: checkpoint("delegating"),
            answer: CHILD_ANSWER.into(),
        },
    )
    .await;

    startup_pass(&db, ws, &probe).await;
    wait_until_claimed_or_retired(&db, &root).await;

    assert_eq!(
        payloads(&db, &root, "input_resolved").await,
        Vec::<Value>::new(),
        "the resume was assigned before the crash; recovery must not assign it again"
    );
    assert_eq!(
        payloads(&db, &root, "worker_task_claimed").await.len(),
        1,
        "the worker ran the queued resume once"
    );
    assert!(!probe.started(&root), "a resume is not a fresh start");
}

/// A root on its *second* delegation, whose child has not finished. The first
/// delegation's outcome is still on record under the same parent; it does not
/// make the second one complete.
#[tokio::test(flavor = "multi_thread")]
async fn an_earlier_delegations_answer_does_not_finish_the_current_one() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let control = seed(&db, ws).await;
    let root = seed(&db, ws).await;
    park_on_a_finished_child(&db, &root).await;
    // Resumed, and delegated again: one child, parked on a person so nothing
    // in the tree moves by itself.
    let second = format!("{root}.2");
    crud::suspend_with_data_txn(
        &db,
        &root,
        "delegating",
        Some(waiting_on(&[&second])),
        "Delegation to monthly report",
        &[],
        &checkpoint("delegating again"),
    )
    .await
    .expect("park the run");
    crud::insert_child_run(&db, &second, &root, "monthly report", "analytics", 0, None)
        .await
        .expect("insert child");
    crud::update_task_status(&db, &second, "awaiting_input", None)
        .await
        .expect("park the child");

    latency_pass(&db, ws, &probe).await;
    wait_for_the_workers(&probe, &control).await;

    assert_eq!(
        payloads(&db, &root, "input_resolved").await,
        Vec::<Value>::new(),
        "the parent was resumed while its child is still running"
    );
    assert_still_parked(&db, &probe, &root, "delegating").await;
}

/// Every child in, and a checkpoint this binary cannot read (a rollback past
/// a `SuspendedRunData` change). No resume can be built, so nothing will ever
/// hand the parent its answer: the run is closed as interrupted now, not left
/// `delegating` until the suspend ceiling four hours later.
#[tokio::test(flavor = "multi_thread")]
async fn children_done_with_no_readable_checkpoint_closes_the_run() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let root = seed_unqueued(&db, ws).await;
    park_on_a_finished_child(&db, &root).await;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "UPDATE agentic_run_suspensions SET resume_data = '{}'::jsonb WHERE run_id = $1",
        [root.clone().into()],
    ))
    .await
    .expect("make the checkpoint unreadable");

    startup_pass(&db, ws, &probe).await;

    let run = crud::get_run(&db, &root).await.unwrap().expect("run");
    assert_eq!(
        run.task_status.as_deref(),
        Some("failed"),
        "a parent nothing can resume must not stay parked"
    );
    let error = run.error_message.unwrap_or_default();
    assert!(
        error.contains("has no checkpoint"),
        "closed as interrupted, got: {error}"
    );
    assert_eq!(
        payloads(&db, &root, "input_resolved").await,
        Vec::<Value>::new()
    );
    assert!(!probe.started(&root), "and it is never run from the top");
}

// ── An automation root: the step must not run twice ─────────────────────────

fn one_step_automation() -> agentic_automation::AutomationConfig {
    agentic_automation::AutomationConfig {
        name: "weekly".into(),
        tasks: vec![agentic_automation::config::TaskConfig {
            name: "step0".into(),
            task_type: agentic_automation::config::TaskType::Unknown,
            export: None,
            cache: None,
        }],
        description: String::new(),
        variables: None,
        consistency_prompt: None,
        consistency_model: None,
    }
}

/// An automation parked on its only step, whose child has finished and
/// reported: the decision that delegated the step is committed, the one that
/// folds its answer has not run.
async fn seed_automation_parked_on_a_finished_step(db: &DatabaseConnection, ws: Uuid) -> String {
    let root = format!("wf-{}", Uuid::new_v4());
    let child = format!("{root}.1");
    crud::insert_run(db, &root, "weekly", None, "workflow", None, ws)
        .await
        .expect("insert root");
    crud::suspend_with_data_txn(
        db,
        &root,
        "delegating",
        Some(waiting_on(&[&child])),
        "Executing step: step0",
        &[],
        &SuspendedRunData {
            from_state: "workflow_decision".into(),
            original_input: "weekly".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({ "step_name": "step0", "step_index": 0 }),
            question: "Executing step: step0".into(),
            suggestions: vec![],
        },
    )
    .await
    .expect("park the root");
    let state = agentic_automation::extension::AutomationRunState {
        run_id: root.clone(),
        workflow: one_step_automation(),
        workflow_yaml_hash: "hash".into(),
        workflow_context: json!({}),
        variables: None,
        trace_id: "trace-1".into(),
        current_step: 0,
        results: HashMap::new(),
        render_context: json!({}),
        pending_children: HashMap::from([("0".to_string(), vec![child.clone()])]),
        decision_version: 0,
        step_hashes: HashMap::new(),
        retry_from_run_id: None,
        cache_enabled: false,
        prior_step_hashes: HashMap::new(),
        prior_results: HashMap::new(),
        initial_render_context: json!({}),
        invalidate_iterations: HashMap::new(),
    };
    agentic_automation::extension::insert_automation_state(db, &state)
        .await
        .expect("insert automation state");
    crud::insert_child_run(db, &child, &root, "step0", "workflow_step", 0, None)
        .await
        .expect("insert child");
    crud::complete_child_done_txn(db, &child, &child, &root, r#"{"text":"step0 result"}"#)
        .await
        .expect("finish the child");
    root
}

async fn decision_version(db: &DatabaseConnection, run_id: &str) -> i64 {
    agentic_automation::extension::load_automation_state(db, run_id)
        .await
        .expect("load state")
        .expect("state")
        .decision_version
}

/// For an automation the continuation is a decision task, and it must be the
/// one that carries the child's answer. A decision made without it — which is
/// what the walk's re-launch was — sees step 0 as not yet run and delegates it
/// again, so the step executes twice.
#[tokio::test(flavor = "multi_thread")]
async fn an_automation_whose_step_finished_makes_one_more_decision() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let root = seed_automation_parked_on_a_finished_step(&db, ws).await;
    let decisions_before = decision_version(&db, &root).await;

    startup_pass(&db, ws, &probe).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let run = crud::get_run(&db, &root).await.unwrap().expect("run");
        if matches!(run.task_status.as_deref(), Some("done") | Some("failed")) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the automation never finished: {:?}",
            run.task_status
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;

    let tree = crud::load_task_tree(&db, &root).await.expect("tree");
    assert_eq!(
        tree.len(),
        2,
        "the finished step was not delegated a second time: {:?}",
        tree.iter()
            .map(|t| (&t.id, &t.task_status))
            .collect::<Vec<_>>()
    );
    let decisions = payloads(&db, &root, "decider_decided").await;
    assert_eq!(
        decisions.len(),
        1,
        "one decision — the one that folds the step's answer — got {decisions:?}"
    );
    let state = agentic_automation::extension::load_automation_state(&db, &root)
        .await
        .expect("load state")
        .expect("state");
    assert_eq!(state.decision_version, decisions_before + 1);
    assert_eq!(state.results["step0"], json!({ "text": "step0 result" }));
    let run = crud::get_run(&db, &root).await.unwrap().expect("run");
    assert_eq!(run.task_status.as_deref(), Some("done"));
}
