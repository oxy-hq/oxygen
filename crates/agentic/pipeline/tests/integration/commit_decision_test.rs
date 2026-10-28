//! Atomicity tests for [`agentic_automation::extension::commit_decision`].
//!
//! `commit_decision` is the single atomic write-point for an automation decision
//! boundary. It replaces the ad-hoc sequence `update_automation_state` → emit
//! events → mark queue completed, which was observably non-atomic: a silent
//! failure anywhere in that sequence stranded an automation in a half-committed
//! state (state advanced, events/queue never written).
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(commit_decision_test)'

use std::collections::HashMap;

use agentic_automation::extension::{
    AutomationRunState, CommitOutcome, DecisionClaim, DecisionCommit, DecisionTerminal,
    commit_decision, insert_automation_state, load_automation_state,
};
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{Database, DatabaseConnection};
use serde_json::{Value, json};
use uuid::Uuid;

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    std::sync::Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    std::sync::Arc::new(
                        Postgres::default()
                            .with_tag("18-alpine")
                            // 64 MB (Docker default) is too small: a parallel plan wants a 32 MB
                            // DSM segment and a REUSED container accumulates them.
                            // Must match at every setup site — reuse hashes the config.
                            // See internal-docs/workspace-source.md.
                            .with_shm_size(1024 * 1024 * 1024)
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("failed to start Postgres testcontainer"),
                    )
                })
                .await;
            let port = container.get_host_port_ipv4(5432_u16).await.unwrap();
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    let mut db = None;
    for attempt in 0..10 {
        match Database::connect(&url).await {
            Ok(conn) => {
                db = Some(conn);
                break;
            }
            Err(e) if attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                eprintln!("test_db: attempt {attempt} failed: {e}");
            }
            Err(e) => panic!("failed to connect after 10 retries: {e}"),
        }
    }
    let db = db.unwrap();

    // Central then runtime (production order — see oxy_test_utils::migration).
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .then::<agentic_analytics::extension::AnalyticsMigrator>()
        .await
        .expect("analytics migrations failed")
        .then::<agentic_automation::AutomationMigrator>()
        .await
        .expect("automation migrations failed")
        .finish()
        .await;
    Some(db)
}

// ── Fixture helpers ──────────────────────────────────────────────────────────

/// Build a two-step automation with a delegated SQL step followed by an inline
/// formatter. Mirrors the real-world shape where a non-atomic commit stranded
/// the automation between the two steps.
fn two_step_automation() -> agentic_automation::AutomationConfig {
    agentic_automation::AutomationConfig {
        name: "atomic_test".to_string(),
        tasks: vec![
            agentic_automation::config::TaskConfig {
                name: "step_sql".to_string(),
                task_type: agentic_automation::config::TaskType::Unknown,
            },
            agentic_automation::config::TaskConfig {
                name: "step_fmt".to_string(),
                task_type: agentic_automation::config::TaskType::Unknown,
            },
        ],
        description: String::new(),
        variables: None,
        consistency_prompt: None,
        consistency_model: None,
    }
}

