//! Integration tests for agentic-runtime against a real PostgreSQL database.
//!
//! Uses testcontainers to automatically spin up a Postgres instance — no manual
//! setup needed. Just run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(integration_tests)'
//!
//! To use an external DB instead (e.g. oxy-postgres), set OXY_DATABASE_URL:
//!   OXY_DATABASE_URL=postgresql://postgres:postgres@localhost:15432/oxy \
//!     cargo nextest run -p agentic-runtime --test integration -E 'test(integration_tests)'

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_core::transport::{CoordinatorTransport, WorkerMessage, WorkerTransport};
use agentic_runtime::crud;
use agentic_runtime::event_registry::EventRegistry;
use agentic_runtime::migration::RuntimeMigrator;
use agentic_runtime::transport::DurableTransport;
use sea_orm::{Database, DatabaseConnection};
use serde_json::json;

/// Shared test Postgres container — started once per process, reused across tests.
/// The container is automatically cleaned up when the test process exits.
static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

/// Keeps the Postgres container handle alive for the process lifetime without
/// leaking. `ReuseDirective::Always` means tests across nextest processes share
/// the same container regardless.
static TEST_CONTAINER: tokio::sync::OnceCell<
    std::sync::Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

/// Get a database connection for testing. Uses testcontainers to spin up
/// Postgres automatically, or OXY_DATABASE_URL if set. Shared with sibling
/// modules (e.g. `run_feed_test`) so they don't copy the container setup.
pub(crate) async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            // If OXY_DATABASE_URL is set, use it (external DB).
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }

            // Start a reusable testcontainer. `ReuseDirective::Always` means
            // nextest processes share one Postgres container instead of each
            // spawning their own.
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
                            .expect("failed to start Postgres testcontainer — is Docker running?"),
                    )
                })
                .await;
            let port = container
                .get_host_port_ipv4(5432_u16)
                .await
                .expect("failed to get Postgres port");
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    // Retry connection — the reusable container may still be starting up.
    let mut db = None;
    for attempt in 0..10 {
        match Database::connect(&url).await {
            Ok(conn) => {
                db = Some(conn);
                break;
            }
            Err(e) if attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                eprintln!("test_db: connection attempt {attempt} failed: {e}, retrying...");
            }
            Err(e) => panic!("failed to connect to test database after 10 retries: {e}"),
        }
    }
    let db = db.unwrap();

    // Central then runtime (production order — see oxy_test_utils::migration),
    // then the analytics extension for the full test surface. Chained through
    // the token rather than called separately so it runs inside the same
    // migration advisory lock, not after it.
    use agentic_analytics::extension::AnalyticsMigrator;
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .then::<AnalyticsMigrator>()
        .await
        .expect("analytics migrations failed")
        .finish()
        .await;

    // Every test shares this one Postgres and the task queue / outcomes are
    // claimed globally, so clear them up front — each test then starts from an
    // empty queue regardless of what a prior (serial-db) test left behind.
    cleanup_queued_entries(&db).await;

    Some(db)
}

/// Generate a unique run ID for test isolation.
fn test_run_id() -> String {
    format!("test-{}", uuid::Uuid::new_v4())
}

// ── CRUD round-trip tests ───────────────────────────────────────────────────

#[tokio::test]
async fn test_insert_and_get_run() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();

    crud::insert_run(
        &db,
        &run_id,
        "What is the revenue?",
        None,
        "analytics",
        Some(json!({ "agent_id": "test_agent" })),
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert_run failed");

    let run = crud::get_run(&db, &run_id)
        .await
        .expect("get_run failed")
        .expect("run not found");

    assert_eq!(run.id, run_id);
    assert_eq!(run.question, "What is the revenue?");
    assert_eq!(
        crud::user_facing_status(run.task_status.as_deref()),
        "running"
    );
    assert_eq!(run.source_type.as_deref(), Some("analytics"));
    assert!(run.answer.is_none());
}

#[tokio::test]
async fn test_update_run_lifecycle() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();

    crud::insert_run(
        &db,
        &run_id,
        "Q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    // running → suspended
    crud::update_run_suspended(&db, &run_id).await.unwrap();
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        crud::user_facing_status(run.task_status.as_deref()),
        "suspended"
    );

    // suspended → running
    crud::update_run_running(&db, &run_id).await.unwrap();
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        crud::user_facing_status(run.task_status.as_deref()),
        "running"
    );

    // running → done
    crud::update_run_done(&db, &run_id, "42", None)
        .await
        .unwrap();
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(crud::user_facing_status(run.task_status.as_deref()), "done");
    assert_eq!(run.answer.as_deref(), Some("42"));
}

#[tokio::test]
async fn test_update_run_failed() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();

    crud::insert_run(&db, &run_id, "Q", None, "builder", None, uuid::Uuid::nil())
        .await
        .unwrap();

    crud::update_run_failed(&db, &run_id, "something broke")
        .await
        .unwrap();
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        crud::user_facing_status(run.task_status.as_deref()),
        "failed"
    );
    assert_eq!(run.error_message.as_deref(), Some("something broke"));
}

#[tokio::test]
async fn test_batch_insert_and_get_events() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();

    crud::insert_run(
        &db,
        &run_id,
        "Q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    let events = vec![
        (
            0,
            "state_enter".to_string(),
            json!({"state": "clarifying", "revision": 0, "trace_id": "t"}).to_string(),
            0,
        ),
        (
            1,
            "llm_token".to_string(),
            json!({"token": "hello"}).to_string(),
            0,
        ),
        (
            2,
            "done".to_string(),
            json!({"answer": "42", "trace_id": "t"}).to_string(),
            0,
        ),
    ];
    crud::batch_insert_events(&db, &run_id, &events)
        .await
        .unwrap();

    let rows = crud::get_events_after(&db, &run_id, -1).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].event_type, "state_enter");
    assert_eq!(rows[1].event_type, "llm_token");
    assert_eq!(rows[2].event_type, "done");

    let max_seq = crud::get_max_seq(&db, &run_id).await.unwrap();
    assert_eq!(max_seq, 2);

    let rows = crud::get_events_after(&db, &run_id, 0).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].seq, 1);
}

#[tokio::test]
async fn test_suspension_round_trip() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();

    crud::insert_run(
        &db,
        &run_id,
        "Q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    let resume_data = agentic_core::human_input::SuspendedRunData {
        from_state: "clarifying".to_string(),
        original_input: "Q".to_string(),
        trace_id: "t".to_string(),
        stage_data: serde_json::Value::Null,
        question: "which metric?".to_string(),
        suggestions: vec!["revenue".to_string()],
    };
    crud::upsert_suspension(
        &db,
        &run_id,
        "which metric?",
        &["revenue".into()],
        &resume_data,
    )
    .await
    .unwrap();

    let loaded = crud::get_suspension(&db, &run_id)
        .await
        .unwrap()
        .expect("suspension not found");
    assert_eq!(loaded.question, "which metric?");
}

#[tokio::test]
async fn test_cleanup_stale_runs() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();

    crud::insert_run(
        &db,
        &run_id,
        "Q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();
    // Run is "running" — cleanup should mark it failed.
    let count = crud::cleanup_stale_runs(&db).await.unwrap();
    assert!(count >= 1);

    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        crud::user_facing_status(run.task_status.as_deref()),
        "failed"
    );
}

// ── EventRegistry integration tests ─────────────────────────────────────────

#[tokio::test]
async fn test_registry_processes_analytics_core_events() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();

    crud::insert_run(
        &db,
        &run_id,
        "Q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    let events = vec![
        (
            0,
            "state_enter".to_string(),
            json!({"state": "clarifying", "revision": 0, "trace_id": "t"}).to_string(),
            0,
        ),
        (
            1,
            "llm_token".to_string(),
            json!({"token": "hello", "sub_spec_index": null}).to_string(),
            0,
        ),
        (
            2,
            "state_exit".to_string(),
            json!({"state": "clarifying", "outcome": "advanced", "trace_id": "t"}).to_string(),
            0,
        ),
        (
            3,
            "done".to_string(),
            json!({"answer": "42", "trace_id": "t"}).to_string(),
            0,
        ),
    ];
    crud::batch_insert_events(&db, &run_id, &events)
        .await
        .unwrap();

    let mut registry = EventRegistry::new();
    registry.register("analytics", agentic_analytics::event_handler());
    let mut processor = registry.stream_processor("analytics");

    let rows = crud::get_events_after(&db, &run_id, -1).await.unwrap();
    let mut ui_events = Vec::new();
    for row in rows {
        ui_events.extend(processor.process(&row.event_type, &row.payload));
    }

    // Verify core events are transformed correctly.
    let event_types: Vec<&str> = ui_events.iter().map(|(et, _)| et.as_str()).collect();
    assert!(event_types.contains(&"step_start"));
    assert!(event_types.contains(&"text_delta"));
    assert!(event_types.contains(&"step_end"));
    assert!(event_types.contains(&"done"));
}

#[tokio::test]
async fn test_registry_processes_builder_domain_events() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();

    crud::insert_run(&db, &run_id, "Q", None, "builder", None, uuid::Uuid::nil())
        .await
        .unwrap();

    let events = vec![
        (
            0,
            "state_enter".to_string(),
            json!({"state": "solving", "revision": 0, "trace_id": "t"}).to_string(),
            0,
        ),
        (
            1,
            "tool_used".to_string(),
            json!({"tool_name": "read_file", "summary": "Read config.yml"}).to_string(),
            0,
        ),
        (
            2,
            "file_change_pending".to_string(),
            json!({"file_path": "config.yml", "description": "Update DB", "new_content": "..."})
                .to_string(),
            0,
        ),
        (
            3,
            "done".to_string(),
            json!({"answer": "done", "trace_id": "t"}).to_string(),
            0,
        ),
    ];
    crud::batch_insert_events(&db, &run_id, &events)
        .await
        .unwrap();

    let mut registry = EventRegistry::new();
    registry.register("builder", agentic_builder::event_handler());
    let mut processor = registry.stream_processor("builder");

    let rows = crud::get_events_after(&db, &run_id, -1).await.unwrap();
    let mut ui_events = Vec::new();
    for row in rows {
        ui_events.extend(processor.process(&row.event_type, &row.payload));
    }

    let event_types: Vec<&str> = ui_events.iter().map(|(et, _)| et.as_str()).collect();
    assert!(
        event_types.contains(&"step_start"),
        "missing step_start: {:?}",
        event_types
    );
    assert!(
        event_types.contains(&"tool_used"),
        "missing tool_used: {:?}",
        event_types
    );
    assert!(
        event_types.contains(&"file_change_pending"),
        "missing file_change_pending: {:?}",
        event_types
    );
    assert!(
        event_types.contains(&"done"),
        "missing done: {:?}",
        event_types
    );
}

/// `delegation_started` and `delegation_completed` are coordinator-emitted events.
/// They must survive the StreamProcessor and appear as UiBlocks on the frontend.
/// This test persists them to the DB then replays through the registry, asserting
/// that both event types are present in the output — not silently dropped.
#[tokio::test]
async fn test_registry_passes_through_delegation_ui_blocks() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();
    let child_run_id = test_run_id();

    crud::insert_run(
        &db,
        &run_id,
        "Q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    // Simulate the exact sequence the coordinator persists for a builder delegation.
    let events = vec![
        (
            0,
            "state_enter".to_string(),
            json!({"state": "clarifying", "revision": 0, "trace_id": "t"}).to_string(),
            0,
        ),
        (
            1,
            "delegation_started".to_string(),
            json!({
                "event_type": "delegation_started",
                "child_task_id": child_run_id,
                "target": "agent:__builder__",
                "request": "create missing metric",
            })
            .to_string(),
            0,
        ),
        (
            2,
            "delegation_completed".to_string(),
            json!({
                "event_type": "delegation_completed",
                "child_task_id": child_run_id,
                "success": true,
                "answer": "built the metric",
            })
            .to_string(),
            0,
        ),
        (
            3,
            "done".to_string(),
            json!({"answer": "analytics answer", "trace_id": "t"}).to_string(),
            0,
        ),
    ];
    crud::batch_insert_events(&db, &run_id, &events)
        .await
        .unwrap();

    let mut registry = EventRegistry::new();
    registry.register("analytics", agentic_analytics::event_handler());
    let mut processor = registry.stream_processor("analytics");

    let rows = crud::get_events_after(&db, &run_id, -1).await.unwrap();
    let mut ui_events: Vec<(String, serde_json::Value)> = Vec::new();
    for row in rows {
        ui_events.extend(processor.process(&row.event_type, &row.payload));
    }

    let event_types: Vec<&str> = ui_events.iter().map(|(et, _)| et.as_str()).collect();

    assert!(
        event_types.contains(&"delegation_started"),
        "delegation_started must reach the frontend as a UiBlock: {event_types:?}"
    );
    assert!(
        event_types.contains(&"delegation_completed"),
        "delegation_completed must reach the frontend as a UiBlock: {event_types:?}"
    );

    // Verify payload fields the frontend relies on.
    let started = ui_events
        .iter()
        .find(|(et, _)| et == "delegation_started")
        .map(|(_, p)| p)
        .unwrap();
    assert_eq!(
        started["child_task_id"].as_str(),
        Some(child_run_id.as_str()),
        "delegation_started payload must carry child_task_id"
    );
    assert_eq!(
        started["target"].as_str(),
        Some("agent:__builder__"),
        "delegation_started payload must carry target"
    );

    let completed = ui_events
        .iter()
        .find(|(et, _)| et == "delegation_completed")
        .map(|(_, p)| p)
        .unwrap();
    assert_eq!(
        completed["success"].as_bool(),
        Some(true),
        "delegation_completed payload must carry success flag"
    );
    assert_eq!(
        completed["child_task_id"].as_str(),
        Some(child_run_id.as_str()),
        "delegation_completed payload must carry child_task_id"
    );
}

// ── Coordinator suspend/resume integration tests ──────────────────────────

mod coordinator_tests {
    use super::*;
    use agentic_core::delegation::{SuspendReason, TaskAssignment, TaskOutcome, TaskSpec};
    use agentic_core::human_input::SuspendedRunData;
    use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
    use agentic_runtime::coordinator::Coordinator;
    use agentic_runtime::state::RuntimeState;
    use agentic_runtime::transport::LocalTransport;
    use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn make_suspend_data() -> SuspendedRunData {
        SuspendedRunData {
            from_state: "clarifying".into(),
            original_input: "test question".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({}),
            question: "which metric?".into(),
            suggestions: vec!["revenue".into()],
        }
    }

    /// Executor that suspends on Agent, completes on Resume, runs automations.
    struct SuspendThenResumeExecutor;

    #[async_trait]
    impl TaskExecutor for SuspendThenResumeExecutor {
        async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
            let (event_tx, event_rx) = mpsc::channel(16);
            let (outcome_tx, outcome_rx) = mpsc::channel(4);
            let cancel = CancellationToken::new();

