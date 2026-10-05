//! A run started in a workspace preview is never driven outside one: every
//! recovery entry point retires it (`preview run interrupted; …`) instead of
//! driving it on the production platform, and a cold resume on a production
//! platform does the same. An unstamped run beside it is recovered as before.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(preview_run_recovery_test)'

use std::sync::Arc;
use std::time::Duration;

use agentic_pipeline::PipelineBuilder;
use agentic_pipeline::automation_run::start_automation_run;
use agentic_pipeline::platform::preview_stamp::{INTERRUPTED, RUN_STAMP};
use agentic_pipeline::platform::{PlatformContext, RunPlatformResolver};
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::automation_recovery_test::test_db;
use crate::run_platform_resolver_test::{Marked, request};

/// Add the preview stamp to a run's metadata, as `start_analytics` writes it
/// for a run started from a preview request.
async fn stamp(db: &DatabaseConnection, run_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET metadata = COALESCE(metadata, '{}'::jsonb) \
         || jsonb_build_object($2::text, jsonb_build_object('revision_id', $3::text)) \
         WHERE id = $1",
        [
            run_id.into(),
            RUN_STAMP.into(),
            Uuid::new_v4().to_string().into(),
        ],
    ))
    .await
    .unwrap();
}

struct Seeded {
    ws: Uuid,
    stamped: String,
    plain: String,
}

/// Two Global runs in a fresh workspace, one stamped.
async fn seed(db: &DatabaseConnection, tag: &str) -> Seeded {
    let ws = Uuid::new_v4();
    let run = |r: String| async move {
        start_automation_run(db, request(&r), crud::TaskScope::Global, ws)
            .await
            .expect("seed run")
    };
    let stamped = run(format!("{tag}_stamped.procedure.yml")).await;
    let plain = run(format!("{tag}_plain.procedure.yml")).await;
    stamp(db, &stamped).await;
    Seeded { ws, stamped, plain }
}

/// The stamped run is failed with the preview's reason and was never driven;
/// the plain one was driven on the platform recovery handed it.
async fn assert_retired_not_driven(db: &DatabaseConnection, tag: &str, s: &Seeded, m: &Marked) {
    let plain = format!("{tag}_plain.procedure.yml");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !m.resolved.lock().unwrap().contains(&plain) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{tag}: the unstamped run was not recovered"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let stamped_ref = format!("{tag}_stamped.procedure.yml");
    assert!(
        !m.resolved.lock().unwrap().contains(&stamped_ref),
        "{tag}: the preview run was driven"
    );
    let row = crud::get_run(db, &s.stamped).await.unwrap().expect("run");
    assert_eq!(row.task_status.as_deref(), Some("failed"), "{tag}: {row:?}");
    let error = row.error_message.unwrap_or_default();
    assert!(error.contains(INTERRUPTED), "{tag}: {error}");
    let plain_row = crud::get_run(db, &s.plain).await.unwrap().expect("run");
    assert!(
        !plain_row
            .error_message
            .unwrap_or_default()
            .contains(INTERRUPTED),
        "{tag}: the unstamped run was retired as a preview run"
    );
}

fn resolver(m: &Arc<Marked>) -> Arc<dyn RunPlatformResolver> {
    struct Hand(Arc<Marked>);
    #[async_trait::async_trait]
    impl RunPlatformResolver for Hand {
        async fn platform_for(
            &self,
            _root: &agentic_runtime::entity::run::Model,
            _base: Arc<dyn PlatformContext>,
        ) -> Result<Arc<dyn PlatformContext>, String> {
            Ok(self.0.clone())
        }
    }
    Arc::new(Hand(m.clone()))
}

fn router() -> Arc<dyn agentic_runtime::router::TaskRouter> {
    Arc::new(agentic_runtime::router::NoopTaskRouter)
}