async fn seed_run(db: &DatabaseConnection) -> (String, AutomationRunState) {
    let run_id = format!("wf-commit-{}", Uuid::new_v4());
    crud::insert_run(
        db,
        &run_id,
        "test commit_decision",
        None,
        "workflow",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert run");

    let state = AutomationRunState {
        run_id: run_id.clone(),
        workflow: two_step_automation(),
        workflow_yaml_hash: "hash".to_string(),
        workflow_context: json!({"workspace_path": "/tmp"}),
        variables: None,
        trace_id: "trace".to_string(),
        current_step: 0,
        results: HashMap::new(),
        render_context: json!({}),
        pending_children: HashMap::new(),
        decision_version: 0,
        step_hashes: HashMap::new(),
        retry_from_run_id: None,
        cache_enabled: false,
        prior_step_hashes: HashMap::new(),
        prior_results: HashMap::new(),
        initial_render_context: json!({}),
        invalidate_iterations: HashMap::new(),
    };
    insert_automation_state(db, &state)
        .await
        .expect("insert automation state");
    (run_id, state)
}

/// Enqueue a decision task row so tests can observe it being marked terminal.
async fn seed_queue_row(db: &DatabaseConnection, task_id: &str, run_id: &str) {
    use agentic_core::delegation::TaskSpec;
    crud::enqueue_task(
        db,
        task_id,
        run_id,
        None,
        &TaskSpec::AutomationDecision {
            run_id: run_id.to_string(),
            pending_child_answer: None,
        },
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue decision task");
}

async fn event_types(db: &DatabaseConnection, run_id: &str) -> Vec<String> {
    crud::get_all_events(db, run_id)
        .await
        .expect("read events")
        .into_iter()
        .map(|e| e.event_type)
        .collect()
}

async fn run_task_status(db: &DatabaseConnection, run_id: &str) -> Option<String> {
    crud::get_run(db, run_id)
        .await
        .unwrap()
        .unwrap()
        .task_status
}

async fn queue_status(db: &DatabaseConnection, task_id: &str) -> Option<String> {
    crud::get_queue_entry(db, task_id)
        .await
        .unwrap()
        .map(|m| m.queue_status)
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Continuing terminal: state CAS'd, events persisted with monotonic seq, run
/// + queue rows untouched. This models the common case of "decider advanced
/// the automation to the next suspended step".
#[tokio::test(flavor = "multi_thread")]
async fn continuing_commits_state_and_events_atomically() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;

    // Decider-produced patch: advance past step 0, store its result.
    state.current_step = 1;
    state.results.insert("step_sql".into(), json!({"rows": 3}));

    let events: Vec<(String, Value)> = vec![
        (
            "subrun_step_completed".into(),
            json!({"step": "step_sql", "success": true}),
        ),
        ("subrun_step_started".into(), json!({"step": "step_fmt"})),
    ];

    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::Unclaimed,
            expected_version: 0,
            new_state: state,
            result_delta: json!({"step_sql": {"rows": 3}}),
            step_hash_delta: json!({}),
            events,
            attempt: 0,
            terminal: DecisionTerminal::Continuing,
        },
    )
    .await
    .expect("commit_decision");

    assert!(matches!(outcome, CommitOutcome::Committed));

    // State CAS'd: version advanced to 1, current_step == 1.
    let loaded = load_automation_state(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(loaded.decision_version, 1);
    assert_eq!(loaded.current_step, 1);
    assert!(loaded.results.contains_key("step_sql"));

    // Events persisted in order with monotonic seq starting at 0.
    let events = crud::get_all_events(&db, &run_id).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].seq, 0);
    assert_eq!(events[0].event_type, "subrun_step_completed");
    assert_eq!(events[1].seq, 1);
    assert_eq!(events[1].event_type, "subrun_step_started");

    // Continuing terminal leaves queue & run untouched.
    assert_eq!(
        queue_status(&db, &decision_task_id).await.as_deref(),
        Some("queued")
    );
    assert_eq!(
        run_task_status(&db, &run_id).await.as_deref(),
        Some("running")
    );
}

/// CompleteAutomation terminal: run row flipped to `done` and decision queue
/// row flipped to `completed` inside the same transaction.
#[tokio::test(flavor = "multi_thread")]
async fn complete_automation_terminal_atomically_finalizes_run_and_queue() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;

    // Fold through both steps so we satisfy the completion invariant.
    state.current_step = 2;
    state.results.insert("step_sql".into(), json!({"rows": 3}));
    state
        .results
        .insert("step_fmt".into(), json!({"text": "ok"}));

    let events = vec![(
        "subrun_completed".into(),
        json!({"subrun_name": "atomic_test", "success": true}),
    )];

    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::Unclaimed,
            expected_version: 0,
            new_state: state,
            result_delta: json!({"step_fmt": {"text": "ok"}}),
            step_hash_delta: json!({}),
            events,
            attempt: 0,
            terminal: DecisionTerminal::CompleteAutomation {
                final_answer: "[\"done\"]".into(),
            },
        },
    )
    .await
    .expect("commit_decision");

    assert!(matches!(outcome, CommitOutcome::Committed));
    assert_eq!(run_task_status(&db, &run_id).await.as_deref(), Some("done"));
    assert_eq!(
        queue_status(&db, &decision_task_id).await.as_deref(),
        Some("completed")
    );
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.answer.as_deref(), Some("[\"done\"]"));
    assert_eq!(event_types(&db, &run_id).await, vec!["subrun_completed"]);
}