            let task_id = assignment.task_id.clone();
            let spec = assignment.spec.clone();
            tokio::spawn(async move {
                let _ = event_tx
                    .send(("test_event".into(), json!({"task": &task_id})))
                    .await;
                drop(event_tx);
                let outcome = match spec {
                    TaskSpec::Agent { .. } => TaskOutcome::Suspended {
                        reason: SuspendReason::HumanInput {
                            questions: vec![agentic_core::HumanInputQuestion {
                                prompt: "which metric?".into(),
                                suggestions: vec!["revenue".into()],
                            }],
                        },
                        resume_data: make_suspend_data(),
                        trace_id: "trace-1".into(),
                    },
                    TaskSpec::Resume { .. } => TaskOutcome::Done {
                        answer: "resumed answer".into(),
                        metadata: None,
                    },
                    TaskSpec::Automation { .. } => TaskOutcome::Done {
                        answer: "workflow done".into(),
                        metadata: None,
                    },
                    TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                        answer: "step done".into(),
                        metadata: None,
                    },
                    _ => {
                        unreachable!("AutomationDecision/Custom not used in runtime tests")
                    }
                    TaskSpec::Airway { .. } => {
                        unreachable!("Airway not used in runtime tests")
                    }
                };
                let _ = outcome_tx.send(outcome).await;
            });

            Ok(ExecutingTask {
                events: event_rx,
                outcomes: outcome_rx,
                cancel,
                answers: None,
            })
        }
    }

    /// Executor that suspends with Delegation (spawns a child task that
    /// never completes). Used to test delegation timeout.
    /// Root tasks delegate; child tasks hang forever (simulating stuck child).
    #[allow(dead_code)]
    struct DelegatingExecutor;

    #[async_trait]
    impl TaskExecutor for DelegatingExecutor {
        async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
            let (_event_tx, event_rx) = mpsc::channel(16);
            let (outcome_tx, outcome_rx) = mpsc::channel(4);
            let cancel = CancellationToken::new();

            let is_child = assignment.parent_task_id.is_some();
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                if is_child {
                    // Child task: hang until cancelled (simulates stuck child).
                    cancel_clone.cancelled().await;
                    let _ = outcome_tx.send(TaskOutcome::Cancelled).await;
                } else {
                    // Root task: delegate to a child.
                    let _ = outcome_tx
                        .send(TaskOutcome::Suspended {
                            reason: SuspendReason::Delegation {
                                target: agentic_core::delegation::DelegationTarget::Agent {
                                    agent_id: "child_agent".into(),
                                },
                                request: "do something".into(),
                                context: json!(null),
                                policy: None,
                            },
                            resume_data: make_suspend_data(),
                            trace_id: "trace-delegation".into(),
                        })
                        .await;
                }
            });

            Ok(ExecutingTask {
                events: event_rx,
                outcomes: outcome_rx,
                cancel,
                answers: None,
            })
        }
    }

    #[tokio::test]
    async fn test_coordinator_human_input_suspend_and_resume() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(SuspendThenResumeExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        )
        .with_suspend_timeout(Duration::from_secs(30));
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        // Wait for the run to become suspended.
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Some(s) = state.statuses.get(&run_id) {
                if matches!(&*s, agentic_runtime::state::RunStatus::Suspended { .. }) {
                    break;
                }
            }
        }

        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(run.task_status.as_deref()),
            "suspended",
            "run should be suspended"
        );

        let suspension = crud::get_suspension(&db, &run_id).await.unwrap();
        assert!(suspension.is_some(), "suspension data should be persisted");

        answer_tx.send("revenue".into()).await.unwrap();

        // Wait for coordinator to finish.
        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(run.task_status.as_deref()),
            "done",
            "run should be done after resume"
        );

        let events = crud::get_all_events(&db, &run_id).await.unwrap();
        let event_types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert!(
            event_types.contains(&"input_resolved"),
            "missing input_resolved event: {:?}",
            event_types
        );
    }

    #[tokio::test]
    /// HITL suspension should never time out — only delegation does.
    /// This test verifies that an awaiting_input task stays alive past
    /// the suspend_timeout period.
    async fn test_coordinator_suspend_timeout() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(SuspendThenResumeExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        )
        .with_suspend_timeout(Duration::from_millis(200)); // Very short timeout
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        // Wait longer than the suspend_timeout (200ms) to prove HITL doesn't time out.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The run should still be awaiting_input (not timed_out).
        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            run.task_status.as_deref(),
            Some("awaiting_input"),
            "HITL suspension should not time out"
        );

        // Now send an answer to let the coordinator finish cleanly.
        answer_tx.send("revenue".into()).await.unwrap();

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out after answer")
            .expect("coordinator panicked");

        // Verify run completed successfully.
        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            run.task_status.as_deref(),
            Some("done"),
            "run should be done after answering"
        );
    }

    #[tokio::test]
    async fn test_coordinator_event_seq_continuity() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(SuspendThenResumeExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });

        // Wait for suspension.
        let coord_handle = tokio::spawn(async move { coordinator.run().await });
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Some(s) = state.statuses.get(&run_id) {
                if matches!(&*s, agentic_runtime::state::RunStatus::Suspended { .. }) {
                    break;
                }
            }
        }

        answer_tx.send("revenue".into()).await.unwrap();

        // Wait for completion.
        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        // Verify event seq has no gaps.
        let events = crud::get_all_events(&db, &run_id).await.unwrap();
        let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
        for i in 1..seqs.len() {
            assert_eq!(
                seqs[i],
                seqs[i - 1] + 1,
                "event seq gap at index {i}: {:?}",
                seqs
            );
        }
        assert!(!seqs.is_empty(), "should have at least some events");
    }

    /// Delegation suspend/resume emits the same awaiting_input →
    /// input_resolved pair as human input. The pair is
    /// suspend-reason-agnostic: awaiting_input is emitted by the
    /// orchestrator on ALL suspensions, and input_resolved is emitted
    /// by the coordinator on ALL resumes.
    #[tokio::test]
    async fn test_coordinator_delegation_emits_awaiting_and_resolved_pair() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        // DelegationSuspendExecutor emits an awaiting_input event (simulating
        // what the orchestrator now does for delegation suspensions).
        struct DelegationWithAwaitingExecutor;

        #[async_trait]
        impl TaskExecutor for DelegationWithAwaitingExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let task_id = assignment.task_id.clone();
                let spec = assignment.spec.clone();
                tokio::spawn(async move {
                    match spec {
                        TaskSpec::Agent { .. } => {
                            // Orchestrator emits awaiting_input for ALL suspend
                            // reasons, including delegation.
                            let _ = event_tx
                                .send((
                                    "awaiting_input".into(),
                                    json!({
                                        "questions": [{"prompt": "Execute procedure: test.automation.yml", "suggestions": []}],
                                        "from_state": "executing",
                                        "trace_id": "t1",
                                    }),
                                ))
                                .await;
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Suspended {
                                    reason: SuspendReason::Delegation {
                                        target:
                                            agentic_core::delegation::DelegationTarget::Automation {
                                                workflow_ref: "test.automation.yml".into(),
                                            },
                                        request: "run workflow".into(),
                                        context: json!({}),
                                        policy: None,
                                    },
                                    resume_data: make_suspend_data(),
                                    trace_id: "t1".into(),
                                })
                                .await;
                        }
                        TaskSpec::Resume { .. } => {
                            let _ = event_tx
                                .send(("test_event".into(), json!({"task": &task_id})))
                                .await;
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Done {
                                    answer: "resumed after delegation".into(),
                                    metadata: None,
                                })
                                .await;
                        }
                        TaskSpec::Automation { .. } => {
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Done {
                                    answer: "workflow output".into(),
                                    metadata: None,
                                })
                                .await;
                        }
                        TaskSpec::AutomationStep { .. } => {
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Done {
                                    answer: "step done".into(),
                                    metadata: None,
                                })
                                .await;
                        }
                        _ => {
                            drop(event_tx);
                            unreachable!("AutomationDecision/Custom not used in runtime tests");
                        }
                        TaskSpec::Airway { .. } => {
                            drop(event_tx);
                            unreachable!("Airway not used in runtime tests");
                        }
                    }
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let executor = Arc::new(DelegationWithAwaitingExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        // Coordinator runs delegation: Agent suspends (with awaiting_input)
        // → child Automation spawned → child completes → parent resumed
        // (with input_resolved) via TaskSpec::Resume → Done.
        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        // Verify both events exist in order.
        let events = crud::get_all_events(&db, &run_id).await.unwrap();
        let event_types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();

        let awaiting_pos = event_types
            .iter()
            .position(|t| *t == "awaiting_input")
            .expect("missing awaiting_input event for delegation");
        let resolved_pos = event_types
            .iter()
            .position(|t| *t == "input_resolved")
            .expect("missing input_resolved event for delegation");
        assert!(
            resolved_pos > awaiting_pos,
            "input_resolved ({resolved_pos}) must come after awaiting_input ({awaiting_pos}): {:?}",
            event_types
        );

        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(run.task_status.as_deref()),
            "done",
            "run should complete after delegation"
        );
    }

    /// When an agent delegates to another agent via `DelegationTarget::Agent`,
    /// the child run must be created as a child in the task tree (with
    /// `parent_run_id` set), not as a root-level run.
    #[tokio::test]
    async fn test_agent_delegation_child_has_parent_run_id() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        /// Executor: parent agent suspends with DelegationTarget::Agent,
        /// child agent completes immediately, parent resumes and completes.
        struct AgentDelegationExecutor;

        #[async_trait]
        impl TaskExecutor for AgentDelegationExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let spec = assignment.spec.clone();
                tokio::spawn(async move {
                    match spec {
                        TaskSpec::Agent {
                            ref agent_id,
                            ref question,
                            ..
                        } if agent_id == "__builder__" => {
                            // Child builder agent — complete immediately.
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Done {
                                    answer: "built the metric".into(),
                                    metadata: None,
                                })
                                .await;
                        }
                        TaskSpec::Agent { .. } => {
                            // Parent analytics agent — suspend with agent delegation.
                            let _ = event_tx
                                .send((
                                    "awaiting_input".into(),
                                    json!({
                                        "questions": [{"prompt": "delegate to builder", "suggestions": []}],
                                        "from_state": "clarifying",
                                        "trace_id": "t1",
                                    }),
                                ))
                                .await;
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Suspended {
                                    reason: SuspendReason::Delegation {
                                        target: agentic_core::delegation::DelegationTarget::Agent {
                                            agent_id: "__builder__".into(),
                                        },
                                        request: "create missing metric".into(),
                                        context: json!({}),
                                        policy: None,
                                    },
                                    resume_data: make_suspend_data(),
                                    trace_id: "t1".into(),
                                })
                                .await;
                        }
                        TaskSpec::Resume { .. } => {
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Done {
                                    answer: "resumed and done".into(),
                                    metadata: None,
                                })
                                .await;
                        }
                        _ => {
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Failed("unexpected spec".into()))
                                .await;
                        }
                    }
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let executor = Arc::new(AgentDelegationExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "analytics".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        let parent_run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(parent_run.task_status.as_deref()),
            "done",
            "parent run should complete after delegation"
        );

        // Find the child run — it should have parent_run_id set to the
        // parent's run_id, proving it was created as a child task.
        let all_events = crud::get_all_events(&db, &run_id).await.unwrap();
        let delegation_event = all_events
            .iter()
            .find(|e| e.event_type == "delegation_started")
            .expect("missing delegation_started event");
        let child_run_id = delegation_event.payload["child_task_id"]
            .as_str()
            .expect("delegation_started should have child_task_id");

        let child_run = crud::get_run(&db, child_run_id).await.unwrap();
        assert!(
            child_run.is_some(),
            "child run should exist in DB with id {child_run_id}"
        );
        let child_run = child_run.unwrap();
        assert_eq!(
            child_run.parent_run_id.as_deref(),
            Some(run_id.as_str()),
            "child run must have parent_run_id = parent's run_id"
        );
    }

    /// Human input resume MUST emit input_resolved so the frontend
    /// closes the suspend popup. This verifies the awaiting_input →
    /// input_resolved pair is correctly emitted.
    #[tokio::test]
    async fn test_coordinator_human_input_emits_awaiting_and_resolved_pair() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        // Use an executor that emits awaiting_input as a regular event
        // (simulating what the orchestrator does) AND suspends.
        struct AwaitingInputExecutor;

        #[async_trait]
        impl TaskExecutor for AwaitingInputExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let spec = assignment.spec.clone();
                tokio::spawn(async move {
                    match spec {
                        TaskSpec::Agent { .. } => {
                            // Simulate the orchestrator emitting awaiting_input.
                            let _ = event_tx
                                .send((
                                    "awaiting_input".into(),
                                    json!({
                                        "questions": [{"prompt": "which metric?", "suggestions": ["revenue"]}],
                                        "from_state": "clarifying",
                                        "trace_id": "t1",
                                    }),
                                ))
                                .await;
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Suspended {
                                    reason: SuspendReason::HumanInput {
                                        questions: vec![agentic_core::HumanInputQuestion {
                                            prompt: "which metric?".into(),
                                            suggestions: vec!["revenue".into()],
                                        }],
                                    },
                                    resume_data: make_suspend_data(),
                                    trace_id: "t1".into(),
                                })
                                .await;
                        }
                        TaskSpec::Resume { .. } => {
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Done {
                                    answer: "done".into(),
                                    metadata: None,
                                })
                                .await;
                        }
                        _ => {
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Failed("unsupported".into()))
                                .await;
                        }
                    }
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let executor = Arc::new(AwaitingInputExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        // Wait for suspended.
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Some(s) = state.statuses.get(&run_id) {
                if matches!(&*s, agentic_runtime::state::RunStatus::Suspended { .. }) {
                    break;
                }
            }
        }

        answer_tx.send("revenue".into()).await.unwrap();

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        // Verify both events exist in order.
        let events = crud::get_all_events(&db, &run_id).await.unwrap();
        let event_types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();

        let awaiting_pos = event_types
            .iter()
            .position(|t| *t == "awaiting_input")
            .expect("missing awaiting_input event");
        let resolved_pos = event_types
            .iter()
            .position(|t| *t == "input_resolved")
            .expect("missing input_resolved event");
        assert!(
            resolved_pos > awaiting_pos,
            "input_resolved (pos {resolved_pos}) should come after awaiting_input (pos {awaiting_pos}): {:?}",
            event_types
        );
    }

    /// When an agent delegates to a child agent, the parent run must receive
    /// both `delegation_started` and `delegation_completed` events, in that order,
    /// and the child run must have its own events persisted to the DB
    /// (so the frontend can stream them via `childRunId`).
    #[tokio::test]
    async fn test_delegation_events_propagate_to_parent_and_child() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        /// Executor:
        /// - Parent agent → suspends with DelegationTarget::Agent (builder)
        /// - Child builder agent → emits some events then completes
        /// - Parent Resume → completes
        struct DelegationEventsPropagationExecutor;

        #[async_trait]
        impl TaskExecutor for DelegationEventsPropagationExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let spec = assignment.spec.clone();
                tokio::spawn(async move {
                    match spec {
                        TaskSpec::Agent { ref agent_id, .. } if agent_id == "__builder__" => {
                            // Child builder: emit a tool_used event then complete.
                            let _ = event_tx
                                .send((
                                    "tool_used".into(),
                                    json!({"tool_name": "read_file", "summary": "Read config.yml"}),
                                ))
                                .await;
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Done {
                                    answer: "built the metric".into(),
                                    metadata: None,
                                })
                                .await;
                        }
                        TaskSpec::Agent { .. } => {
                            // Parent: emit awaiting_input then suspend with delegation.
                            let _ = event_tx
                                .send((
                                    "awaiting_input".into(),
                                    json!({
                                        "questions": [{"prompt": "delegating to builder", "suggestions": []}],
                                        "from_state": "solving",
                                        "trace_id": "t1",
                                    }),
                                ))
                                .await;
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Suspended {
                                    reason: SuspendReason::Delegation {
                                        target: agentic_core::delegation::DelegationTarget::Agent {
                                            agent_id: "__builder__".into(),
                                        },
                                        request: "create missing metric".into(),
                                        context: json!({}),
                                        policy: None,
                                    },
                                    resume_data: make_suspend_data(),
                                    trace_id: "t1".into(),
                                })
                                .await;
                        }
                        TaskSpec::Resume { .. } => {
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Done {
                                    answer: "analytics answer".into(),
                                    metadata: None,
                                })
                                .await;
                        }
                        _ => {
                            drop(event_tx);
                            let _ = outcome_tx
                                .send(TaskOutcome::Failed("unexpected spec".into()))
                                .await;
                        }
                    }
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let executor = Arc::new(DelegationEventsPropagationExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "analytics".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        tokio::time::timeout(
            Duration::from_secs(10),
            tokio::spawn(async move { coordinator.run().await }),
        )
        .await
        .expect("coordinator timed out")
        .expect("coordinator panicked");

        // ── Parent run checks ────────────────────────────────────────────────

        let parent_run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(parent_run.task_status.as_deref()),
            "done",
            "parent run should complete after delegation"
        );

        let parent_events = crud::get_all_events(&db, &run_id).await.unwrap();
        let parent_types: Vec<&str> = parent_events
            .iter()
            .map(|e| e.event_type.as_str())
            .collect();

        // delegation_started must be present and carry child_task_id.
        let started = parent_events
            .iter()
            .find(|e| e.event_type == "delegation_started")
            .expect("parent must have delegation_started event");
        let child_run_id = started.payload["child_task_id"]
            .as_str()
            .expect("delegation_started must have child_task_id");
        assert_eq!(
            started.payload["target"].as_str(),
            Some("agent:__builder__"),
            "delegation target should be the builder agent"
        );

        // delegation_completed must follow delegation_started.
        let started_pos = parent_types
            .iter()
            .position(|t| *t == "delegation_started")
            .unwrap();
        let completed_pos = parent_types
            .iter()
            .position(|t| *t == "delegation_completed")
            .expect("parent must have delegation_completed event");
        assert!(
            completed_pos > started_pos,
            "delegation_completed ({completed_pos}) must come after delegation_started ({started_pos}): {parent_types:?}"
        );

        // delegation_completed must report success.
        let completed = parent_events
            .iter()
            .find(|e| e.event_type == "delegation_completed")
            .unwrap();
        assert_eq!(
            completed.payload["success"].as_bool(),
            Some(true),
            "delegation_completed should report success=true"
        );

        // ── Child run checks ─────────────────────────────────────────────────

        let child_run = crud::get_run(&db, child_run_id)
            .await
            .unwrap()
            .expect("child run should exist in DB");
        assert_eq!(
            child_run.parent_run_id.as_deref(),
            Some(run_id.as_str()),
            "child run must reference parent"
        );
        assert_eq!(
            crud::user_facing_status(child_run.task_status.as_deref()),
            "done",
            "child run should be done"
        );

        // Child run must have its own events persisted (frontend streams these).
        let child_events = crud::get_all_events(&db, child_run_id).await.unwrap();
        let child_types: Vec<&str> = child_events.iter().map(|e| e.event_type.as_str()).collect();
        assert!(
            child_types.contains(&"tool_used"),
            "child run should have tool_used event: {child_types:?}"
        );
    }
}

