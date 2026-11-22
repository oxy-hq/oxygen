//! A seeded run commits its `agentic_runs` row and its queue row together, or
//! neither.
//!
//! The scheduler's seeds used to insert the run, commit it, then enqueue its
//! task as a second statement. A server booting in that gap (role ide/all)
//! saw a `running` root with no events, no lease and no queue row, which
//! `cleanup_stale_runs` reads as "run never started" and fails, leaving the
//! queue row that lands a moment later queued under a dead run.
//!
//! Each case below makes the queue insert fail for one workspace with a
//! trigger, fires one seed path, and checks that no run row was left behind.
//! A run row that survives a failed enqueue is exactly the row cleanup could
//! see between the two statements. Safe on the shared database: this binary's
//! tests run in nextest's `serial-db` group, and the trigger only matches runs
//! of the test's own workspace.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(seed_atomicity_test)'

use agentic_pipeline::scheduler::{
    ScheduleInput, create_schedule, enqueue_app_function_job_in, enqueue_health_eval,
    enqueue_preagg_cycle, run_schedule_now,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

use crate::automation_recovery_test::test_db;
use crate::scheduler_test::FakeWorkspace;

/// Refuses every `agentic_task_queue` insert for a run of `workspace_id`.
struct RefuseEnqueue {
    tag: String,
}

impl RefuseEnqueue {
    async fn arm(db: &DatabaseConnection, workspace_id: Uuid) -> Self {
        let tag = format!("sa_{}", Uuid::new_v4().simple());
        db.execute_unprepared(&format!(
            "CREATE FUNCTION {tag}_fn() RETURNS trigger AS $$ BEGIN \
               IF (SELECT workspace_id FROM agentic_runs WHERE id = NEW.run_id) \
                  = '{workspace_id}'::uuid THEN \
                 RAISE EXCEPTION 'enqueue refused (test)'; \
               END IF; \
               RETURN NEW; \
             END $$ LANGUAGE plpgsql; \
             CREATE TRIGGER {tag}_trg BEFORE INSERT ON agentic_task_queue \
               FOR EACH ROW EXECUTE FUNCTION {tag}_fn();"
        ))
        .await
        .expect("arm the refused enqueue");
        Self { tag }
    }

    async fn disarm(self, db: &DatabaseConnection) {
        let tag = self.tag;
        db.execute_unprepared(&format!(
            "DROP TRIGGER IF EXISTS {tag}_trg ON agentic_task_queue; \
             DROP FUNCTION IF EXISTS {tag}_fn();"
        ))
        .await
        .expect("disarm the refused enqueue");
    }
}

async fn runs_in(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM agentic_runs WHERE workspace_id = $1",
            [workspace_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get::<i64>("", "n").unwrap()
}

async fn queued_in(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM agentic_task_queue q \
               JOIN agentic_runs r ON r.id = q.run_id WHERE r.workspace_id = $1",
            [workspace_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get::<i64>("", "n").unwrap()
}

fn schedule(kind: &str, target_ref: &str, question: Option<&str>) -> ScheduleInput {
    ScheduleInput {
        name: format!("seed-atomicity-{kind}"),
        target_kind: kind.to_string(),
        target_ref: target_ref.to_string(),
        question: question.map(str::to_string),
        variables: None,
        cron_expr: "0 9 * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
    }
}

/// Fire `input`'s schedule now with the enqueue refused: the fire fails and
/// leaves no run.
async fn assert_schedule_fire_is_atomic(input: ScheduleInput) {
    let Some(db) = test_db().await else { return };
    let ws = Uuid::new_v4();
    let kind = input.target_kind.clone();
    let s = create_schedule(&db, ws, input)
        .await
        .expect("create schedule");
    let refuse = RefuseEnqueue::arm(&db, ws).await;
    let fired = run_schedule_now(&db, ws, &FakeWorkspace, &s.id).await;
    let left = runs_in(&db, ws).await;
    refuse.disarm(&db).await;
    assert!(fired.is_err(), "{kind}: the fire must fail, got {fired:?}");
    assert_eq!(
        left, 0,
        "{kind}: a failed enqueue must not leave its run row"
    );
}

#[tokio::test]
async fn scheduled_automation_fire_commits_run_and_queue_together() {
    let r = format!("workflows/seed-{}.automation.yml", Uuid::new_v4());
    assert_schedule_fire_is_atomic(schedule("workflow", &r, None)).await;
}

#[tokio::test]
async fn scheduled_agent_fire_commits_run_and_queue_together() {
    let agent = format!("agents/seed-{}.agentic.yml", Uuid::new_v4());
    assert_schedule_fire_is_atomic(schedule("agent", &agent, Some("revenue?"))).await;
}

#[tokio::test]
async fn scheduled_function_fire_commits_run_and_queue_together() {
    let target = format!("{}/refresh", Uuid::new_v4());
    assert_schedule_fire_is_atomic(schedule("function", &target, None)).await;
}

#[tokio::test]
async fn health_eval_seed_commits_run_and_queue_together() {
    let Some(db) = test_db().await else { return };
    let ws = Uuid::new_v4();
    let refuse = RefuseEnqueue::arm(&db, ws).await;
    let refused = enqueue_health_eval(&db, ws, false).await;
    let left = runs_in(&db, ws).await;
    refuse.disarm(&db).await;
    assert!(refused.is_err(), "the seed must fail, got {refused:?}");
    assert_eq!(left, 0, "a failed enqueue must not leave its run row");

    // Unrefused, the same seed lands both rows: the trigger, not the seed, made
    // the call above fail.
    enqueue_health_eval(&db, ws, false)
        .await
        .expect("seed without the trigger");
    assert_eq!(runs_in(&db, ws).await, 1);
    assert_eq!(queued_in(&db, ws).await, 1);
}

#[tokio::test]
async fn preagg_cycle_seed_commits_run_and_queue_together() {
    let Some(db) = test_db().await else { return };
    let ws = Uuid::new_v4();
    let refuse = RefuseEnqueue::arm(&db, ws).await;
    let refused = enqueue_preagg_cycle(&db, ws, None).await;
    let left = runs_in(&db, ws).await;
    refuse.disarm(&db).await;
    assert!(refused.is_err(), "the seed must fail, got {refused:?}");
    assert_eq!(left, 0, "a failed enqueue must not leave its run row");
}

#[tokio::test]
async fn app_function_job_seed_commits_run_and_queue_together() {
    let Some(db) = test_db().await else { return };
    let ws = Uuid::new_v4();
    let app = Uuid::new_v4().to_string();
    let refuse = RefuseEnqueue::arm(&db, ws).await;
    let refused =
        enqueue_app_function_job_in(&db, &app, "refresh", ws, None, "manual", None, None, None)
            .await;
    let left = runs_in(&db, ws).await;
    refuse.disarm(&db).await;
    assert!(refused.is_err(), "the seed must fail, got {refused:?}");
    assert_eq!(left, 0, "a failed enqueue must not leave its run row");
}