/// FailAutomation terminal: run row flipped to `failed` and decision queue
/// row flipped to `failed` atomically, with the error surfaced on both.
#[tokio::test(flavor = "multi_thread")]
async fn fail_automation_terminal_atomically_marks_run_and_queue_failed() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;

    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::Unclaimed,
            expected_version: 0,
            new_state: state,
            result_delta: json!({}),
            step_hash_delta: json!({}),
            events: vec![(
                "subrun_completed".into(),
                json!({"success": false, "error": "boom"}),
            )],
            attempt: 0,
            terminal: DecisionTerminal::FailAutomation {
                error: "boom".into(),
            },
        },
    )
    .await
    .expect("commit_decision");

    assert!(matches!(outcome, CommitOutcome::Committed));
    assert_eq!(
        run_task_status(&db, &run_id).await.as_deref(),
        Some("failed")
    );
    assert_eq!(
        queue_status(&db, &decision_task_id).await.as_deref(),
        Some("failed")
    );
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.error_message.as_deref(), Some("boom"));
}

/// Version conflict rolls back EVERYTHING: no event insert, no run/queue
/// transition. This is the invariant that makes decision tasks idempotent —
/// a retry that raced a prior worker must leave the DB untouched.
#[tokio::test(flavor = "multi_thread")]
async fn version_conflict_rolls_back_all_writes() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;

    // Simulate another worker racing ahead by bumping the decision_version to
    // 1 via a committed update.
    state.current_step = 1;
    state.results.insert("step_sql".into(), json!({"rows": 1}));
    assert!(
        agentic_automation::extension::update_automation_state(&db, &state)
            .await
            .unwrap()
    );
    let after_race = load_automation_state(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(after_race.decision_version, 1);

    // Our worker still holds expected_version = 0. Commit should rollback.
    state.current_step = 2;
    state
        .results
        .insert("step_fmt".into(), json!({"text": "x"}));
    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::Unclaimed,
            expected_version: 0,
            new_state: state,
            result_delta: json!({"step_fmt": {"text": "x"}}),
            step_hash_delta: json!({}),
            events: vec![("subrun_completed".into(), json!({"success": true}))],
            attempt: 0,
            terminal: DecisionTerminal::CompleteAutomation {
                final_answer: "stale".into(),
            },
        },
    )
    .await
    .expect("commit_decision");

    assert!(matches!(outcome, CommitOutcome::VersionConflict));

    // DB reflects the racing worker's state, NOT our conflicting commit.
    let final_state = load_automation_state(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(final_state.decision_version, 1);
    assert_eq!(final_state.current_step, 1);
    assert!(!final_state.results.contains_key("step_fmt"));

    // No event inserted; queue + run untouched.
    assert!(event_types(&db, &run_id).await.is_empty());
    assert_eq!(
        run_task_status(&db, &run_id).await.as_deref(),
        Some("running")
    );
    assert_eq!(
        queue_status(&db, &decision_task_id).await.as_deref(),
        Some("queued")
    );
}