// ── Task tree persistence tests ─────────────────────────────────────────────

mod task_tree_tests {
    use super::*;

    #[tokio::test]
    async fn test_insert_run_with_parent() {
        let Some(db) = test_db().await else {
            return;
        };
        let parent_id = test_run_id();
        let child_id = test_run_id();

        crud::insert_run(
            &db,
            &parent_id,
            "parent Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        crud::insert_run_with_parent(
            &db,
            &child_id,
            &parent_id,
            "child Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let child = crud::get_run(&db, &child_id).await.unwrap().unwrap();
        assert_eq!(child.parent_run_id.as_deref(), Some(parent_id.as_str()));
        assert_eq!(child.task_status.as_deref(), Some("running"));
    }

    #[tokio::test]
    async fn test_update_task_status_round_trip() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        // running → suspended_human
        crud::update_task_status(&db, &run_id, "awaiting_input", None)
            .await
            .unwrap();
        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(run.task_status.as_deref(), Some("awaiting_input"));

        // suspended_human → waiting_on_child with metadata
        let meta = json!({ "child_task_id": "child-1" });
        crud::update_task_status(&db, &run_id, "delegating", Some(meta.clone()))
            .await
            .unwrap();
        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(run.task_status.as_deref(), Some("delegating"));
        assert_eq!(run.task_metadata.unwrap()["child_task_id"], "child-1");

        // waiting_on_child → done
        crud::update_task_status(&db, &run_id, "done", None)
            .await
            .unwrap();
        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(run.task_status.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn test_load_task_tree_three_levels() {
        let Some(db) = test_db().await else {
            return;
        };
        let root_id = test_run_id();
        let child_id = test_run_id();
        let grandchild_id = test_run_id();

        crud::insert_run(
            &db,
            &root_id,
            "root Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &child_id,
            &root_id,
            "child Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &grandchild_id,
            &child_id,
            "grandchild Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let tree = crud::load_task_tree(&db, &root_id).await.unwrap();
        assert_eq!(tree.len(), 3, "tree should have 3 nodes");

        let ids: Vec<&str> = tree.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&root_id.as_str()));
        assert!(ids.contains(&child_id.as_str()));
        assert!(ids.contains(&grandchild_id.as_str()));
    }

    #[tokio::test]
    async fn test_get_active_root_runs() {
        let Some(db) = test_db().await else {
            return;
        };
        let active_id = test_run_id();
        let done_id = test_run_id();

        crud::insert_run(
            &db,
            &active_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run(
            &db,
            &done_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(&db, &done_id, "done", None)
            .await
            .unwrap();

        let active = crud::get_active_root_runs(&db).await.unwrap();
        let active_ids: Vec<&str> = active.iter().map(|r| r.id.as_str()).collect();
        assert!(
            active_ids.contains(&active_id.as_str()),
            "active run should be found"
        );
        assert!(
            !active_ids.contains(&done_id.as_str()),
            "done run should not be found"
        );
    }

    #[tokio::test]
    async fn test_coordinator_from_db_reconstruction() {
        let Some(db) = test_db().await else {
            return;
        };
        let root_id = test_run_id();
        let child_id = test_run_id();

        // Set up a task tree in the DB: root waiting on child, child running.
        crud::insert_run(
            &db,
            &root_id,
            "root Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(
            &db,
            &root_id,
            "delegating",
            Some(json!({ "child_task_id": &child_id })),
        )
        .await
        .unwrap();

        crud::insert_run_with_parent(
            &db,
            &child_id,
            &root_id,
            "child Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        // Insert some events so next_seq can be computed.
        crud::insert_event(&db, &root_id, 0, "test_event", &json!({}), 0)
            .await
            .unwrap();
        crud::insert_event(&db, &root_id, 1, "test_event", &json!({}), 0)
            .await
            .unwrap();

        // Reconstruct coordinator from DB.
        use agentic_runtime::coordinator::Coordinator;
        use agentic_runtime::state::RuntimeState;
        use agentic_runtime::transport::LocalTransport;
        use std::sync::Arc;

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let (coordinator, pending_resumes) = Coordinator::from_db(
            db.clone(),
            state,
            transport as Arc<dyn agentic_core::transport::CoordinatorTransport>,
            &root_id,
        )
        .await
        .unwrap();

        // No children have completed, so no pending resumes expected.
        assert!(
            pending_resumes.is_empty(),
            "no pending resumes expected for running children"
        );

        // Verify by checking the coordinator was created successfully.
        // The coordinator is opaque, but from_db() would have failed
        // if the task tree was malformed.
        drop(coordinator);
    }

    /// Integration test: delegation through the coordinator persists
    /// parent_run_id and task_status correctly.
    #[tokio::test]
    async fn test_coordinator_delegation_persists_task_tree() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        use agentic_core::delegation::{SuspendReason, TaskAssignment, TaskOutcome, TaskSpec};
        use agentic_core::human_input::SuspendedRunData;
        use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
        use agentic_runtime::coordinator::Coordinator;
        use agentic_runtime::state::RuntimeState;
        use agentic_runtime::transport::LocalTransport;
        use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
        use async_trait::async_trait;
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::sync::mpsc;
        use tokio_util::sync::CancellationToken;

        fn make_suspend_data() -> SuspendedRunData {
            SuspendedRunData {
                from_state: "executing".into(),
                original_input: "test question".into(),
                trace_id: "trace-1".into(),
                stage_data: json!({}),
                question: "delegate this".into(),
                suggestions: vec![],
            }
        }

        struct DelegationExecutor;

        #[async_trait]
        impl TaskExecutor for DelegationExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let spec = assignment.spec.clone();
                tokio::spawn(async move {
                    drop(event_tx);
                    let outcome = match spec {
                        TaskSpec::Agent { .. } => TaskOutcome::Suspended {
                            reason: SuspendReason::Delegation {
                                target: agentic_core::delegation::DelegationTarget::Automation {
                                    workflow_ref: "test.automation.yml".into(),
                                },
                                request: "run workflow".into(),
                                context: json!({}),
                                policy: None,
                            },
                            resume_data: make_suspend_data(),
                            trace_id: "trace-1".into(),
                        },
                        TaskSpec::Resume { .. } => TaskOutcome::Done {
                            answer: "resumed after delegation".into(),
                            metadata: None,
                        },
                        TaskSpec::Automation { .. } => TaskOutcome::Done {
                            answer: "workflow done".into(),
                            metadata: None,
                        },
                        TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                            answer: "step done".into(),
                            metadata: None,
                        },
                        _ => {
                            unreachable!("AutomationDecision/Custom not used in runtime tests")
                        }
                        TaskSpec::Airway { .. } => {
                            unreachable!("Airway not used in runtime tests")
                        }
                    };
                    let _ = outcome_tx.send(outcome).await;
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(DelegationExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done"
        );
        assert_eq!(root.task_status.as_deref(), Some("done"));

        let tree = crud::load_task_tree(&db, &run_id).await.unwrap();
        assert!(tree.len() >= 2, "should have root + at least 1 child");

        let child = tree.iter().find(|r| r.id != run_id).unwrap();
        assert_eq!(child.parent_run_id.as_deref(), Some(run_id.as_str()));
        assert_eq!(child.task_status.as_deref(), Some("done"));
    }
}

// ── Parallel delegation (fan-out) tests ─────────────────────────────────────

mod fanout_tests {
    use super::*;
    use agentic_core::delegation::{
        DelegationItem, DelegationTarget, FanoutFailurePolicy, SuspendReason, TaskAssignment,
        TaskOutcome, TaskSpec,
    };
    use agentic_core::human_input::SuspendedRunData;
    use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
    use agentic_runtime::coordinator::Coordinator;
    use agentic_runtime::state::RuntimeState;
    use agentic_runtime::transport::LocalTransport;
    use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn make_suspend_data() -> SuspendedRunData {
        SuspendedRunData {
            from_state: "executing".into(),
            original_input: "test question".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({}),
            question: "parallel delegate".into(),
            suggestions: vec![],
        }
    }

    /// Executor that suspends with ParallelDelegation on Agent spec,
    /// completes immediately on Automation and Resume specs.
    struct ParallelDelegationExecutor {
        child_count: usize,
    }

    #[async_trait]
    impl TaskExecutor for ParallelDelegationExecutor {
        async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
            let (event_tx, event_rx) = mpsc::channel(16);
            let (outcome_tx, outcome_rx) = mpsc::channel(4);
            let cancel = CancellationToken::new();

            let spec = assignment.spec.clone();
            let child_count = self.child_count;
            tokio::spawn(async move {
                drop(event_tx);
                let outcome = match spec {
                    TaskSpec::Agent { .. } => {
                        let targets: Vec<DelegationItem> = (0..child_count)
                            .map(|i| DelegationItem {
                                target: DelegationTarget::Automation {
                                    workflow_ref: format!("workflow_{i}.yml"),
                                },
                                request: format!("run workflow {i}"),
                                context: json!({}),
                            })
                            .collect();
                        TaskOutcome::Suspended {
                            reason: SuspendReason::ParallelDelegation {
                                targets,
                                failure_policy: FanoutFailurePolicy::BestEffort,
                            },
                            resume_data: make_suspend_data(),
                            trace_id: "trace-1".into(),
                        }
                    }
                    TaskSpec::Resume { .. } => TaskOutcome::Done {
                        answer: "resumed after parallel delegation".into(),
                        metadata: None,
                    },
                    TaskSpec::Automation { workflow_ref, .. } => TaskOutcome::Done {
                        answer: format!("result from {workflow_ref}"),
                        metadata: None,
                    },
                    TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                        answer: "step done".into(),
                        metadata: None,
                    },
                    _ => {
                        unreachable!("AutomationDecision/Custom not used in runtime tests")
                    }
                    TaskSpec::Airway { .. } => {
                        unreachable!("Airway not used in runtime tests")
                    }
                };
                let _ = outcome_tx.send(outcome).await;
            });

            Ok(ExecutingTask {
                events: event_rx,
                outcomes: outcome_rx,
                cancel,
                answers: None,
            })
        }
    }

    /// Executor that fails on certain automation refs (for fail-fast testing).
    struct FailFastDelegationExecutor;

    #[async_trait]
    impl TaskExecutor for FailFastDelegationExecutor {
        async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
            let (event_tx, event_rx) = mpsc::channel(16);
            let (outcome_tx, outcome_rx) = mpsc::channel(4);
            let cancel = CancellationToken::new();

            let spec = assignment.spec.clone();
            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                drop(event_tx);
                let outcome = match spec {
                    TaskSpec::Agent { .. } => {
                        let targets = vec![
                            DelegationItem {
                                target: DelegationTarget::Automation {
                                    workflow_ref: "good.yml".into(),
                                },
                                request: "good".into(),
                                context: json!({}),
                            },
                            DelegationItem {
                                target: DelegationTarget::Automation {
                                    workflow_ref: "bad.yml".into(),
                                },
                                request: "bad".into(),
                                context: json!({}),
                            },
                        ];
                        TaskOutcome::Suspended {
                            reason: SuspendReason::ParallelDelegation {
                                targets,
                                failure_policy: FanoutFailurePolicy::FailFast,
                            },
                            resume_data: make_suspend_data(),
                            trace_id: "trace-1".into(),
                        }
                    }
                    TaskSpec::Resume { .. } => TaskOutcome::Done {
                        answer: "resumed after fail-fast".into(),
                        metadata: None,
                    },
                    TaskSpec::Automation { workflow_ref, .. } => {
                        if workflow_ref.contains("bad") {
                            // Small delay so the good automation might complete first.
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            TaskOutcome::Failed("workflow failed".into())
                        } else {
                            // Wait a bit then check cancellation.
                            tokio::select! {
                                _ = cancel_clone.cancelled() => TaskOutcome::Cancelled,
                                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                                    TaskOutcome::Done {
                                        answer: format!("result from {workflow_ref}"),
                                        metadata: None,
                                    }
                                }
                            }
                        }
                    }
                    TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                        answer: "step done".into(),
                        metadata: None,
                    },
                    _ => {
                        unreachable!("AutomationDecision/Custom not used in runtime tests")
                    }
                    TaskSpec::Airway { .. } => {
                        unreachable!("Airway not used in runtime tests")
                    }
                };
                let _ = outcome_tx.send(outcome).await;
            });

            Ok(ExecutingTask {
                events: event_rx,
                outcomes: outcome_rx,
                cancel,
                answers: None,
            })
        }
    }

    #[tokio::test]
    async fn test_parallel_delegation_all_succeed() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(ParallelDelegationExecutor { child_count: 3 });
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done",
            "root should be done"
        );
        assert_eq!(root.task_status.as_deref(), Some("done"));

        let tree = crud::load_task_tree(&db, &run_id).await.unwrap();
        assert_eq!(tree.len(), 4, "should have root + 3 children");

        let children: Vec<_> = tree.iter().filter(|r| r.id != run_id).collect();
        assert_eq!(children.len(), 3);
        for child in &children {
            assert_eq!(child.parent_run_id.as_deref(), Some(run_id.as_str()));
            assert_eq!(child.task_status.as_deref(), Some("done"));
        }

        // Verify delegation_started events were emitted for each child.
        let events = crud::get_all_events(&db, &run_id).await.unwrap();
        let delegation_started_count = events
            .iter()
            .filter(|e| e.event_type == "delegation_started")
            .count();
        assert_eq!(
            delegation_started_count, 3,
            "should have 3 delegation_started events"
        );

        let delegation_completed_count = events
            .iter()
            .filter(|e| e.event_type == "delegation_completed")
            .count();
        assert_eq!(
            delegation_completed_count, 3,
            "should have 3 delegation_completed events"
        );
    }

    #[tokio::test]
    async fn test_parallel_delegation_fail_fast() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(FailFastDelegationExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        // Root should complete (resumed after fail-fast).
        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done",
            "root should complete after fail-fast resume"
        );

        let tree = crud::load_task_tree(&db, &run_id).await.unwrap();
        assert!(tree.len() >= 3, "should have root + 2 children");

        // The bad automation child should be failed.
        let children: Vec<_> = tree.iter().filter(|r| r.id != run_id).collect();
        let failed_children: Vec<_> = children
            .iter()
            .filter(|c| c.task_status.as_deref() == Some("failed"))
            .collect();
        assert!(
            !failed_children.is_empty(),
            "should have at least one failed child"
        );
    }

    #[tokio::test]
    async fn test_parallel_delegation_single_child_backward_compat() {
        // Single-child delegation still works with the new WaitingOnChildren status.
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(ParallelDelegationExecutor { child_count: 1 });
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done"
        );

        let tree = crud::load_task_tree(&db, &run_id).await.unwrap();
        assert_eq!(tree.len(), 2, "root + 1 child");
    }
}