#[tokio::test(flavor = "multi_thread")]
async fn every_recovery_entry_point_retires_a_preview_run() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let base: Arc<dyn PlatformContext> = Arc::new(crate::automation_recovery_test::FakePlatform);
    let state = || Arc::new(RuntimeState::new());

    // The latency worker's path.
    let (s, m) = (seed(&db, "pending").await, Arc::new(Marked::default()));
    agentic_pipeline::recovery::recover_pending_global_runs(
        db.clone(),
        state(),
        base.clone(),
        resolver(&m),
        None,
        None,
        None,
        None,
        router(),
        Some(s.ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;
    assert_retired_not_driven(&db, "pending", &s, &m).await;

    // The periodic tick's path.
    let (s, m) = (seed(&db, "stranded").await, Arc::new(Marked::default()));
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET updated_at = now() - interval '10 minutes' WHERE workspace_id = $1",
        [s.ws.into()],
    ))
    .await
    .unwrap();
    agentic_pipeline::recovery::recover_stranded_runs(
        db.clone(),
        state(),
        base.clone(),
        resolver(&m),
        None,
        None,
        None,
        None,
        router(),
        Some(s.ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;
    assert_retired_not_driven(&db, "stranded", &s, &m).await;

    // The one-shot startup pass.
    let (s, m) = (seed(&db, "startup").await, Arc::new(Marked::default()));
    agentic_pipeline::recovery::recover_active_runs(
        db.clone(),
        state(),
        base.clone(),
        resolver(&m),
        None,
        None,
        None,
        None,
        router(),
        Some(s.ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;
    assert_retired_not_driven(&db, "startup", &s, &m).await;
}

fn suspended() -> agentic_core::human_input::SuspendedRunData {
    agentic_core::human_input::SuspendedRunData {
        from_state: "clarifying".into(),
        original_input: "q".into(),
        trace_id: "t".into(),
        stage_data: serde_json::json!({}),
        question: "which store?".into(),
        suggestions: vec![],
    }
}

/// A cold resume on a production platform (an answer sent without the preview
/// header) retires a stamped run instead of resuming it there.
#[tokio::test(flavor = "multi_thread")]
async fn a_cold_resume_outside_the_preview_retires_a_preview_run() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let run_id = Uuid::new_v4().to_string();
    crud::insert_run(
        &db,
        &run_id,
        "q",
        None,
        "analytics",
        Some(serde_json::json!({ "agent_id": "chat" })),
        ws,
    )
    .await
    .unwrap();
    stamp(&db, &run_id).await;
    let production: Arc<dyn PlatformContext> =
        Arc::new(crate::automation_recovery_test::FakePlatform);

    let result = PipelineBuilder::new(production)
        .resume(
            &db,
            &run_id,
            "analytics",
            "chat",
            None,
            suspended(),
            "a".into(),
        )
        .await;
    let err = match result {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a preview run must not resume on production"),
    };
    assert!(err.contains(INTERRUPTED), "{err}");
    let row = crud::get_run(&db, &run_id).await.unwrap().expect("run");
    assert_eq!(row.task_status.as_deref(), Some("failed"), "{row:?}");
    assert!(
        row.error_message.unwrap_or_default().contains(INTERRUPTED),
        "retired with the preview's reason"
    );

    // The control: an unstamped run is not retired as a preview run.
    let plain = Uuid::new_v4().to_string();
    crud::insert_run(
        &db,
        &plain,
        "q",
        None,
        "analytics",
        Some(serde_json::json!({ "agent_id": "chat" })),
        ws,
    )
    .await
    .unwrap();
    let production: Arc<dyn PlatformContext> =
        Arc::new(crate::automation_recovery_test::FakePlatform);
    let result = PipelineBuilder::new(production)
        .resume(
            &db,
            &plain,
            "analytics",
            "chat",
            None,
            suspended(),
            "a".into(),
        )
        .await;
    if let Err(e) = result {
        assert!(!e.to_string().contains(INTERRUPTED), "{e}");
    }
    let row = crud::get_run(&db, &plain).await.unwrap().expect("run");
    assert!(
        !row.error_message.unwrap_or_default().contains(INTERRUPTED),
        "an unstamped run is resumed as before"
    );
}