/// Event inserts start at `max_seq + 1` — sharing a run with prior events
/// (from coordinator-emitted `delegation_completed`/`input_resolved`) must
/// not collide on `(run_id, seq)`.
#[tokio::test(flavor = "multi_thread")]
async fn events_append_after_existing_coordinator_events() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;

    // Coordinator wrote these before this decision boundary.
    crud::insert_event(&db, &run_id, 0, "delegation_completed", &json!({}), 0)
        .await
        .unwrap();
    crud::insert_event(&db, &run_id, 1, "input_resolved", &json!({}), 0)
        .await
        .unwrap();

    state.current_step = 1;
    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id,
            claim: DecisionClaim::Unclaimed,
            expected_version: 0,
            new_state: state,
            result_delta: json!({}),
            step_hash_delta: json!({}),
            events: vec![
                ("subrun_step_completed".into(), json!({"step": "step_sql"})),
                ("subrun_step_started".into(), json!({"step": "step_fmt"})),
            ],
            attempt: 0,
            terminal: DecisionTerminal::Continuing,
        },
    )
    .await
    .expect("commit_decision");
    assert!(matches!(outcome, CommitOutcome::Committed));

    let events = crud::get_all_events(&db, &run_id).await.unwrap();
    let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3],
        "seq must be monotonic across writers"
    );
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        vec![
            "delegation_completed",
            "input_resolved",
            "subrun_step_completed",
            "subrun_step_started",
        ]
    );
}

/// Multi-step accumulation: each commit_decision passes a one-key delta;
/// the persisted `results` column merges via `||` so all keys accumulate
/// across decisions. This is the core optimization path — a regression that
/// causes `||` to overwrite (or that drops keys somewhere upstream) would
/// only surface at runtime as missing step outputs.
#[tokio::test(flavor = "multi_thread")]
async fn sequential_deltas_accumulate_in_results_column() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;

    // First decision: fold step_sql.
    state.current_step = 1;
    state.results.insert("step_sql".into(), json!({"rows": 3}));
    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::Unclaimed,
            expected_version: 0,
            new_state: state.clone(),
            result_delta: json!({"step_sql": {"rows": 3}}),
            step_hash_delta: json!({}),
            events: vec![],
            attempt: 0,
            terminal: DecisionTerminal::Continuing,
        },
    )
    .await
    .expect("first commit");
    assert!(matches!(outcome, CommitOutcome::Committed));

    // Reload and verify only step_sql is present.
    let loaded = load_automation_state(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(loaded.decision_version, 1);
    assert!(loaded.results.contains_key("step_sql"));
    assert!(!loaded.results.contains_key("step_fmt"));

    // Second decision: fold step_fmt with delta containing only the new key.
    // If `||` were overwriting instead of merging, step_sql would be lost.
    let mut state2 = loaded;
    state2.current_step = 2;
    state2
        .results
        .insert("step_fmt".into(), json!({"text": "ok"}));
    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::Unclaimed,
            expected_version: 1,
            new_state: state2,
            result_delta: json!({"step_fmt": {"text": "ok"}}),
            step_hash_delta: json!({}),
            events: vec![],
            attempt: 0,
            terminal: DecisionTerminal::Continuing,
        },
    )
    .await
    .expect("second commit");
    assert!(matches!(outcome, CommitOutcome::Committed));

    // Both keys must be present and untouched.
    let final_state = load_automation_state(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(final_state.decision_version, 2);
    assert_eq!(final_state.current_step, 2);
    assert_eq!(final_state.results.len(), 2);
    assert_eq!(
        final_state.results.get("step_sql"),
        Some(&json!({"rows": 3}))
    );
    assert_eq!(
        final_state.results.get("step_fmt"),
        Some(&json!({"text": "ok"}))
    );
}