// ── Retry and fallback tests ────────────────────────────────────────────────

mod retry_tests {
    use super::*;
    use agentic_core::delegation::{
        BackoffStrategy, DelegationTarget, RetryPolicy, SuspendReason, TaskAssignment, TaskOutcome,
        TaskPolicy, TaskSpec,
    };
    use agentic_core::human_input::SuspendedRunData;
    use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
    use agentic_runtime::coordinator::Coordinator;
    use agentic_runtime::state::RuntimeState;
    use agentic_runtime::transport::LocalTransport;
    use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn make_suspend_data() -> SuspendedRunData {
        SuspendedRunData {
            from_state: "executing".into(),
            original_input: "test question".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({}),
            question: "delegate".into(),
            suggestions: vec![],
        }
    }

    /// Executor that delegates with a retry policy. The automation fails
    /// `fail_count` times then succeeds.
    struct RetryExecutor {
        fail_count: u32,
        attempt_counter: Arc<AtomicU32>,
    }

    #[async_trait]
    impl TaskExecutor for RetryExecutor {
        async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
            let (event_tx, event_rx) = mpsc::channel(16);
            let (outcome_tx, outcome_rx) = mpsc::channel(4);
            let cancel = CancellationToken::new();

            let spec = assignment.spec.clone();
            let fail_count = self.fail_count;
            let counter = self.attempt_counter.clone();
            tokio::spawn(async move {
                drop(event_tx);
                let outcome = match spec {
                    TaskSpec::Agent { .. } => TaskOutcome::Suspended {
                        reason: SuspendReason::Delegation {
                            target: DelegationTarget::Automation {
                                workflow_ref: "flaky.yml".into(),
                            },
                            request: "run flaky workflow".into(),
                            context: json!({}),
                            policy: Some(TaskPolicy {
                                retry: Some(RetryPolicy {
                                    max_retries: 3,
                                    backoff: BackoffStrategy::Fixed { delay_ms: 10 },
                                    retry_on: vec![],
                                }),
                                fallback_targets: vec![],
                            }),
                        },
                        resume_data: make_suspend_data(),
                        trace_id: "trace-1".into(),
                    },
                    TaskSpec::Resume { .. } => TaskOutcome::Done {
                        answer: "resumed after retry".into(),
                        metadata: None,
                    },
                    TaskSpec::Automation { .. } => {
                        let attempt = counter.fetch_add(1, Ordering::SeqCst);
                        if attempt < fail_count {
                            TaskOutcome::Failed(format!("attempt {attempt} failed"))
                        } else {
                            TaskOutcome::Done {
                                answer: format!("succeeded on attempt {attempt}"),
                                metadata: None,
                            }
                        }
                    }
                    TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                        answer: "step done".into(),
                        metadata: None,
                    },
                    _ => {
                        unreachable!("AutomationDecision/Custom not used in runtime tests")
                    }
                    TaskSpec::Airway { .. } => {
                        unreachable!("Airway not used in runtime tests")
                    }
                };
                let _ = outcome_tx.send(outcome).await;
            });

            Ok(ExecutingTask {
                events: event_rx,
                outcomes: outcome_rx,
                cancel,
                answers: None,
            })
        }
    }

    #[tokio::test]
    async fn test_retry_succeeds_on_second_attempt() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let counter = Arc::new(AtomicU32::new(0));

        let executor = Arc::new(RetryExecutor {
            fail_count: 1, // Fail once, then succeed.
            attempt_counter: counter.clone(),
        });
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        // Root should be done (retry succeeded).
        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done",
            "root should complete after retry"
        );

        // The automation was called twice (1 fail + 1 success).
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        // Verify delegation_retry event was emitted.
        let events = crud::get_all_events(&db, &run_id).await.unwrap();
        let retry_events: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "delegation_retry")
            .collect();
        assert_eq!(
            retry_events.len(),
            1,
            "should have 1 delegation_retry event"
        );
    }

    #[tokio::test]
    async fn test_retry_exhausted_then_fallback() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        /// Executor that always fails on the primary automation but succeeds on fallback.
        struct FallbackExecutor;

        #[async_trait]
        impl TaskExecutor for FallbackExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let spec = assignment.spec.clone();
                tokio::spawn(async move {
                    drop(event_tx);
                    let outcome = match spec {
                        TaskSpec::Agent { .. } => TaskOutcome::Suspended {
                            reason: SuspendReason::Delegation {
                                target: DelegationTarget::Automation {
                                    workflow_ref: "primary.yml".into(),
                                },
                                request: "run primary".into(),
                                context: json!({}),
                                policy: Some(TaskPolicy {
                                    retry: Some(RetryPolicy {
                                        max_retries: 1,
                                        backoff: BackoffStrategy::Fixed { delay_ms: 10 },
                                        retry_on: vec![],
                                    }),
                                    fallback_targets: vec![DelegationTarget::Automation {
                                        workflow_ref: "fallback.yml".into(),
                                    }],
                                }),
                            },
                            resume_data: make_suspend_data(),
                            trace_id: "trace-1".into(),
                        },
                        TaskSpec::Resume { .. } => TaskOutcome::Done {
                            answer: "resumed after fallback".into(),
                            metadata: None,
                        },
                        TaskSpec::Automation { workflow_ref, .. } => {
                            if workflow_ref.contains("primary") {
                                TaskOutcome::Failed("primary always fails".into())
                            } else {
                                TaskOutcome::Done {
                                    answer: format!("fallback {workflow_ref} succeeded"),
                                    metadata: None,
                                }
                            }
                        }
                        TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                            answer: "step done".into(),
                            metadata: None,
                        },
                        _ => {
                            unreachable!("AutomationDecision/Custom not used in runtime tests")
                        }
                        TaskSpec::Airway { .. } => {
                            unreachable!("Airway not used in runtime tests")
                        }
                    };
                    let _ = outcome_tx.send(outcome).await;
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(FallbackExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        // Root should be done (fallback succeeded).
        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done",
            "root should complete after fallback succeeds"
        );

        // Verify both retry and fallback events were emitted.
        let events = crud::get_all_events(&db, &run_id).await.unwrap();
        let retry_count = events
            .iter()
            .filter(|e| e.event_type == "delegation_retry")
            .count();
        let fallback_count = events
            .iter()
            .filter(|e| e.event_type == "delegation_fallback")
            .count();

        assert!(retry_count >= 1, "should have retry events");
        assert_eq!(fallback_count, 1, "should have 1 fallback event");
    }

    #[tokio::test]
    async fn test_retry_pattern_matching() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        /// Executor that fails with a specific error and only retries on matching patterns.
        struct PatternExecutor {
            counter: Arc<AtomicU32>,
        }

        #[async_trait]
        impl TaskExecutor for PatternExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let spec = assignment.spec.clone();
                let counter = self.counter.clone();
                tokio::spawn(async move {
                    drop(event_tx);
                    let outcome = match spec {
                        TaskSpec::Agent { .. } => TaskOutcome::Suspended {
                            reason: SuspendReason::Delegation {
                                target: DelegationTarget::Automation {
                                    workflow_ref: "pattern_test.yml".into(),
                                },
                                request: "test".into(),
                                context: json!({}),
                                policy: Some(TaskPolicy {
                                    retry: Some(RetryPolicy {
                                        max_retries: 3,
                                        backoff: BackoffStrategy::Fixed { delay_ms: 10 },
                                        retry_on: vec!["transient".into()],
                                    }),
                                    fallback_targets: vec![],
                                }),
                            },
                            resume_data: SuspendedRunData {
                                from_state: "executing".into(),
                                original_input: "test".into(),
                                trace_id: "t".into(),
                                stage_data: json!({}),
                                question: "q".into(),
                                suggestions: vec![],
                            },
                            trace_id: "t".into(),
                        },
                        TaskSpec::Resume { .. } => TaskOutcome::Done {
                            answer: "done".into(),
                            metadata: None,
                        },
                        TaskSpec::Automation { .. } => {
                            let attempt = counter.fetch_add(1, Ordering::SeqCst);
                            if attempt == 0 {
                                // First failure: "transient" — should retry.
                                TaskOutcome::Failed("transient error".into())
                            } else {
                                // Second failure: "permanent" — should NOT retry.
                                TaskOutcome::Failed("permanent error".into())
                            }
                        }
                        TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                            answer: "step done".into(),
                            metadata: None,
                        },
                        _ => {
                            unreachable!("AutomationDecision/Custom not used in runtime tests")
                        }
                        TaskSpec::Airway { .. } => {
                            unreachable!("Airway not used in runtime tests")
                        }
                    };
                    let _ = outcome_tx.send(outcome).await;
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let counter = Arc::new(AtomicU32::new(0));

        let executor = Arc::new(PatternExecutor {
            counter: counter.clone(),
        });
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        // Should have retried once (transient), then failed on permanent.
        assert_eq!(counter.load(Ordering::SeqCst), 2, "should have 2 attempts");

        // Root should be done (parent resumed with failure message).
        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done",
            "root should complete (resumed with error)"
        );
    }
}

// ── Crash recovery tests (task_outcomes table) ────────────────────────────────
//
// These tests simulate the crash windows at the parent-child boundary by
// manually setting up DB state that represents a mid-crash scenario, then
// verifying that `Coordinator::from_db` correctly reconstructs state and
// detects pending resumes.

mod crash_recovery_tests {
    use super::*;
    use agentic_core::human_input::SuspendedRunData;
    use agentic_core::transport::CoordinatorTransport;
    use agentic_runtime::coordinator::Coordinator;
    use agentic_runtime::state::RuntimeState;
    use agentic_runtime::transport::LocalTransport;
    use std::sync::Arc;

    fn make_suspend_data() -> SuspendedRunData {
        SuspendedRunData {
            from_state: "executing".into(),
            original_input: "test question".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({}),
            question: "delegate this".into(),
            suggestions: vec![],
        }
    }

    // ── CRUD round-trip for task_outcomes ────────────────────────────────

