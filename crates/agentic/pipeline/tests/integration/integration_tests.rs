//! Integration tests for agentic-pipeline against a real PostgreSQL database.
//!
//! Tests the pipeline-level facades: insert_run, update_run_done,
//! get_thread_history, analytics extensions, event registry construction.
//!
//! Uses testcontainers to automatically spin up Postgres. To use an external DB:
//!   OXY_DATABASE_URL=postgresql://postgres:postgres@localhost:15432/oxy \
//!     cargo nextest run -p agentic-pipeline --test integration -E 'test(integration_tests)'

use agentic_pipeline::{
    build_event_registry, get_analytics_extension, get_analytics_extensions, insert_run,
    update_run_done, update_run_thinking_mode,
};
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{Database, DatabaseConnection};
use serde_json::json;

async fn test_db() -> Option<DatabaseConnection> {
    let url = oxy_test_utils::containers::shared_db_url::<RuntimeMigrator>().await;

    // Retry connection — the reusable container may still be starting up.
    let mut db = None;
    for attempt in 0..10 {
        match Database::connect(&url).await {
            Ok(conn) => {
                db = Some(conn);
                break;
            }
            Err(_e) if attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(e) => panic!("failed to connect to test database after 10 retries: {e}"),
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

fn test_run_id() -> String {
    format!("test-pipe-{}", uuid::Uuid::new_v4())
}

// ── insert_run tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_insert_run_analytics_creates_extension() {
    let Some(db) = test_db().await else { return };
    let run_id = test_run_id();

    insert_run(
        &db,
        &run_id,
        "test_agent",
        "What is revenue?",
        None,
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert_run failed");

    // Verify run record exists.
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        crud::user_facing_status(run.task_status.as_deref()),
        "running"
    );
    assert_eq!(run.source_type.as_deref(), Some("analytics"));

    let ext = get_analytics_extension(&db, &run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ext.agent_id, "test_agent");
    assert!(ext.thinking_mode.is_none());
    assert!(ext.spec_hint.is_none());
}

#[tokio::test]
async fn test_insert_run_builder_no_extension() {
    let Some(db) = test_db().await else { return };
    let run_id = test_run_id();

    insert_run(
        &db,
        &run_id,
        "__builder__",
        "Build a dashboard",
        None,
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert_run failed");

    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.source_type.as_deref(), Some("builder"));

    // Builder runs should NOT have an analytics extension.
    let ext = get_analytics_extension(&db, &run_id).await.unwrap();
    assert!(ext.is_none());
}

#[tokio::test]
async fn test_insert_run_with_thinking_mode() {
    let Some(db) = test_db().await else { return };
    let run_id = test_run_id();

    insert_run(
        &db,
        &run_id,
        "agent",
        "Q",
        None,
        Some("extended_thinking".to_string()),
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    let ext = get_analytics_extension(&db, &run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ext.thinking_mode.as_deref(), Some("extended_thinking"));
}

// ── update_run_done tests ───────────────────────────────────────────────────

#[tokio::test]
async fn test_update_run_done_with_spec_hint() {
    let Some(db) = test_db().await else { return };
    let run_id = test_run_id();

    insert_run(&db, &run_id, "agent", "Q", None, None, uuid::Uuid::nil())
        .await
        .unwrap();

    let spec_hint = json!({"measures": ["revenue"], "dimensions": ["region"]});
    update_run_done(&db, &run_id, "The answer is 42", Some(spec_hint.clone()))
        .await
        .unwrap();

    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(crud::user_facing_status(run.task_status.as_deref()), "done");
    assert_eq!(run.answer.as_deref(), Some("The answer is 42"));

    // Verify spec_hint on extension.
    let ext = get_analytics_extension(&db, &run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ext.spec_hint.unwrap(), spec_hint);
}

#[tokio::test]
async fn test_update_run_done_without_spec_hint() {
    let Some(db) = test_db().await else { return };
    let run_id = test_run_id();

    insert_run(&db, &run_id, "agent", "Q", None, None, uuid::Uuid::nil())
        .await
        .unwrap();
    update_run_done(&db, &run_id, "Done", None).await.unwrap();

    let ext = get_analytics_extension(&db, &run_id)
        .await
        .unwrap()
        .unwrap();
    assert!(ext.spec_hint.is_none());
}

// ── update_run_thinking_mode test ───────────────────────────────────────────

#[tokio::test]
async fn test_update_thinking_mode() {
    let Some(db) = test_db().await else { return };
    let run_id = test_run_id();

    insert_run(&db, &run_id, "agent", "Q", None, None, uuid::Uuid::nil())
        .await
        .unwrap();

    update_run_thinking_mode(&db, &run_id, Some("extended_thinking".to_string()))
        .await
        .unwrap();

    let ext = get_analytics_extension(&db, &run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ext.thinking_mode.as_deref(), Some("extended_thinking"));
}

// ── get_analytics_extensions bulk test ───────────────────────────────────────

#[tokio::test]
async fn test_get_analytics_extensions_bulk() {
    let Some(db) = test_db().await else { return };
    let id1 = test_run_id();
    let id2 = test_run_id();
    let id3 = test_run_id();

    insert_run(&db, &id1, "agent_a", "Q1", None, None, uuid::Uuid::nil())
        .await
        .unwrap();
    insert_run(&db, &id2, "agent_b", "Q2", None, None, uuid::Uuid::nil())
        .await
        .unwrap();
    insert_run(
        &db,
        &id3,
        "__builder__",
        "Q3",
        None,
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    let exts = get_analytics_extensions(&db, &[id1.clone(), id2.clone(), id3.clone()])
        .await
        .unwrap();

    // Only analytics runs have extensions.
    assert_eq!(exts.len(), 2);
    let agent_ids: Vec<&str> = exts.iter().map(|e| e.agent_id.as_str()).collect();
    assert!(agent_ids.contains(&"agent_a"));
    assert!(agent_ids.contains(&"agent_b"));
}

// ── build_event_registry test ───────────────────────────────────────────────

#[tokio::test]
async fn test_build_event_registry_processes_both_domains() {
    let Some(_db) = test_db().await else { return };
    let registry = build_event_registry();

    // Test analytics core events.
    let mut proc = registry.stream_processor("analytics");
    let results = proc.process(
        "state_enter",
        &json!({"state": "clarifying", "revision": 0, "trace_id": "t"}),
    );
    assert!(!results.is_empty());
    assert_eq!(results[0].0, "step_start");

    // Test builder domain events.
    let mut proc = registry.stream_processor("builder");
    let results = proc.process(
        "tool_used",
        &json!({"tool_name": "read_file", "summary": "Read config.yml"}),
    );
    assert!(!results.is_empty());
    assert_eq!(results[0].0, "tool_used");
}

// Note: get_thread_history is not tested here because it requires a valid
// thread_id FK referencing the central `threads` table. It's exercised
// by the runtime integration tests and the HTTP handler tests instead.