/// Completion invariant: CompleteAutomation with `current_step < tasks.len()`
/// is rejected as a bug and rolls back. Without this, a decider with an
/// off-by-one would strand the automation with pending steps never running.
#[tokio::test(flavor = "multi_thread")]
async fn complete_automation_with_pending_steps_is_rejected() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;

    // Only step 0 folded — step 1 still pending. This must not be marked complete.
    state.current_step = 1;
    state.results.insert("step_sql".into(), json!({"rows": 1}));

    let err = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::Unclaimed,
            expected_version: 0,
            new_state: state,
            result_delta: json!({"step_sql": {"rows": 1}}),
            step_hash_delta: json!({}),
            events: vec![],
            attempt: 0,
            terminal: DecisionTerminal::CompleteAutomation {
                final_answer: "premature".into(),
            },
        },
    )
    .await
    .expect_err("expected invariant violation");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("current_step"),
        "unexpected: {msg}"
    );

    // Nothing committed.
    let loaded = load_automation_state(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(loaded.decision_version, 0);
    assert_eq!(
        run_task_status(&db, &run_id).await.as_deref(),
        Some("running")
    );
    assert_eq!(
        queue_status(&db, &decision_task_id).await.as_deref(),
        Some("queued")
    );
}

// ── Claim ownership on the decision task's queue row ─────────────────────────
//
// `commit_decision`'s terminal branches stamp `decision_task_id`'s queue row,
// and that row is the decision task's *own* claim — this code runs inside
// `TaskExecutor::execute` for that very task, on the automation path, which is
// the primary durable-queue workload. Graceful shutdown makes the following
// interleaving reachable within milliseconds:
//
//   1. this process claims the decision task and starts the inline step;
//   2. SIGTERM — the release flips the claim back to `queued`, NOTIFY wakes
//      the fleet, and a peer re-claims it;
//   3. this process's inline step (an SQL or LLM call — seconds, not
//      microseconds) finally finishes and commits.
//
// Stamping by primary key at step 3 lands `completed` on the peer's live
// claim. If the peer then dies the reaper skips the row — terminal rows are
// not reaped — so the task is never requeued and the parent coordinator waits
// forever.

/// Set a queue row's owner directly, simulating the peer takeover of step 2
/// without needing a second live transport.
async fn claim_as(db: &DatabaseConnection, task_id: &str, worker_id: &str) {
    use sea_orm::{ConnectionTrait, Statement};
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET queue_status = 'claimed', worker_id = $2, claimed_at = now(), \
             last_heartbeat = now(), claim_count = claim_count + 1, \
             claim_epoch = claim_epoch + 1 \
         WHERE task_id = $1",
        [task_id.into(), worker_id.into()],
    ))
    .await
    .expect("claim as worker");
}

async fn queue_owner(db: &DatabaseConnection, task_id: &str) -> Option<String> {
    crud::get_queue_entry(db, task_id)
        .await
        .unwrap()
        .and_then(|m| m.worker_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_rolls_back_rather_than_stamp_a_peers_reclaimed_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;

    // Steps 1–2: we held the claim, were released on shutdown, and a peer
    // re-claimed the row while our inline step was still running.
    claim_as(&db, &decision_task_id, "peer").await;

    // Step 3: our decision finally commits, still believing it owns the row.
    state.current_step = state.workflow.tasks.len();
    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::HeldBy {
                worker_id: "dying".into(),
                claim_epoch: 0,
            },
            expected_version: 0,
            new_state: state,
            result_delta: json!({"step_fmt": {"text": "late"}}),
            step_hash_delta: json!({}),
            events: vec![("subrun_completed".into(), json!({"success": true}))],
            attempt: 0,
            terminal: DecisionTerminal::CompleteAutomation {
                final_answer: "late".into(),
            },
        },
    )
    .await
    .expect("commit_decision");

    assert!(
        matches!(outcome, CommitOutcome::ClaimLost),
        "a commit from a worker that no longer owns the decision task must report ClaimLost, \
         got {outcome:?}"
    );

    // The peer's claim is intact and still reapable if the peer dies.
    assert_eq!(
        queue_status(&db, &decision_task_id).await.as_deref(),
        Some("claimed"),
        "the peer's live claim must not be stamped terminal"
    );
    assert_eq!(
        queue_owner(&db, &decision_task_id).await.as_deref(),
        Some("peer")
    );

    // And the transaction rolled back: no run-row state, no events, no state
    // advance. Committing these while the queue row belongs to someone else is
    // exactly the split-brain the rollback exists to prevent.
    assert_eq!(
        run_task_status(&db, &run_id).await.as_deref(),
        Some("running"),
        "run state must not advance against a row this worker no longer owns"
    );
    assert!(
        event_types(&db, &run_id).await.is_empty(),
        "no events may be persisted by a rolled-back commit"
    );
    let final_state = load_automation_state(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        final_state.decision_version, 0,
        "the CAS must not have advanced"
    );
    assert_eq!(final_state.current_step, 0);
    assert!(!final_state.results.contains_key("step_fmt"));
}