    #[tokio::test]
    async fn test_task_outcome_crud_round_trip() {
        let Some(db) = test_db().await else {
            return;
        };
        let parent_id = test_run_id();
        let child_id = format!("{parent_id}.1");

        // Create parent and child runs.
        crud::insert_run(
            &db,
            &parent_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &child_id,
            &parent_id,
            "child Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        crud::insert_task_outcome(&db, &child_id, &parent_id, "done", Some("42"))
            .await
            .unwrap();

        // Read it back.
        let outcomes = crud::get_outcomes_for_parent(&db, &parent_id)
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].child_id, child_id);
        assert_eq!(outcomes[0].parent_id, parent_id);
        assert_eq!(outcomes[0].status, "done");
        assert_eq!(outcomes[0].answer.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn test_task_outcome_upsert_is_idempotent() {
        let Some(db) = test_db().await else {
            return;
        };
        let parent_id = test_run_id();
        let child_id = format!("{parent_id}.1");

        crud::insert_run(
            &db,
            &parent_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &child_id,
            &parent_id,
            "child Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        // Insert twice — should not error.
        crud::insert_task_outcome(&db, &child_id, &parent_id, "done", Some("first"))
            .await
            .unwrap();
        crud::insert_task_outcome(&db, &child_id, &parent_id, "done", Some("updated"))
            .await
            .unwrap();

        let outcomes = crud::get_outcomes_for_parent(&db, &parent_id)
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1);
        // Upsert should update to the latest value.
        assert_eq!(outcomes[0].answer.as_deref(), Some("updated"));
    }

    // ── Crash Window A: child done in DB, parent metadata NOT updated ───
    //
    // Simulates: child task completed, `insert_task_outcome` succeeded,
    // but process crashed before `record_child_result` updated parent's
    // task_metadata.completed. On recovery, from_db should reconstruct
    // the completed state from task_outcomes and detect a pending resume.

    #[tokio::test]
    async fn test_crash_window_a_single_child_done_parent_not_updated() {
        let Some(db) = test_db().await else {
            return;
        };
        let parent_id = test_run_id();
        let child_id = format!("{parent_id}.1");

        crud::insert_run(
            &db,
            &parent_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        // Parent is delegating, but task_metadata.completed is EMPTY
        // (simulating crash before record_child_result).
        crud::update_task_status(
            &db,
            &parent_id,
            "delegating",
            Some(json!({
                "child_task_ids": [&child_id],
                "completed": {},
                "failure_policy": "fail_fast",
            })),
        )
        .await
        .unwrap();

        // Persist suspension data so resume_parent can work.
        crud::upsert_suspension(&db, &parent_id, "delegate this", &[], &make_suspend_data())
            .await
            .unwrap();

        // Set up child run (marked done in DB).
        crud::insert_run_with_parent(
            &db,
            &child_id,
            &parent_id,
            "child Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(&db, &child_id, "done", None)
            .await
            .unwrap();

        // The task outcome WAS written (this is what makes recovery possible).
        crud::insert_task_outcome(&db, &child_id, &parent_id, "done", Some("child answer"))
            .await
            .unwrap();

        // Recover via from_db.
        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let (coordinator, pending_resumes) = Coordinator::from_db(
            db.clone(),
            state,
            transport as Arc<dyn CoordinatorTransport>,
            &parent_id,
        )
        .await
        .unwrap();

        // The key assertion: from_db detected that the child completed
        // (from task_outcomes) and the parent should be resumed.
        assert_eq!(
            pending_resumes.len(),
            1,
            "should detect 1 pending resume for completed child"
        );
        assert_eq!(pending_resumes[0].parent_task_id, parent_id);
        assert_eq!(pending_resumes[0].answer, "child answer");

        drop(coordinator);
    }

    // ── Crash Window A variant: child done but NO task_outcome written ──
    //
    // Simulates: child completed, outcome was NOT yet written (crash
    // happened before insert_task_outcome). In this case, from_db should
    // NOT detect a pending resume — the child is still considered running.

    #[tokio::test]
    async fn test_crash_window_no_outcome_no_resume() {
        let Some(db) = test_db().await else {
            return;
        };
        let parent_id = test_run_id();
        let child_id = format!("{parent_id}.1");

        crud::insert_run(
            &db,
            &parent_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(
            &db,
            &parent_id,
            "delegating",
            Some(json!({
                "child_task_ids": [&child_id],
                "completed": {},
                "failure_policy": "fail_fast",
            })),
        )
        .await
        .unwrap();

        // Child exists but no task_outcome — simulating crash before outcome write.
        crud::insert_run_with_parent(
            &db,
            &child_id,
            &parent_id,
            "child Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        // Child is still "running" in the DB.

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let (_coordinator, pending_resumes) = Coordinator::from_db(
            db.clone(),
            state,
            transport as Arc<dyn CoordinatorTransport>,
            &parent_id,
        )
        .await
        .unwrap();

        // No pending resumes — the child hasn't reported back.
        assert!(
            pending_resumes.is_empty(),
            "no resume expected when outcome is missing"
        );
    }

    // ── Crash during fan-out: some children done, some still running ────

    #[tokio::test]
    async fn test_crash_fanout_partial_completion() {
        let Some(db) = test_db().await else {
            return;
        };
        let parent_id = test_run_id();
        let child1_id = format!("{parent_id}.1");
        let child2_id = format!("{parent_id}.2");
        let child3_id = format!("{parent_id}.3");

        crud::insert_run(
            &db,
            &parent_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(
            &db,
            &parent_id,
            "delegating",
            Some(json!({
                "child_task_ids": [&child1_id, &child2_id, &child3_id],
                "completed": {},
                "failure_policy": "best_effort",
            })),
        )
        .await
        .unwrap();

        // Create children.
        for child_id in [&child1_id, &child2_id, &child3_id] {
            crud::insert_run_with_parent(
                &db,
                child_id,
                &parent_id,
                "child Q",
                "analytics",
                None,
                0,
                uuid::Uuid::nil(),
            )
            .await
            .unwrap();
        }

        // Only child1 and child2 completed, child3 still running.
        crud::update_run_done(&db, &child1_id, "result 1", None)
            .await
            .unwrap();
        crud::update_task_status(&db, &child1_id, "done", None)
            .await
            .unwrap();
        crud::insert_task_outcome(&db, &child1_id, &parent_id, "done", Some("result 1"))
            .await
            .unwrap();

        crud::update_run_done(&db, &child2_id, "result 2", None)
            .await
            .unwrap();
        crud::update_task_status(&db, &child2_id, "done", None)
            .await
            .unwrap();
        crud::insert_task_outcome(&db, &child2_id, &parent_id, "done", Some("result 2"))
            .await
            .unwrap();

        // child3: no outcome — still running.

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let (_coordinator, pending_resumes) = Coordinator::from_db(
            db.clone(),
            state,
            transport as Arc<dyn CoordinatorTransport>,
            &parent_id,
        )
        .await
        .unwrap();

        // Parent should NOT be resumed yet — child3 is still pending.
        assert!(
            pending_resumes.is_empty(),
            "no resume when fan-out is partially complete"
        );
    }

    // ── Crash during fan-out: ALL children done, parent never resumed ───

    #[tokio::test]
    async fn test_crash_fanout_all_complete_parent_not_resumed() {
        let Some(db) = test_db().await else {
            return;
        };
        let parent_id = test_run_id();
        let child1_id = format!("{parent_id}.1");
        let child2_id = format!("{parent_id}.2");

        crud::insert_run(
            &db,
            &parent_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(
            &db,
            &parent_id,
            "delegating",
            Some(json!({
                "child_task_ids": [&child1_id, &child2_id],
                "completed": {},
                "failure_policy": "best_effort",
            })),
        )
        .await
        .unwrap();

        crud::upsert_suspension(&db, &parent_id, "delegate this", &[], &make_suspend_data())
            .await
            .unwrap();

        for child_id in [&child1_id, &child2_id] {
            crud::insert_run_with_parent(
                &db,
                child_id,
                &parent_id,
                "child Q",
                "analytics",
                None,
                0,
                uuid::Uuid::nil(),
            )
            .await
            .unwrap();
            crud::update_run_done(&db, child_id, &format!("result from {child_id}"), None)
                .await
                .unwrap();
            crud::update_task_status(&db, child_id, "done", None)
                .await
                .unwrap();
        }

        // Both outcomes written.
        crud::insert_task_outcome(
            &db,
            &child1_id,
            &parent_id,
            "done",
            Some(&format!("result from {child1_id}")),
        )
        .await
        .unwrap();
        crud::insert_task_outcome(
            &db,
            &child2_id,
            &parent_id,
            "done",
            Some(&format!("result from {child2_id}")),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let (_coordinator, pending_resumes) = Coordinator::from_db(
            db.clone(),
            state,
            transport as Arc<dyn CoordinatorTransport>,
            &parent_id,
        )
        .await
        .unwrap();

        // Both children done → parent should be resumed.
        assert_eq!(
            pending_resumes.len(),
            1,
            "should detect pending resume for fully-completed fan-out"
        );
        assert_eq!(pending_resumes[0].parent_task_id, parent_id);

        // The aggregated answer should be JSON (multi-child format).
        let answer: serde_json::Value = serde_json::from_str(&pending_resumes[0].answer).unwrap();
        assert!(answer.is_object());
        assert!(answer[&child1_id]["status"] == "done");
        assert!(answer[&child2_id]["status"] == "done");
    }

    // ── Crash with failed child outcome ─────────────────────────────────

    #[tokio::test]
    async fn test_crash_recovery_with_failed_child() {
        let Some(db) = test_db().await else {
            return;
        };
        let parent_id = test_run_id();
        let child_id = format!("{parent_id}.1");

        crud::insert_run(
            &db,
            &parent_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(
            &db,
            &parent_id,
            "delegating",
            Some(json!({
                "child_task_ids": [&child_id],
                "completed": {},
                "failure_policy": "fail_fast",
            })),
        )
        .await
        .unwrap();

        crud::upsert_suspension(&db, &parent_id, "delegate this", &[], &make_suspend_data())
            .await
            .unwrap();

        crud::insert_run_with_parent(
            &db,
            &child_id,
            &parent_id,
            "child Q",
            "analytics",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_run_failed(&db, &child_id, "child exploded")
            .await
            .unwrap();
        crud::update_task_status(&db, &child_id, "failed", None)
            .await
            .unwrap();

        crud::insert_task_outcome(&db, &child_id, &parent_id, "failed", Some("child exploded"))
            .await
            .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let (_coordinator, pending_resumes) = Coordinator::from_db(
            db.clone(),
            state,
            transport as Arc<dyn CoordinatorTransport>,
            &parent_id,
        )
        .await
        .unwrap();

        assert_eq!(pending_resumes.len(), 1);
        assert!(
            pending_resumes[0].answer.contains("Delegation failed"),
            "should propagate failure message: {}",
            pending_resumes[0].answer
        );
    }

    // ── End-to-end: delegation writes task_outcome during normal flow ───

    #[tokio::test]
    async fn test_delegation_writes_task_outcome_during_normal_flow() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        use agentic_core::delegation::{SuspendReason, TaskAssignment, TaskOutcome, TaskSpec};
        use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
        use agentic_runtime::coordinator::Coordinator;
        use agentic_runtime::state::RuntimeState;
        use agentic_runtime::transport::LocalTransport;
        use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
        use async_trait::async_trait;
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::sync::mpsc;
        use tokio_util::sync::CancellationToken;

        struct DelegationExecutor;

        #[async_trait]
        impl TaskExecutor for DelegationExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let spec = assignment.spec.clone();
                tokio::spawn(async move {
                    drop(event_tx);
                    let outcome = match spec {
                        TaskSpec::Agent { .. } => TaskOutcome::Suspended {
                            reason: SuspendReason::Delegation {
                                target: agentic_core::delegation::DelegationTarget::Automation {
                                    workflow_ref: "test.automation.yml".into(),
                                },
                                request: "run workflow".into(),
                                context: json!({}),
                                policy: None,
                            },
                            resume_data: make_suspend_data(),
                            trace_id: "trace-1".into(),
                        },
                        TaskSpec::Resume { .. } => TaskOutcome::Done {
                            answer: "resumed".into(),
                            metadata: None,
                        },
                        TaskSpec::Automation { .. } => TaskOutcome::Done {
                            answer: "workflow done".into(),
                            metadata: None,
                        },
                        TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                            answer: "step done".into(),
                            metadata: None,
                        },
                        _ => {
                            unreachable!("AutomationDecision/Custom not used in runtime tests")
                        }
                        TaskSpec::Airway { .. } => {
                            unreachable!("Airway not used in runtime tests")
                        }
                    };
                    let _ = outcome_tx.send(outcome).await;
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(DelegationExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done"
        );

        // Verify task_outcome was written for the child.
        let outcomes = crud::get_outcomes_for_parent(&db, &run_id).await.unwrap();
        assert_eq!(
            outcomes.len(),
            1,
            "should have 1 task outcome for the delegation child"
        );
        assert_eq!(outcomes[0].status, "done");
        assert_eq!(outcomes[0].answer.as_deref(), Some("workflow done"));
    }

    /// Verifies that the normal delegation flow now correctly sets
    /// `task_status` on child runs.
    #[tokio::test]
    async fn test_delegation_child_run_has_terminal_status() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        use agentic_core::delegation::{SuspendReason, TaskAssignment, TaskOutcome, TaskSpec};
        use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
        use agentic_runtime::coordinator::Coordinator;
        use agentic_runtime::state::RuntimeState;
        use agentic_runtime::transport::LocalTransport;
        use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
        use async_trait::async_trait;
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::sync::mpsc;
        use tokio_util::sync::CancellationToken;

        struct DelegationExecutor;

        #[async_trait]
        impl TaskExecutor for DelegationExecutor {
            async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
                let (event_tx, event_rx) = mpsc::channel(16);
                let (outcome_tx, outcome_rx) = mpsc::channel(4);
                let cancel = CancellationToken::new();

                let spec = assignment.spec.clone();
                tokio::spawn(async move {
                    drop(event_tx);
                    let outcome = match spec {
                        TaskSpec::Agent { .. } => TaskOutcome::Suspended {
                            reason: SuspendReason::Delegation {
                                target: agentic_core::delegation::DelegationTarget::Automation {
                                    workflow_ref: "test.automation.yml".into(),
                                },
                                request: "run workflow".into(),
                                context: json!({}),
                                policy: None,
                            },
                            resume_data: make_suspend_data(),
                            trace_id: "trace-1".into(),
                        },
                        TaskSpec::Resume { .. } => TaskOutcome::Done {
                            answer: "resumed".into(),
                            metadata: None,
                        },
                        TaskSpec::Automation { .. } => TaskOutcome::Done {
                            answer: "workflow done".into(),
                            metadata: None,
                        },
                        TaskSpec::AutomationStep { .. } => TaskOutcome::Done {
                            answer: "step done".into(),
                            metadata: None,
                        },
                        _ => {
                            unreachable!("AutomationDecision/Custom not used in runtime tests")
                        }
                        TaskSpec::Airway { .. } => {
                            unreachable!("Airway not used in runtime tests")
                        }
                    };
                    let _ = outcome_tx.send(outcome).await;
                });

                Ok(ExecutingTask {
                    events: event_rx,
                    outcomes: outcome_rx,
                    cancel,
                    answers: None,
                })
            }
        }

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(DelegationExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out")
            .expect("coordinator panicked");

        let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(root.task_status.as_deref()),
            "done"
        );

        // Verify child run has task_status set to "done".
        let tree = crud::load_task_tree(&db, &run_id).await.unwrap();
        let children: Vec<_> = tree.iter().filter(|r| r.id != run_id).collect();
        assert!(!children.is_empty(), "should have at least 1 child");

        for child in &children {
            assert_eq!(
                crud::user_facing_status(child.task_status.as_deref()),
                "done",
                "child user-facing status should be done, got: {}",
                crud::user_facing_status(child.task_status.as_deref())
            );
            assert_eq!(
                child.task_status.as_deref(),
                Some("done"),
                "child task_status should be done"
            );
        }
    }
}

// ── Answer channel resume tests ─────────────────────────────────────────────

mod answer_channel_tests {
    use super::*;
    use agentic_core::delegation::{
        DelegationTarget, SuspendReason, TaskAssignment, TaskOutcome, TaskSpec,
    };
    use agentic_core::human_input::SuspendedRunData;
    use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
    use agentic_runtime::coordinator::Coordinator;
    use agentic_runtime::state::RuntimeState;
    use agentic_runtime::transport::LocalTransport;
    use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn make_suspend_data() -> SuspendedRunData {
        SuspendedRunData {
            from_state: "workflow".into(),
            original_input: "test".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({}),
            question: "delegate".into(),
            suggestions: vec![],
        }
    }

    /// A long-lived executor that simulates an automation orchestrator:
    /// - On Agent spec: suspends for Delegation, then waits for answer via channel
    /// - On Automation spec: completes immediately (child task)
    /// - On Resume: should NOT be called for this test
    /// - On AutomationStep: completes immediately
    struct LongLivedOrchestrator;

    #[async_trait]
    impl TaskExecutor for LongLivedOrchestrator {
        async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
            let (event_tx, event_rx) = mpsc::channel(16);
            let (outcome_tx, outcome_rx) = mpsc::channel(4);
            let (answer_tx, answer_rx) = mpsc::channel::<String>(4);
            let cancel = CancellationToken::new();

            let spec = assignment.spec.clone();
            let _run_id = assignment.run_id.clone();

            match spec {
                TaskSpec::Agent { .. } => {
                    // Long-lived: suspend, wait for answer, then complete.
                    tokio::spawn(async move {
                        let _ = event_tx
                            .send(("test_event".into(), json!({"task": "agent"})))
                            .await;

                        let _ = outcome_tx
                            .send(TaskOutcome::Suspended {
                                reason: SuspendReason::Delegation {
                                    target: DelegationTarget::Automation {
                                        workflow_ref: "test.automation.yml".into(),
                                    },
                                    request: "run workflow".into(),
                                    context: json!({}),
                                    policy: None,
                                },
                                resume_data: make_suspend_data(),
                                trace_id: "trace-1".into(),
                            })
                            .await;

                        // Wait for resume via answer channel — NOT TaskSpec::Resume.
                        let mut answer_rx = answer_rx;
                        let answer = answer_rx.recv().await;
                        match answer {
                            Some(a) => {
                                let _ = event_tx
                                    .send(("resumed".into(), json!({"answer": &a})))
                                    .await;
                                let _ = outcome_tx
                                    .send(TaskOutcome::Done {
                                        answer: format!("orchestrator got: {a}"),
                                        metadata: None,
                                    })
                                    .await;
                            }
                            None => {
                                let _ = outcome_tx
                                    .send(TaskOutcome::Failed(
                                        "answer channel closed unexpectedly".into(),
                                    ))
                                    .await;
                            }
                        }
                    });

                    Ok(ExecutingTask {
                        events: event_rx,
                        outcomes: outcome_rx,
                        cancel,
                        answers: Some(answer_tx),
                    })
                }
                TaskSpec::Automation { .. } => {
                    // Child task: complete immediately.
                    tokio::spawn(async move {
                        drop(event_tx);
                        let _ = outcome_tx
                            .send(TaskOutcome::Done {
                                answer: "workflow child done".into(),
                                metadata: None,
                            })
                            .await;
                    });

                    Ok(ExecutingTask {
                        events: event_rx,
                        outcomes: outcome_rx,
                        cancel,
                        answers: None,
                    })
                }
                TaskSpec::Resume { answer, .. } => {
                    // Resume path: the old answer-channel mechanism was removed.
                    // Now long-lived orchestrator tasks use TaskSpec::Resume for
                    // non-workflow_decision from_state (legacy analytics/builder).
                    let a = if answer.is_empty() {
                        "resumed-via-task-spec".to_string()
                    } else {
                        format!("orchestrator got: {answer}")
                    };
                    tokio::spawn(async move {
                        drop(event_tx);
                        let _ = outcome_tx
                            .send(TaskOutcome::Done {
                                answer: a,
                                metadata: None,
                            })
                            .await;
                    });

                    Ok(ExecutingTask {
                        events: event_rx,
                        outcomes: outcome_rx,
                        cancel,
                        answers: None,
                    })
                }
                TaskSpec::AutomationStep { .. } => {
                    tokio::spawn(async move {
                        drop(event_tx);
                        let _ = outcome_tx
                            .send(TaskOutcome::Done {
                                answer: "step done".into(),
                                metadata: None,
                            })
                            .await;
                    });

                    Ok(ExecutingTask {
                        events: event_rx,
                        outcomes: outcome_rx,
                        cancel,
                        answers: None,
                    })
                }
                _ => {
                    unreachable!("AutomationDecision/Custom not used in runtime tests")
                }
                TaskSpec::Airway { .. } => {
                    unreachable!("Airway not used in runtime tests")
                }
            }
        }
    }

    /// Test that a task suspended with `from_state = "workflow"` is resumed via
    /// `TaskSpec::Resume` (the non-workflow_decision path).
    ///
    /// The old answer-channel mechanism (`orchestrator_txs`) has been removed.
    /// Temporal-style automation tasks use `from_state = "workflow_decision"` and
    /// get resumed via `TaskSpec::AutomationDecision`. Everything else uses
    /// `TaskSpec::Resume`.
    #[tokio::test]
    async fn test_resume_via_task_spec_resume() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(LongLivedOrchestrator);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_coord_answer_tx, coord_answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), coord_answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        // Coordinator should:
        // 1. Root task (Agent) suspends for Delegation(Automation) with from_state="workflow"
        // 2. Coordinator spawns child Automation task
        // 3. Child completes with "workflow child done"
        // 4. resume_parent sees from_state="workflow" → assigns TaskSpec::Resume
        // 5. LongLivedOrchestrator::execute(Resume) completes with "orchestrator got: ..."

        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out — task is likely hanging")
            .expect("coordinator panicked");

        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(run.task_status.as_deref()),
            "done",
            "run should complete successfully, got: {:?}",
            run.task_status
        );
        assert!(
            run.answer
                .as_deref()
                .unwrap_or("")
                .contains("orchestrator got:"),
            "answer should come from Resume path, got: {:?}",
            run.answer
        );
    }

    /// Executor that waits for cancellation and emits TaskOutcome::Cancelled.
    struct CancellableExecutor;

    #[async_trait]
    impl TaskExecutor for CancellableExecutor {
        async fn execute(&self, _assignment: TaskAssignment) -> Result<ExecutingTask, String> {
            let (event_tx, event_rx) = mpsc::channel(16);
            let (outcome_tx, outcome_rx) = mpsc::channel(4);
            let cancel = CancellationToken::new();

            let cancel_clone = cancel.clone();
            tokio::spawn(async move {
                drop(event_tx);
                // Wait for cancellation — simulates a long-running task.
                cancel_clone.cancelled().await;
                let _ = outcome_tx.send(TaskOutcome::Cancelled).await;
            });

            Ok(ExecutingTask {
                events: event_rx,
                outcomes: outcome_rx,
                cancel,
                answers: None,
            })
        }
    }

    /// Verify that cancelling a running task via `transport.cancel()` propagates
    /// through the coordinator and sets the run status to "failed" (with a
    /// "cancelled" message) in the database.
    #[tokio::test]
    async fn test_coordinator_cancel_updates_run_status() {
        let Some(db) = test_db().await else {
            return;
        };
        let run_id = test_run_id();
        crud::insert_run(
            &db,
            &run_id,
            "test Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();

        let executor = Arc::new(CancellableExecutor);
        let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

        let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
        let mut coordinator = Coordinator::new(
            db.clone(),
            state.clone(),
            transport.clone() as Arc<dyn CoordinatorTransport>,
        );
        coordinator.register_answer_channel(run_id.clone(), answer_rx);

        coordinator
            .submit_root(
                run_id.clone(),
                TaskSpec::Agent {
                    agent_id: "test".into(),
                    question: "test Q".into(),
                    extra: None,
                },
            )
            .await
            .unwrap();

        tokio::spawn(async move { worker.run().await });
        let coord_handle = tokio::spawn(async move { coordinator.run().await });

        // Give the task time to start.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cancel via the transport — this is what the cancel_rx forwarding does.
        transport
            .cancel(&run_id)
            .await
            .expect("cancel should succeed");

        // Wait for coordinator to finish processing the cancellation.
        tokio::time::timeout(Duration::from_secs(10), coord_handle)
            .await
            .expect("coordinator timed out — cancel did not propagate")
            .expect("coordinator panicked");

        // Verify run status is "cancelled" with cancellation message in DB.
        let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
        assert_eq!(
            crud::user_facing_status(run.task_status.as_deref()),
            "cancelled",
            "cancelled run should have status 'cancelled'"
        );
        assert!(
            run.error_message
                .as_deref()
                .unwrap_or("")
                .contains("cancelled"),
            "error_message should mention 'cancelled', got: {:?}",
            run.error_message
        );

        // Verify in-memory status is also updated.
        let mem_status = state.statuses.get(&run_id).map(|s| s.clone());
        assert!(
            matches!(
                mem_status,
                Some(agentic_runtime::state::RunStatus::Cancelled)
            ),
            "in-memory status should be Cancelled, got: {:?}",
            mem_status
        );
    }
}

// ── Recovery: attempt tracking and child ID collision tests ───────────────

mod recovery_attempt_tests {
    use super::*;
    use agentic_core::transport::CoordinatorTransport;
    use agentic_runtime::coordinator::Coordinator;
    use agentic_runtime::state::RuntimeState;
    use agentic_runtime::transport::LocalTransport;
    use std::sync::Arc;

    /// Simulates an automation run that crashes mid-execution, recovers, then
    /// verifies that the coordinator's child_counter is high enough to avoid
    /// PK collisions with children created in the previous attempt.
    #[tokio::test]
    async fn test_recovery_child_counter_avoids_pk_collision() {
        let Some(db) = test_db().await else {
            return;
        };

        let root_id = test_run_id();
        let child_1 = format!("{root_id}.1");
        let child_2 = format!("{root_id}.2");
        let grandchild_a = format!("{root_id}.1.3"); // child_counter was 3

        // 1. Create initial run tree (attempt 0).
        crud::insert_run(
            &db,
            &root_id,
            "root Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &child_1,
            &root_id,
            "child 1 Q",
            "workflow",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &child_2,
            &root_id,
            "child 2 Q",
            "workflow",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &grandchild_a,
            &child_1,
            "grandchild Q",
            "workflow_step",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        // Mark root as waiting on children.
        crud::update_task_status(
            &db,
            &root_id,
            "delegating",
            Some(json!({
                "child_task_ids": [&child_1, &child_2],
                "completed": {},
                "failure_policy": "fail_fast",
            })),
        )
        .await
        .unwrap();

        // 2. Simulate crash + recovery: increment attempt.
        let new_attempt = crud::increment_attempt(&db, &root_id).await.unwrap();
        assert_eq!(new_attempt, 1);

        // 3. Reconstruct coordinator from DB.
        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let (coordinator, _pending_resumes) = Coordinator::from_db(
            db.clone(),
            state,
            transport as Arc<dyn CoordinatorTransport>,
            &root_id,
        )
        .await
        .unwrap();

        // 4. Verify that the coordinator's child counter is >= 3 (from grandchild).
        //    The next child ID should be a1_4 or higher — NOT a1_1, a1_2, or a1_3.
        //    We verify this by checking the internal state via a test method.
        //    Since child_counter is private, we verify indirectly: inserting a run
        //    with the ID that the coordinator would generate should NOT conflict.
        drop(coordinator);

        let max_counter = crud::get_max_child_counter(&db, &root_id).await.unwrap();
        assert!(
            max_counter >= 3,
            "max_counter should be >= 3 (from grandchild), got {max_counter}"
        );
    }

    /// Simulates multiple recovery attempts with children created at each attempt.
    /// Verifies that child_counter from get_max_child_counter accounts for ALL
    /// children across ALL attempts, even orphaned ones.
    #[tokio::test]
    async fn test_recovery_multiple_attempts_no_collision() {
        let Some(db) = test_db().await else {
            return;
        };

        let root_id = test_run_id();

        // 1. Create root run.
        crud::insert_run(
            &db,
            &root_id,
            "root Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(&db, &root_id, "running", None)
            .await
            .unwrap();

        // 2. Attempt 0 creates children with simple counter format.
        let child_0_1 = format!("{root_id}.1");
        let child_0_2 = format!("{root_id}.2");
        crud::insert_run_with_parent(
            &db,
            &child_0_1,
            &root_id,
            "Q",
            "workflow",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &child_0_2,
            &root_id,
            "Q",
            "workflow",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        // 3. Recovery attempt 1 creates children with a1_ prefix.
        crud::increment_attempt(&db, &root_id).await.unwrap();
        let child_1_3 = format!("{root_id}.a1_3");
        let child_1_4 = format!("{root_id}.a1_4");
        crud::insert_run_with_parent(
            &db,
            &child_1_3,
            &root_id,
            "Q",
            "workflow",
            None,
            1,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &child_1_4,
            &root_id,
            "Q",
            "workflow",
            None,
            1,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        // 4. Recovery attempt 2 — verify counter starts above 4.
        crud::increment_attempt(&db, &root_id).await.unwrap();
        let max_counter = crud::get_max_child_counter(&db, &root_id).await.unwrap();
        assert!(
            max_counter >= 4,
            "max_counter should be >= 4 (from a1_4), got {max_counter}"
        );

        // 5. Simulate coordinator creating a new child — should not collide.
        let child_2_5 = format!("{root_id}.a2_{}", max_counter + 1);
        crud::insert_run_with_parent(
            &db,
            &child_2_5,
            &root_id,
            "Q",
            "workflow",
            None,
            2,
            uuid::Uuid::nil(),
        )
        .await
        .expect("should not collide with existing children");
    }

    /// Verifies that from_db reconstructs a coordinator that can create new
    /// children without PK collisions, even after multiple recovery cycles
    /// with orphaned children from failed attempts.
    #[tokio::test]
    async fn test_from_db_with_orphaned_children_from_previous_attempt() {
        let Some(db) = test_db().await else {
            return;
        };

        let root_id = test_run_id();
        let automation_child = format!("{root_id}.1");

        // 1. Set up: root → automation child, root waiting on children.
        crud::insert_run(
            &db,
            &root_id,
            "analytics Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &automation_child,
            &root_id,
            "workflow Q",
            "workflow",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::update_task_status(
            &db,
            &root_id,
            "delegating",
            Some(json!({
                "child_task_ids": [&automation_child],
                "completed": {},
                "failure_policy": "fail_fast",
            })),
        )
        .await
        .unwrap();

        // Automation child delegates step tasks (children of the child).
        let step_1 = format!("{automation_child}.2");
        let step_2 = format!("{automation_child}.3");
        crud::insert_run_with_parent(
            &db,
            &step_1,
            &automation_child,
            "step 1",
            "workflow_step",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &step_2,
            &automation_child,
            "step 2",
            "workflow_step",
            None,
            0,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        // Mark step tasks as failed (simulating stale child cleanup).
        crud::update_run_failed(&db, &step_1, "server crashed")
            .await
            .unwrap();
        crud::update_task_status(&db, &step_1, "failed", None)
            .await
            .unwrap();
        crud::update_run_failed(&db, &step_2, "server crashed")
            .await
            .unwrap();
        crud::update_task_status(&db, &step_2, "failed", None)
            .await
            .unwrap();

        // 2. Recovery attempt 1: creates new step children.
        crud::increment_attempt(&db, &root_id).await.unwrap();
        let step_1_retry = format!("{automation_child}.a1_4");
        let step_2_retry = format!("{automation_child}.a1_5");
        crud::insert_run_with_parent(
            &db,
            &step_1_retry,
            &automation_child,
            "step 1 retry",
            "workflow_step",
            None,
            1,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();
        crud::insert_run_with_parent(
            &db,
            &step_2_retry,
            &automation_child,
            "step 2 retry",
            "workflow_step",
            None,
            1,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        crud::update_run_failed(&db, &step_1_retry, "server crashed again")
            .await
            .unwrap();
        crud::update_task_status(&db, &step_1_retry, "failed", None)
            .await
            .unwrap();

        // 3. Recovery attempt 2: from_db should find max_counter=5.
        let new_attempt = crud::increment_attempt(&db, &root_id).await.unwrap();
        assert_eq!(new_attempt, 2);

        let state = Arc::new(RuntimeState::new());
        let transport = LocalTransport::with_defaults();
        let (coordinator, _pending_resumes) = Coordinator::from_db(
            db.clone(),
            state,
            transport as Arc<dyn CoordinatorTransport>,
            &root_id,
        )
        .await
        .unwrap();

        // Verify coordinator was created successfully (from_db didn't fail).
        drop(coordinator);

        // Verify max child counter accounts for orphaned children.
        let max_counter = crud::get_max_child_counter(&db, &root_id).await.unwrap();
        assert!(
            max_counter >= 5,
            "max_counter should be >= 5 (from a1_5), got {max_counter}"
        );

        // Creating a new child at counter max+1 should succeed.
        let new_child = format!("{automation_child}.a2_{}", max_counter + 1);
        crud::insert_run_with_parent(
            &db,
            &new_child,
            &automation_child,
            "step 1 re-retry",
            "workflow_step",
            None,
            2,
            uuid::Uuid::nil(),
        )
        .await
        .expect("should not collide with any existing children");
    }

    /// Verifies that transparent recovery deletes partial events and writes
    /// a recovery_resumed marker without incrementing the attempt counter.
    #[tokio::test]
    async fn test_transparent_recovery_cleans_partial_events() {
        let Some(db) = test_db().await else {
            return;
        };

        let root_id = test_run_id();
        crud::insert_run(
            &db,
            &root_id,
            "Q",
            None,
            "analytics",
            None,
            uuid::Uuid::nil(),
        )
        .await
        .unwrap();

        // Simulate: step A completed, step B started but crashed before completing.
        crud::insert_event(&db, &root_id, 0, "step_start", &json!({"label": "A"}), 0)
            .await
            .unwrap();
        crud::insert_event(
            &db,
            &root_id,
            1,
            "step_end",
            &json!({"outcome": "advanced"}),
            0,
        )
        .await
        .unwrap();
        crud::insert_event(&db, &root_id, 2, "step_start", &json!({"label": "B"}), 0)
            .await
            .unwrap();
        // Crash here — step B's step_end never written.

        // Simulate transparent recovery: delete partial events, write marker.
        let all_events = crud::get_all_events(&db, &root_id).await.unwrap();
        if let Some(last_complete) = all_events.iter().rev().find(|e| e.event_type == "step_end") {
            crud::delete_events_from_seq(&db, &root_id, last_complete.seq + 1)
                .await
                .unwrap();
        }
        let next_seq = crud::get_max_seq(&db, &root_id).await.unwrap() + 1;
        crud::insert_event(&db, &root_id, next_seq, "recovery_resumed", &json!({}), 0)
            .await
            .unwrap();

        // Verify: partial step B event deleted, recovery marker added.
        let events = crud::get_all_events(&db, &root_id).await.unwrap();
        assert_eq!(
            events.len(),
            3,
            "should have 3 events: step A start+end + recovery_resumed"
        );
        assert_eq!(events[0].event_type, "step_start");
        assert_eq!(events[1].event_type, "step_end");
        assert_eq!(events[2].event_type, "recovery_resumed");

        // All events should have the same attempt (transparent — no increment).
        assert!(events.iter().all(|e| e.attempt == 0));

        // Verify attempt was NOT incremented on the run.
        let run = crud::get_run(&db, &root_id).await.unwrap().unwrap();
        assert_eq!(
            run.attempt, 0,
            "attempt should not increment on transparent recovery"
        );
    }
}

// ── Task Queue CRUD tests ──────────────────────────────────────────────────

/// Reset the global task-queue tables so each queue test starts from a clean
/// slate. The whole `agentic-runtime` integration binary shares ONE Postgres
/// (a reused testcontainer locally / a single CI service) and both `claim_task`
/// and outcome `recv` are GLOBAL — so any row a prior test left behind (e.g. a
/// requeued/reaped task left `queued`) would be poached by the next test's
/// "claim my task" / "queue is empty" assertions. Called from `test_db()` so
/// every test gets the clean slate. The `serial-db` nextest group runs these DB
/// tests one at a time, so this unconditional delete can't race a concurrent
/// test. (The previous `created_at < now() - 30s` guard was insufficient:
/// serial tests run far less than 30s apart, so recent rows survived and
/// polluted the next test.)
async fn cleanup_queued_entries(db: &DatabaseConnection) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    for sql in [
        "DELETE FROM agentic_task_queue",
        "DELETE FROM agentic_task_outcomes",
    ] {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            sql.to_string(),
        ))
        .await
        .expect("cleanup_queued_entries failed");
    }
}

/// Helper: create a parent run and enqueue a task for it. The clean slate is
/// established once per test in `test_db()`, so this just enqueues — letting a
/// single test enqueue multiple coexisting tasks without wiping the earlier ones.
async fn enqueue_test_task(db: &DatabaseConnection) -> (String, String) {
    let run_id = test_run_id();
    let task_id = run_id.clone(); // root tasks share run_id as task_id

    crud::insert_run(
        db,
        &run_id,
        "test question",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert_run failed");

    let spec = TaskSpec::Agent {
        agent_id: "test_agent".into(),
        question: "test question".into(),
        extra: None,
    };

    crud::enqueue_task(
        db,
        &task_id,
        &run_id,
        None,
        &spec,
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue_task failed");

    (task_id, run_id)
}

#[tokio::test]
async fn test_enqueue_and_claim_task() {
    let Some(db) = test_db().await else {
        return;
    };
    let (task_id, _run_id) = enqueue_test_task(&db).await;

    // Verify it was enqueued.
    let entry = crud::get_queue_entry(&db, &task_id)
        .await
        .expect("get_queue_entry failed")
        .expect("queue entry not found");
    assert_eq!(entry.queue_status, "queued");

    let claimed = crud::claim_task(&db, "worker-1")
        .await
        .expect("claim_task failed")
        .expect("no task to claim");
    assert_eq!(claimed.task_id, task_id);

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "claimed");
    assert_eq!(entry.worker_id.as_deref(), Some("worker-1"));
    assert_eq!(entry.claim_count, 1);

    let nothing = crud::claim_task(&db, "worker-2")
        .await
        .expect("claim_task failed");
    assert!(nothing.is_none());
}

#[tokio::test]
async fn test_complete_and_fail_task() {
    let Some(db) = test_db().await else {
        return;
    };

    let (task_id_1, _) = enqueue_test_task(&db).await;
    crud::claim_task(&db, "w1").await.unwrap().unwrap();
    assert_eq!(
        crud::complete_queue_task(&db, &task_id_1, "w1")
            .await
            .expect("complete_queue_task failed"),
        crud::TerminalWrite::Stamped,
        "the claim holder's completion must land"
    );
    let entry = crud::get_queue_entry(&db, &task_id_1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.queue_status, "completed");

    // Test fail.
    let (task_id_2, _) = enqueue_test_task(&db).await;
    crud::claim_task(&db, "w1").await.unwrap().unwrap();
    assert_eq!(
        crud::fail_queue_task(&db, &task_id_2, "w1")
            .await
            .expect("fail_queue_task failed"),
        crud::TerminalWrite::Stamped,
        "the claim holder's failure must land"
    );
    let entry = crud::get_queue_entry(&db, &task_id_2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.queue_status, "failed");
}

#[tokio::test]
async fn test_cancel_queued_task() {
    let Some(db) = test_db().await else {
        return;
    };
    let (task_id, _) = enqueue_test_task(&db).await;

    crud::cancel_queued_task(&db, &task_id)
        .await
        .expect("cancel_queued_task failed");

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "cancelled");

    // Cancelled tasks should not be claimable.
    let nothing = crud::claim_task(&db, "w1").await.unwrap();
    assert!(nothing.is_none());
}

#[tokio::test]
async fn test_heartbeat_and_reap() {
    let Some(db) = test_db().await else {
        return;
    };
    let (task_id, _) = enqueue_test_task(&db).await;

    crud::claim_task(&db, "w1").await.unwrap().unwrap();

    // Heartbeat should update last_heartbeat. Keyed on the claiming worker —
    // a heartbeat may only stamp a row its own worker still holds.
    let stamped = crud::update_queue_heartbeat(&db, &task_id, "w1")
        .await
        .expect("update_queue_heartbeat failed");
    assert!(stamped, "the claim holder's heartbeat must stamp the row");

    // A heartbeat from anyone else must not touch it.
    let stolen = crud::update_queue_heartbeat(&db, &task_id, "someone-else")
        .await
        .expect("update_queue_heartbeat failed");
    assert!(
        !stolen,
        "a heartbeat must never stamp a row held by another worker"
    );

    // Backdate last_heartbeat to simulate stale worker.
    use agentic_runtime::entity::task_queue;
    use sea_orm::{EntityTrait, Set};
    let mut active: task_queue::ActiveModel = crud::get_queue_entry(&db, &task_id)
        .await
        .unwrap()
        .unwrap()
        .into();
    active.last_heartbeat = Set(Some(
        chrono::Utc::now().fixed_offset() - chrono::Duration::seconds(120),
    ));
    task_queue::Entity::update(active).exec(&db).await.unwrap();

    // Reap stale tasks → should re-queue.
    let reaped = crud::reap_stale_tasks(&db).await.expect("reap failed");
    assert_eq!(reaped.requeued, 1);
    assert_eq!(reaped.dead_lettered, 0);

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "queued");
    assert!(entry.worker_id.is_none());

    let claimed = crud::claim_task(&db, "w2").await.unwrap().unwrap();
    assert_eq!(claimed.claim_count, 2);
}

#[tokio::test]
async fn test_dead_letter_after_max_claims() {
    let Some(db) = test_db().await else {
        return;
    };
    let (task_id, _) = enqueue_test_task(&db).await;

    // Claim and reap repeatedly until max_claims (default 3).
    for i in 0..3 {
        crud::claim_task(&db, &format!("w{i}"))
            .await
            .unwrap()
            .unwrap();

        // Backdate heartbeat to simulate stale.
        use agentic_runtime::entity::task_queue;
        use sea_orm::{EntityTrait, Set};
        let mut active: task_queue::ActiveModel = crud::get_queue_entry(&db, &task_id)
            .await
            .unwrap()
            .unwrap()
            .into();
        active.last_heartbeat = Set(Some(
            chrono::Utc::now().fixed_offset() - chrono::Duration::seconds(120),
        ));
        task_queue::Entity::update(active).exec(&db).await.unwrap();

        crud::reap_stale_tasks(&db).await.unwrap();
    }

    // After 3 claims + reaps, should be dead.
    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "dead");

    // Dead tasks should not be claimable.
    let nothing = crud::claim_task(&db, "w-final").await.unwrap();
    assert!(nothing.is_none());
}

// ── DurableTransport tests ─────────────────────────────────────────────────

/// Helper: create a DurableTransport and a parent run for testing.
/// Cleans up stale queued entries first to avoid cross-test interference.
async fn setup_durable_transport(
    db: &DatabaseConnection,
) -> (std::sync::Arc<DurableTransport>, String) {
    cleanup_queued_entries(db).await;
    let run_id = test_run_id();
    crud::insert_run(
        db,
        &run_id,
        "durable test",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();
    let transport = DurableTransport::new(db.clone());
    (transport, run_id)
}

fn test_assignment(task_id: &str, run_id: &str) -> TaskAssignment {
    TaskAssignment {
        task_id: task_id.to_string(),
        parent_task_id: None,
        run_id: run_id.to_string(),
        spec: TaskSpec::Agent {
            agent_id: "test_agent".into(),
            question: "test question".into(),
            extra: None,
        },
        policy: None,
    }
}

#[tokio::test]
async fn test_durable_transport_assign_and_recv() {
    let Some(db) = test_db().await else {
        return;
    };
    let (transport, run_id) = setup_durable_transport(&db).await;
    let task_id = run_id.clone();

    let assignment = test_assignment(&task_id, &run_id);
    CoordinatorTransport::assign(transport.as_ref(), assignment)
        .await
        .unwrap();

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "queued");

    let received = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        WorkerTransport::recv_assignment(transport.as_ref()),
    )
    .await
    .expect("recv_assignment timed out")
    .expect("no assignment");

    assert_eq!(received.task_id, task_id);
    assert_eq!(received.run_id, run_id);

    // After claim, queue status should be "claimed".
    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "claimed");
}

#[tokio::test]
async fn test_durable_transport_cancel() {
    let Some(db) = test_db().await else {
        return;
    };
    let (transport, run_id) = setup_durable_transport(&db).await;
    let task_id = run_id.clone();

    // Assign then cancel.
    let assignment = test_assignment(&task_id, &run_id);
    CoordinatorTransport::assign(transport.as_ref(), assignment)
        .await
        .unwrap();
    CoordinatorTransport::cancel(transport.as_ref(), &task_id)
        .await
        .unwrap();

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "cancelled");
}

#[tokio::test]
async fn test_durable_transport_worker_outcome_updates_queue() {
    let Some(db) = test_db().await else {
        return;
    };
    let (transport, run_id) = setup_durable_transport(&db).await;
    let task_id = run_id.clone();

    let assignment = test_assignment(&task_id, &run_id);
    CoordinatorTransport::assign(transport.as_ref(), assignment)
        .await
        .unwrap();
    let _received = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        WorkerTransport::recv_assignment(transport.as_ref()),
    )
    .await
    .unwrap()
    .unwrap();

    WorkerTransport::send(
        transport.as_ref(),
        WorkerMessage::Outcome {
            task_id: task_id.clone(),
            outcome: TaskOutcome::Done {
                answer: "42".into(),
                metadata: None,
            },
        },
    )
    .await
    .unwrap();

    // Coordinator receives the outcome.
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        CoordinatorTransport::recv(transport.as_ref()),
    )
    .await
    .expect("recv timed out")
    .expect("no message");
    assert!(matches!(msg, WorkerMessage::Outcome { .. }));

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "completed");
}

#[tokio::test]
async fn test_durable_transport_events_pass_through() {
    let Some(db) = test_db().await else {
        return;
    };
    let (transport, run_id) = setup_durable_transport(&db).await;
    let task_id = run_id.clone();

    let assignment = test_assignment(&task_id, &run_id);
    CoordinatorTransport::assign(transport.as_ref(), assignment)
        .await
        .unwrap();
    let _received = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        WorkerTransport::recv_assignment(transport.as_ref()),
    )
    .await
    .unwrap()
    .unwrap();

    // Worker sends an event (not an outcome).
    WorkerTransport::send(
        transport.as_ref(),
        WorkerMessage::Event {
            task_id: task_id.clone(),
            event_type: "step_start".into(),
            payload: json!({"state": "clarifying"}),
        },
    )
    .await
    .unwrap();

    // Coordinator receives the event. Queue status should still be claimed.
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        CoordinatorTransport::recv(transport.as_ref()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(msg, WorkerMessage::Event { .. }));

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "claimed");
}

// ── Heartbeat + Reaper tests ───────────────────────────────────────────────

#[tokio::test]
async fn test_durable_transport_heartbeat() {
    let Some(db) = test_db().await else {
        return;
    };
    let (transport, run_id) = setup_durable_transport(&db).await;
    let task_id = run_id.clone();

    let assignment = test_assignment(&task_id, &run_id);
    CoordinatorTransport::assign(transport.as_ref(), assignment)
        .await
        .unwrap();
    let _received = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        WorkerTransport::recv_assignment(transport.as_ref()),
    )
    .await
    .unwrap()
    .unwrap();

    let entry_before = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    let hb_before = entry_before.last_heartbeat.unwrap();

    // Wait a moment then heartbeat via the WorkerTransport trait method.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    WorkerTransport::heartbeat(transport.as_ref(), &task_id)
        .await
        .unwrap();

    let entry_after = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    let hb_after = entry_after.last_heartbeat.unwrap();
    assert!(hb_after > hb_before);
}

#[tokio::test]
async fn test_durable_transport_reaper_requeues_stale() {
    let Some(db) = test_db().await else {
        return;
    };
    let (transport, run_id) = setup_durable_transport(&db).await;
    let task_id = run_id.clone();

    let assignment = test_assignment(&task_id, &run_id);
    CoordinatorTransport::assign(transport.as_ref(), assignment)
        .await
        .unwrap();
    let _received = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        WorkerTransport::recv_assignment(transport.as_ref()),
    )
    .await
    .unwrap()
    .unwrap();

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "claimed");

    // Backdate the heartbeat to simulate a stale worker.
    use agentic_runtime::entity::task_queue;
    use sea_orm::{EntityTrait, Set};
    let mut active: task_queue::ActiveModel = entry.into();
    active.last_heartbeat = Set(Some(
        chrono::Utc::now().fixed_offset() - chrono::Duration::seconds(120),
    ));
    task_queue::Entity::update(active).exec(&db).await.unwrap();

    let reaped = transport.run_reaper().await;
    assert_eq!(reaped.requeued, 1);
    assert_eq!(reaped.dead_lettered, 0);

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "queued");
}