/// `worker_id` is process-stable, so after a reap this process can hold a NEW
/// claim of the row under the very same id. A commit from the old execution
/// must still roll back: only the claim's epoch tells the two apart.
#[tokio::test(flavor = "multi_thread")]
async fn commit_from_a_stale_claim_of_the_same_worker_rolls_back() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;
    claim_as(&db, &decision_task_id, "holder").await;
    let stale = crud::get_queue_entry(&db, &decision_task_id)
        .await
        .unwrap()
        .unwrap()
        .claim_epoch;
    // The reaper requeued it and the same worker took it again.
    claim_as(&db, &decision_task_id, "holder").await;

    state.current_step = state.workflow.tasks.len();
    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::HeldBy {
                worker_id: "holder".into(),
                claim_epoch: stale,
            },
            expected_version: 0,
            new_state: state,
            result_delta: json!({"step_fmt": {"text": "late"}}),
            step_hash_delta: json!({}),
            events: vec![("subrun_completed".into(), json!({"success": true}))],
            attempt: 0,
            terminal: DecisionTerminal::CompleteAutomation {
                final_answer: "late".into(),
            },
        },
    )
    .await
    .expect("commit_decision");

    assert!(
        matches!(outcome, CommitOutcome::ClaimLost),
        "got {outcome:?}"
    );
    assert_eq!(
        queue_status(&db, &decision_task_id).await.as_deref(),
        Some("claimed"),
        "the re-claim's live row must not be stamped terminal"
    );
    assert_eq!(
        run_task_status(&db, &run_id).await.as_deref(),
        Some("running")
    );
}

/// The other half: an ownership-scoped commit from the *actual* holder must
/// still work. Without this, "always roll back" would satisfy the test above
/// while breaking every automation in production.
#[tokio::test(flavor = "multi_thread")]
async fn commit_lands_for_the_worker_that_holds_the_claim() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (run_id, mut state) = seed_run(&db).await;
    let decision_task_id = run_id.clone();
    seed_queue_row(&db, &decision_task_id, &run_id).await;
    claim_as(&db, &decision_task_id, "holder").await;

    state.current_step = state.workflow.tasks.len();
    let outcome = commit_decision(
        &db,
        DecisionCommit {
            run_id: run_id.clone(),
            decision_task_id: decision_task_id.clone(),
            claim: DecisionClaim::HeldBy {
                worker_id: "holder".into(),
                claim_epoch: 0,
            },
            expected_version: 0,
            new_state: state,
            result_delta: json!({"step_fmt": {"text": "ok"}}),
            step_hash_delta: json!({}),
            events: vec![("subrun_completed".into(), json!({"success": true}))],
            attempt: 0,
            terminal: DecisionTerminal::CompleteAutomation {
                final_answer: "ok".into(),
            },
        },
    )
    .await
    .expect("commit_decision");

    assert!(
        matches!(outcome, CommitOutcome::Committed),
        "got {outcome:?}"
    );
    assert_eq!(
        queue_status(&db, &decision_task_id).await.as_deref(),
        Some("completed")
    );
    assert_eq!(run_task_status(&db, &run_id).await.as_deref(), Some("done"));
}