// ── Recovery re-enqueue tests ──────────────────────────────────────────────

#[tokio::test]
async fn test_requeue_task_upserts_existing_entry() {
    let Some(db) = test_db().await else {
        return;
    };
    let (task_id, _run_id) = enqueue_test_task(&db).await;

    // Claim the task (simulates worker picking it up).
    let claimed = crud::claim_task(&db, "worker-1").await.unwrap().unwrap();
    assert_eq!(claimed.task_id, task_id);

    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "claimed");
    assert_eq!(entry.claim_count, 1);

    // Re-enqueue using requeue_task (upsert). This should reset the row
    // instead of failing with a PK violation.
    let new_spec = TaskSpec::Agent {
        agent_id: "__builder__".into(),
        question: "rebuild semantic model".into(),
        extra: None,
    };
    crud::requeue_task(&db, &task_id, &new_spec)
        .await
        .expect("requeue_task should not fail with PK violation");

    // Verify the entry is back to queued with reset counters.
    let entry = crud::get_queue_entry(&db, &task_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "queued");
    assert_eq!(entry.claim_count, 0);
    assert!(entry.worker_id.is_none());
    assert!(entry.last_heartbeat.is_none());

    // Verify the spec was updated.
    let spec: TaskSpec = serde_json::from_value(entry.spec).unwrap();
    match spec {
        TaskSpec::Agent { agent_id, .. } => assert_eq!(agent_id, "__builder__"),
        _ => panic!("expected Agent spec"),
    }

    // The re-queued task should be claimable.
    let reclaimed = crud::claim_task(&db, "worker-2").await.unwrap().unwrap();
    assert_eq!(reclaimed.task_id, task_id);
}

/// Driver lease: CAS acquire/renew, contention rejection, stale-takeover,
/// the recovery-selection gate, scoped release, and terminal auto-clear.
#[tokio::test]
async fn test_driver_lease() {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();
    crud::insert_run(
        &db,
        &run_id,
        "Q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert_run");

    // Acquire, then idempotent re-acquire by the same driver.
    assert!(
        crud::try_acquire_driver(&db, &run_id, "drv-A")
            .await
            .unwrap()
    );
    assert!(
        crud::try_acquire_driver(&db, &run_id, "drv-A")
            .await
            .unwrap()
    );
    // A different driver is rejected while A's lease is fresh.
    assert!(
        !crud::try_acquire_driver(&db, &run_id, "drv-B")
            .await
            .unwrap()
    );

    // Heartbeat only succeeds for the lease holder.
    assert!(!crud::heartbeat_driver(&db, &run_id, "drv-B").await.unwrap());
    assert!(crud::heartbeat_driver(&db, &run_id, "drv-A").await.unwrap());

    // A fresh lease excludes the run from recovery selection (the F1 gate).
    let resumable = crud::get_resumable_root_runs(&db, None).await.unwrap();
    assert!(
        !resumable.iter().any(|r| r.id == run_id),
        "run with a live lease must not be resumable"
    );

    // Age A's heartbeat past the TTL → B can steal it, and the run becomes
    // resumable again.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET driver_heartbeat_at = now() - make_interval(secs => $1) \
         WHERE id = $2",
        [
            (agentic_runtime::crud::DRIVER_LEASE_TTL_SECS as i32 + 60).into(),
            run_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    let resumable = crud::get_resumable_root_runs(&db, None).await.unwrap();
    assert!(
        resumable.iter().any(|r| r.id == run_id),
        "run with a stale lease must be resumable"
    );
    assert!(
        crud::try_acquire_driver(&db, &run_id, "drv-B")
            .await
            .unwrap()
    );

    // A non-owner release is a no-op; the owner can release.
    crud::release_driver(&db, &run_id, "drv-A").await.unwrap();
    assert!(
        !crud::try_acquire_driver(&db, &run_id, "drv-C")
            .await
            .unwrap(),
        "drv-B still owns it, drv-A's release must not have cleared it"
    );
    crud::release_driver(&db, &run_id, "drv-B").await.unwrap();
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert!(run.driver_id.is_none(), "owner release clears the lease");

    // Terminal transition auto-clears any held lease.
    assert!(
        crud::try_acquire_driver(&db, &run_id, "drv-D")
            .await
            .unwrap()
    );
    crud::update_run_done(&db, &run_id, "ok", None)
        .await
        .expect("update_run_done");
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert!(
        run.driver_id.is_none() && run.driver_heartbeat_at.is_none(),
        "terminal transition clears the driver lease"
    );
}

/// Rung-2 acceptance proof (consolidated, claim + lease layers).
///
/// The whole Phase-1 safety story in one deterministic test:
///   1. The unscoped/global claim path NEVER returns a `scope_owned`
///      (interactive) task — even when it is the only queued row.
///   2. The scoped path STILL claims that interactive task, so the
///      per-request coordinator never stalls.
///   3. The driver-lease CAS rejects a second concurrent driver for the
///      same run, so two loops/replicas can't double-drive.
#[tokio::test]
async fn test_rung2_global_never_poaches_scoped() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_scoped = test_run_id();
    let run_global = test_run_id();

    // Queue rows FK to agentic_runs — seed the run rows first.
    crud::insert_run(
        &db,
        &run_scoped,
        "Q",
        None,
        "workflow",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert run_scoped");
    crud::insert_run(
        &db,
        &run_global,
        "Q",
        None,
        "workflow",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert run_global");

    let spec = |rid: &str| TaskSpec::AutomationDecision {
        run_id: rid.to_string(),
        pending_child_answer: None,
    };

    // An interactive run's root task (a per-request coordinator owns it)…
    crud::enqueue_task(
        &db,
        &run_scoped,
        &run_scoped,
        None,
        &spec(&run_scoped),
        None,
        crud::TaskScope::Scoped,
    )
    .await
    .expect("enqueue scoped");
    // …and a global (scheduler-seeded / orphaned) root task.
    crud::enqueue_task(
        &db,
        &run_global,
        &run_global,
        None,
        &spec(&run_global),
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue global");

    // (1) The global worker claims ONLY the Global task.
    let claimed = crud::claim_task(&db, "global-worker-1")
        .await
        .expect("claim_task")
        .expect("a global task must be claimable");
    assert_eq!(
        claimed.task_id, run_global,
        "global claim path must return the Global task, never the Scoped one"
    );
    assert!(!claimed.scope_owned);

    // The Scoped task is now the only queued row. The global path must
    // STILL refuse it — this is the anti-poaching invariant.
    let none = crud::claim_task(&db, "global-worker-2")
        .await
        .expect("claim_task");
    assert!(
        none.is_none(),
        "global claim path must never claim a scope_owned task, even as \
         the sole queued row (got {none:?})"
    );

    // (2) The scoped path still claims the interactive task → the
    // per-request coordinator is not starved.
    let scoped = crud::claim_task_under_root(&db, "scoped-coordinator", &run_scoped)
        .await
        .expect("claim_task_under_root")
        .expect("scoped worker must still claim its own task");
    assert_eq!(scoped.task_id, run_scoped);

    // (3) Driver-lease CAS rejects a concurrent second driver.
    let leased_run = test_run_id();
    crud::insert_run(
        &db,
        &leased_run,
        "Q",
        None,
        "workflow",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert_run");
    assert!(
        crud::try_acquire_driver(&db, &leased_run, "driver-A")
            .await
            .unwrap(),
        "first driver acquires the lease"
    );
    assert!(
        !crud::try_acquire_driver(&db, &leased_run, "driver-B")
            .await
            .unwrap(),
        "a concurrent second driver must be rejected (no double-drive)"
    );
}

/// Rung-4 acceptance proof: kill a Global run's driver mid-flight, the
/// reaper re-queues its task, and the periodic loop becomes able to drive
/// it exactly once — with no partial-state corruption.
///
/// Deterministic at the data layer (no real coordinators / timing): the
/// rung-4 invariant is the composition of reaper + stranded-selection +
/// lease-CAS, each provable directly.
#[tokio::test]
async fn test_rung4_kill_and_recover() {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();
    crud::insert_run(&db, &run_id, "Q", None, "workflow", None, uuid::Uuid::nil())
        .await
        .expect("insert_run");
    // A scheduler-seeded / orphan Global root task.
    crud::enqueue_task(
        &db,
        &run_id,
        &run_id,
        None,
        &TaskSpec::AutomationDecision {
            run_id: run_id.clone(),
            pending_child_answer: None,
        },
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue");

    // A driver claims it and takes the lease — then is hard-killed
    // (no further heartbeats).
    let claimed = crud::claim_task(&db, "dead-worker")
        .await
        .unwrap()
        .expect("global worker claims the Global task");
    assert_eq!(claimed.task_id, run_id);
    assert!(
        crud::try_acquire_driver(&db, &run_id, "dead-driver")
            .await
            .unwrap()
    );

    // Age everything past the relevant windows: queue heartbeat past the
    // visibility timeout, driver lease past its TTL, run past the grace.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET last_heartbeat = now() - make_interval(secs => 3600) \
         WHERE task_id = $1",
        [run_id.clone().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs \
         SET driver_heartbeat_at = now() - make_interval(secs => 3600), \
             updated_at = now() - make_interval(secs => 3600) \
         WHERE id = $1",
        [run_id.clone().into()],
    ))
    .await
    .unwrap();

    // Reaper re-queues the dead worker's task.
    crud::reap_stale_tasks(&db).await.expect("reap");
    let entry = crud::get_queue_entry(&db, &run_id)
        .await
        .unwrap()
        .expect("queue row still present");
    assert_eq!(entry.queue_status, "queued", "reaper must re-queue");
    assert!(entry.worker_id.is_none(), "reaper clears the dead worker");
    assert!(
        !entry.scope_owned,
        "scope_owned must be preserved across reaping"
    );

    // Rung-4 invariant: a reaper-requeued Global task (queued,
    // scope_owned=false) IS now stranded — the periodic loop will pick it
    // up. (Pre-fix this was excluded forever → nothing drove it.)
    let stuck = crud::find_stuck_runs(&db, 30, None).await.unwrap();
    assert!(
        stuck.iter().any(|r| r.run_id == run_id),
        "a reaper-requeued Global run must be selectable by the periodic loop"
    );

    // Exactly-once: the dead driver's lease is stale, so a fresh driver
    // takes it — and a second concurrent driver is then rejected.
    assert!(
        crud::try_acquire_driver(&db, &run_id, "recovery-driver")
            .await
            .unwrap(),
        "fresh driver takes over the stale lease"
    );
    assert!(
        !crud::try_acquire_driver(&db, &run_id, "other-driver")
            .await
            .unwrap(),
        "no second driver while the recovery driver holds a fresh lease"
    );

    // No partial-state corruption: run still non-terminal, and the
    // recovered driver's scoped reconstruction reclaims the same task with
    // its original spec intact.
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.task_status.as_deref(), Some("running"));
    let reclaimed = crud::claim_task_under_root(&db, "recovery-coordinator", &run_id)
        .await
        .unwrap()
        .expect("scoped reconstruction reclaims the requeued task");
    assert_eq!(reclaimed.task_id, run_id);
    let spec: TaskSpec = serde_json::from_value(reclaimed.spec).unwrap();
    assert!(matches!(spec, TaskSpec::AutomationDecision { .. }));
}

/// §12 FU4a: the durable cross-process cancel flag — `request_cancel` is
/// idempotent (preserves the first timestamp) and `is_cancel_requested`
/// reflects it. This is the DB-observable signal the recovered/Global
/// run's cancel forwarder polls.
#[tokio::test]
async fn test_request_cancel_flag() {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

    let Some(db) = test_db().await else {
        return;
    };
    let run_id = test_run_id();
    crud::insert_run(&db, &run_id, "Q", None, "workflow", None, uuid::Uuid::nil())
        .await
        .expect("insert_run");

    assert!(
        !crud::is_cancel_requested(&db, &run_id).await.unwrap(),
        "fresh run has no cancel request"
    );

    crud::request_cancel(&db, &run_id).await.unwrap();
    assert!(crud::is_cancel_requested(&db, &run_id).await.unwrap());

    #[derive(FromQueryResult)]
    struct Ts {
        cancel_requested_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    }
    let first = Ts::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT cancel_requested_at FROM agentic_runs WHERE id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .unwrap()
    .cancel_requested_at
    .expect("set");

    // Idempotent: a second request must not move the timestamp.
    crud::request_cancel(&db, &run_id).await.unwrap();
    let second = Ts::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT cancel_requested_at FROM agentic_runs WHERE id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .unwrap()
    .cancel_requested_at
    .expect("still set");
    assert_eq!(first, second, "request_cancel must be idempotent");

    // Unknown run → false, not an error.
    assert!(!crud::is_cancel_requested(&db, "no-such-run").await.unwrap());

    // Cleared on terminal transition (terminal nulls the lease; the
    // cancel flag is separate and intentionally NOT auto-cleared, so a
    // late observer still sees it — assert it persists through terminal).
    crud::update_run_done(&db, &run_id, "ok", None)
        .await
        .unwrap();
    assert!(
        crud::is_cancel_requested(&db, &run_id).await.unwrap(),
        "cancel flag persists across terminal (audit trail)"
    );
}
