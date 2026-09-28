//! Phase 2 scheduler backend tests (P6).
//!
//! Real Postgres via testcontainers. Covers: CRUD round-trip +
//! validation, CAS exactly-once under concurrent ticks, misfire
//! run-once-then-resume, and run-now (Global seed, cadence untouched).
//!
//! All fire-path tests use a `workflow` target: `start_automation_run`
//! validates the ref + seeds + enqueues at seed time and never touches
//! the workspace (YAML loads at drive time), so the `FakeWorkspace` stub
//! below is never actually invoked.
//!
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(scheduler_test)'

use std::sync::Arc;

use agentic_airway::AirwayMigrator;
use agentic_pipeline::scheduler::{
    ScheduleError, ScheduleInput, create_schedule, delete_schedule, delete_workspace_schedules,
    enqueue_health_eval, enqueue_preagg_cycle, enqueue_preagg_cycle_for_promote, get_schedule,
    health_interval_cron, list_schedules, reconcile_health_schedule, reconcile_preagg_schedule,
    run_schedule_now, tick_health_schedules, tick_monitor_schedules, tick_preagg_schedules,
    tick_schedules, update_schedule,
};
use agentic_runtime::migration::RuntimeMigrator;
use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement,
};

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
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
                    Arc::new(
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
                            .expect("start Postgres testcontainer — is Docker running?"),
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
            Err(e) => panic!("connect after 10 retries: {e}"),
        }
    }
    let db = db?;
    // Central first, then runtime — every fixture on the shared test database
    // now follows production order (see oxy_test_utils::migration for the
    // rationale). This file used to skip central deliberately: central's
    // `agentic_runs` migrations weren't idempotent (`ADD COLUMN thread_id`,
    // and the `idx_agentic_run_events_run_id_seq` index) and collided with
    // `RuntimeMigrator`'s idempotent copies whenever runtime got there first.
    // That's now backwards — central is supposed to lead, and it's runtime's
    // migrator that carries the idempotency guards for the case where it
    // runs second (see `RationalizeStatusModel`'s `column_exists` checks).
    // Central-first also means `fire_schedule` (`scheduler.rs`), which
    // reaches `start_airway_run` and resolves against `airway_source_config`
    // — a central table — no longer needs a special case: it's part of the
    // shared migration this binary now runs before anything else.
    // `start_airway_run` also writes `airway_run_extensions`, owned by this
    // migrator — needed for the same reason as above.
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations")
        .then::<AirwayMigrator>()
        .await
        .expect("airway migrations")
        .finish()
        .await;
    Some(db)
}

/// Never actually called for `workflow` targets (see module docs); only
/// satisfies the `tick_schedules` / `run_schedule_now` signature.
struct FakeWorkspace;

#[async_trait]
impl agentic_automation::WorkspaceContext for FakeWorkspace {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        Some(std::path::Path::new(""))
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        _name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err("unused".into())
    }
    async fn get_integration(
        &self,
        _name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err("unused".into())
    }
    async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _r: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("unused".into())
    }
}

fn input(name: &str, target_ref: &str, cron_expr: &str) -> ScheduleInput {
    ScheduleInput {
        name: name.to_string(),
        target_kind: "workflow".to_string(),
        target_ref: target_ref.to_string(),
        question: None,
        variables: None,
        cron_expr: cron_expr.to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
    }
}

/// Unique workflow ref so fire-path assertions can count exactly the runs
/// this test seeded (the testcontainer is shared/reused across tests).
fn uniq_ref() -> String {
    format!("workflows/sched-{}.automation.yml", uuid::Uuid::new_v4())
}

async fn force_due(db: &DatabaseConnection, id: &str, secs_ago: i64) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE agentic_schedules SET next_run_at = now() - make_interval(secs => $1) WHERE id = $2",
        [secs_ago.into(), id.into()],
    ))
    .await
    .unwrap();
}

async fn run_count_for(db: &DatabaseConnection, workflow_ref: &str) -> i64 {
    #[derive(sea_orm::FromQueryResult)]
    struct C {
        c: i64,
    }
    C::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*)::int8 AS c FROM agentic_runs WHERE question = $1",
        [format!("workflow: {workflow_ref}").into()],
    ))
    .one(db)
    .await
    .unwrap()
    .map(|r| r.c)
    .unwrap_or(0)
}

#[tokio::test]
async fn crud_round_trip_and_validation() {
    let Some(db) = test_db().await else { return };
    let r = uniq_ref();
    let ws = uuid::Uuid::new_v4();

    // Validation: bad cron, bad kind, empty name → Invalid.
    assert!(matches!(
        create_schedule(&db, ws, input("n", &r, "not a cron")).await,
        Err(ScheduleError::Invalid(_))
    ));
    let mut bad_kind = input("n", &r, "0 9 * * *");
    bad_kind.target_kind = "bogus".into();
    assert!(matches!(
        create_schedule(&db, ws, bad_kind).await,
        Err(ScheduleError::Invalid(_))
    ));
    assert!(matches!(
        create_schedule(&db, ws, input("  ", &r, "0 9 * * *")).await,
        Err(ScheduleError::Invalid(_))
    ));

    // Create → get → list.
    let created = create_schedule(&db, ws, input("daily", &r, "0 9 * * *"))
        .await
        .unwrap();
    assert!(created.next_run_at > agentic_runtime::crud::now());
    let got = get_schedule(&db, ws, &created.id).await.unwrap();
    assert_eq!(got.name, "daily");
    assert!(
        list_schedules(&db, ws)
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == created.id)
    );

    // Update without cadence change → next_run_at preserved.
    let mut rename = input("renamed", &r, "0 9 * * *");
    rename.enabled = false;
    let updated = update_schedule(&db, ws, &created.id, rename).await.unwrap();
    assert_eq!(updated.name, "renamed");
    assert!(!updated.enabled);
    assert_eq!(
        updated.next_run_at, created.next_run_at,
        "rename must keep the slot"
    );

    // Update WITH cadence change → next_run_at recomputed.
    let updated2 = update_schedule(&db, ws, &created.id, input("renamed", &r, "0 * * * *"))
        .await
        .unwrap();
    assert_ne!(
        updated2.next_run_at, created.next_run_at,
        "cron change recomputes"
    );

    // Delete → gone, second delete is NotFound.
    delete_schedule(&db, ws, &created.id).await.unwrap();
    assert!(matches!(
        get_schedule(&db, ws, &created.id).await,
        Err(ScheduleError::NotFound)
    ));
    assert!(matches!(
        delete_schedule(&db, ws, &created.id).await,
        Err(ScheduleError::NotFound)
    ));
}

#[tokio::test]
async fn tick_cas_fires_exactly_once_under_concurrency() {
    let Some(db) = test_db().await else { return };
    let r = uniq_ref();
    let ws = uuid::Uuid::new_v4();
    let s = create_schedule(&db, ws, input("conc", &r, "0 9 * * *"))
        .await
        .unwrap();
    force_due(&db, &s.id, 3600).await;

    // Two concurrent ticks (simulating two replicas / overlapping cycles)
    // both see the due row; the CAS on next_run_at lets exactly one fire.
    let (db1, db2) = (db.clone(), db.clone());
    let (a, b) = tokio::join!(
        async move { tick_schedules(&db1, ws, &FakeWorkspace).await },
        async move { tick_schedules(&db2, ws, &FakeWorkspace).await },
    );

    assert_eq!(
        run_count_for(&db, &r).await,
        1,
        "exactly one run seeded for this schedule despite two ticks (a={a}, b={b})"
    );
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert!(after.next_run_at > agentic_runtime::crud::now());
    assert!(after.last_fired_at.is_some());
    assert!(after.last_run_id.is_some());
}

#[tokio::test]
async fn misfire_runs_once_then_resumes() {
    let Some(db) = test_db().await else { return };
    let r = uniq_ref();
    let ws = uuid::Uuid::new_v4();
    let s = create_schedule(&db, ws, input("misfire", &r, "0 9 * * *"))
        .await
        .unwrap();
    // A week overdue (many missed daily slots).
    force_due(&db, &s.id, 7 * 24 * 3600).await;

    tick_schedules(&db, ws, &FakeWorkspace).await;

    assert_eq!(
        run_count_for(&db, &r).await,
        1,
        "missed slots collapse to a single catch-up run"
    );
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert!(
        after.next_run_at > agentic_runtime::crud::now(),
        "next_run_at jumps to the next future slot, not the next missed one"
    );

    // A second immediate tick must NOT re-fire (already advanced).
    tick_schedules(&db, ws, &FakeWorkspace).await;
    assert_eq!(run_count_for(&db, &r).await, 1, "no double fire");
}

#[tokio::test]
async fn run_now_seeds_global_without_advancing() {
    let Some(db) = test_db().await else { return };
    let r = uniq_ref();
    let ws = uuid::Uuid::new_v4();
    let s = create_schedule(&db, ws, input("runnow", &r, "0 9 * * *"))
        .await
        .unwrap();
    let before_next = s.next_run_at;

    let run_id = run_schedule_now(&db, ws, &FakeWorkspace, &s.id)
        .await
        .unwrap();
    assert_eq!(run_count_for(&db, &r).await, 1);

    // The seeded run is Global (scope_owned=false), driverless.
    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        scope_owned: bool,
        queue_status: String,
    }
    let q = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_owned, queue_status FROM agentic_task_queue WHERE task_id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("queue row for the seeded run");
    assert!(!q.scope_owned, "run-now seeds a Global task");
    assert_eq!(q.queue_status, "queued");

    // Cadence untouched: run-now is out-of-band.
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert_eq!(
        after.next_run_at, before_next,
        "run-now must not advance next_run_at"
    );
    assert_eq!(after.last_run_id.as_deref(), Some(run_id.as_str()));
}

/// §12 FU4b: a tick for workspace A must fire only A's due schedule and
/// leave B's untouched, and vice versa. Asserts CRUD + tick scoping is
/// truly per-workspace.
#[tokio::test]
async fn multi_tenant_tick_isolation() {
    let Some(db) = test_db().await else { return };
    let ws_a = uuid::Uuid::new_v4();
    let ws_b = uuid::Uuid::new_v4();
    let r_a = uniq_ref();
    let r_b = uniq_ref();

    let s_a = create_schedule(&db, ws_a, input("A", &r_a, "0 9 * * *"))
        .await
        .unwrap();
    let s_b = create_schedule(&db, ws_b, input("B", &r_b, "0 9 * * *"))
        .await
        .unwrap();
    force_due(&db, &s_a.id, 3600).await;
    force_due(&db, &s_b.id, 3600).await;

    // Tick A only.
    let fired_a = tick_schedules(&db, ws_a, &FakeWorkspace).await;

    // A fired exactly once for r_a; r_b untouched.
    assert_eq!(run_count_for(&db, &r_a).await, 1);
    assert_eq!(run_count_for(&db, &r_b).await, 0);
    assert!(fired_a >= 1);
    let after_a = get_schedule(&db, ws_a, &s_a.id).await.unwrap();
    let after_b_pre = get_schedule(&db, ws_b, &s_b.id).await.unwrap();
    assert!(after_a.last_fired_at.is_some(), "A's tick fired A");
    assert!(
        after_b_pre.last_fired_at.is_none(),
        "A's tick must not touch B"
    );

    // Cross-workspace CRUD: get/update/delete of B from ws_a is NotFound,
    // never leaks the existence of another workspace's schedule.
    assert!(matches!(
        get_schedule(&db, ws_a, &s_b.id).await,
        Err(ScheduleError::NotFound)
    ));
    assert!(matches!(
        delete_schedule(&db, ws_a, &s_b.id).await,
        Err(ScheduleError::NotFound)
    ));
    assert!(matches!(
        update_schedule(&db, ws_a, &s_b.id, input("hijack", &r_b, "0 9 * * *")).await,
        Err(ScheduleError::NotFound)
    ));

    let fired_b = tick_schedules(&db, ws_b, &FakeWorkspace).await;
    assert_eq!(run_count_for(&db, &r_b).await, 1);
    assert_eq!(run_count_for(&db, &r_a).await, 1, "A unchanged");
    assert!(fired_b >= 1);
    let after_b = get_schedule(&db, ws_b, &s_b.id).await.unwrap();
    assert!(after_b.last_fired_at.is_some());

    // list_schedules is scoped: each workspace only sees its own row.
    let list_a = list_schedules(&db, ws_a).await.unwrap();
    let list_b = list_schedules(&db, ws_b).await.unwrap();
    assert!(list_a.iter().all(|s| s.workspace_id == ws_a));
    assert!(list_b.iter().all(|s| s.workspace_id == ws_b));
    assert!(list_a.iter().any(|s| s.id == s_a.id));
    assert!(!list_a.iter().any(|s| s.id == s_b.id));
    assert!(list_b.iter().any(|s| s.id == s_b.id));
}

// ── Agent target kind ────────────────────────────────────────────────────────

fn agent_input(name: &str, agent_ref: &str, question: &str, cron_expr: &str) -> ScheduleInput {
    ScheduleInput {
        name: name.to_string(),
        target_kind: "agent".to_string(),
        target_ref: agent_ref.to_string(),
        question: Some(question.to_string()),
        variables: None,
        cron_expr: cron_expr.to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
    }
}

/// `target_kind="agent"` requires a non-empty `question`. Empty / missing
/// surfaces as `Invalid` from `create_schedule` before any DB write.
#[tokio::test]
async fn agent_schedule_requires_question() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    // Missing question entirely.
    let mut no_q = agent_input("nq", "agents/foo", "", "0 9 * * *");
    no_q.question = None;
    assert!(matches!(
        create_schedule(&db, ws, no_q).await,
        Err(ScheduleError::Invalid(_))
    ));

    // Whitespace-only question.
    assert!(matches!(
        create_schedule(&db, ws, agent_input("ws", "agents/foo", "   ", "0 9 * * *")).await,
        Err(ScheduleError::Invalid(_))
    ));

    // Workflow / airway schedules MAY omit `question`.
    let mut wf = input("wf", &uniq_ref(), "0 9 * * *");
    wf.question = None;
    create_schedule(&db, ws, wf)
        .await
        .expect("workflow schedule must not require question");
}

/// `run_schedule_now` for an `agent` schedule seeds an analytics run:
/// `agentic_runs` row with `source_type="analytics"`, `schedule_id`
/// linked back to the schedule, metadata stamped with `agent_id` +
/// `question`, and an `analytics_run_extensions` row with the
/// `agent_id`. The queue row is `Global` (driverless) like the
/// workflow/airway paths.
#[tokio::test]
async fn agent_run_now_seeds_analytics_run() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();
    // Use a unique agent_id so we can isolate this run from sibling tests
    // running against the shared testcontainer.
    let agent_id = format!("agents/sched-{}.agentic.yml", uuid::Uuid::new_v4());
    let question = "What is yesterday's revenue?";

    let s = create_schedule(
        &db,
        ws,
        agent_input("agent-nightly", &agent_id, question, "0 9 * * *"),
    )
    .await
    .unwrap();
    let before_next = s.next_run_at;

    let run_id = run_schedule_now(&db, ws, &FakeWorkspace, &s.id)
        .await
        .expect("run_now succeeds");

    // The agentic_runs row: analytics source, linked to the schedule,
    // question stored verbatim, metadata.agent_id populated.
    #[derive(FromQueryResult)]
    struct RunRow {
        source_type: Option<String>,
        schedule_id: Option<String>,
        question: String,
        metadata: Option<serde_json::Value>,
    }
    let row = RunRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_type, schedule_id, question, metadata \
         FROM agentic_runs WHERE id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("run row exists for the seeded agent run");
    assert_eq!(row.source_type.as_deref(), Some("analytics"));
    assert_eq!(row.schedule_id.as_deref(), Some(s.id.as_str()));
    assert_eq!(row.question, question);
    let meta = row.metadata.expect("metadata stamped");
    assert_eq!(
        meta.get("agent_id").and_then(|v| v.as_str()),
        Some(agent_id.as_str())
    );
    assert_eq!(
        meta.get("question").and_then(|v| v.as_str()),
        Some(question)
    );
    // `run_now` stamps trigger="manual" (vs "scheduled" / "backfill").
    assert_eq!(meta.get("trigger").and_then(|v| v.as_str()), Some("manual"));

    // analytics_run_extensions row: agent_id matches what start_agent_run
    // wrote, no spec_hint yet (the run hasn't executed).
    #[derive(FromQueryResult)]
    struct Ext {
        agent_id: String,
    }
    let ext = Ext::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT agent_id FROM analytics_run_extensions WHERE run_id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("analytics extension row exists");
    assert_eq!(ext.agent_id, agent_id);

    // Queue row: Global (scope_owned=false), queued — driven by the
    // standalone consumer, not a co-located coordinator.
    #[derive(FromQueryResult)]
    struct Q {
        scope_owned: bool,
        queue_status: String,
    }
    let q = Q::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_owned, queue_status FROM agentic_task_queue WHERE task_id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("queue row for the seeded run");
    assert!(!q.scope_owned, "agent run-now seeds a Global task");
    assert_eq!(q.queue_status, "queued");

    // Cadence is untouched — `run_now` is out-of-band — and `last_run_id`
    // points at the freshly seeded run.
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert_eq!(
        after.next_run_at, before_next,
        "agent run-now must not advance next_run_at"
    );
    assert_eq!(after.last_run_id.as_deref(), Some(run_id.as_str()));
}

/// Scheduler tick for a due agent schedule fires `start_agent_run` and
/// links the seeded run back via `schedule_id`. Confirms the agent arm
/// of `fire_schedule` participates in the CAS / cadence-advance flow.
#[tokio::test]
async fn agent_schedule_tick_fires_once() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();
    let agent_id = format!("agents/tick-{}.agentic.yml", uuid::Uuid::new_v4());

    let s = create_schedule(
        &db,
        ws,
        agent_input("agent-tick", &agent_id, "Daily standup", "0 9 * * *"),
    )
    .await
    .unwrap();
    force_due(&db, &s.id, 3600).await;

    let fired = tick_schedules(&db, ws, &FakeWorkspace).await;
    assert!(fired >= 1, "tick fired at least one schedule");

    // Exactly one analytics run linked to this schedule.
    #[derive(FromQueryResult)]
    struct Row {
        id: String,
        source_type: Option<String>,
        trigger: Option<String>,
    }
    let runs = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, source_type, metadata->>'trigger' AS trigger \
         FROM agentic_runs WHERE schedule_id = $1",
        [s.id.clone().into()],
    ))
    .all(&db)
    .await
    .unwrap();
    assert_eq!(runs.len(), 1, "exactly one run seeded for this schedule");
    assert_eq!(runs[0].source_type.as_deref(), Some("analytics"));
    // Tick stamps trigger="scheduled" (vs "manual" / "backfill").
    assert_eq!(runs[0].trigger.as_deref(), Some("scheduled"));

    // Cadence advanced past now.
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert!(after.next_run_at > agentic_runtime::crud::now());
    assert!(after.last_fired_at.is_some());
    assert_eq!(after.last_run_id.as_deref(), Some(runs[0].id.as_str()));
}

/// Health-eval queue rows for a given workspace: count + scope flag.
#[derive(sea_orm::FromQueryResult)]
struct HealthQueueRow {
    workspace_id: String,
    scope_owned: bool,
}

async fn health_tasks_for(db: &DatabaseConnection, ws: uuid::Uuid) -> Vec<HealthQueueRow> {
    HealthQueueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT spec->'payload'->>'workspace_id' AS workspace_id, scope_owned \
         FROM agentic_task_queue \
         WHERE spec->>'kind' = 'health_eval_workspace' \
           AND spec->'payload'->>'workspace_id' = $1",
        [ws.to_string().into()],
    ))
    .all(db)
    .await
    .unwrap()
}

// ── Pre-aggregation cycle scheduling ────────────────────────────────────────
//
// Mirrors the health-eval block below exactly — same fire path, same
// task_id == run_id contract, just a different `target_kind` and a payload
// shape that carries `force`/`target` alongside `workspace_id`.

/// Preagg-cycle queue rows for a given workspace: force flag + target, so a
/// test can tell a scheduled (unforced, untargeted) fire apart from an
/// on-demand (`enqueue_preagg_cycle`) one from the payload alone.
#[derive(sea_orm::FromQueryResult)]
struct PreaggQueueRow {
    workspace_id: String,
    force: bool,
    target: Option<String>,
    scope_owned: bool,
}

async fn preagg_tasks_for(db: &DatabaseConnection, ws: uuid::Uuid) -> Vec<PreaggQueueRow> {
    PreaggQueueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT spec->'payload'->>'workspace_id' AS workspace_id, \
                (spec->'payload'->>'force')::bool AS force, \
                spec->'payload'->>'target' AS target, \
                scope_owned \
         FROM agentic_task_queue \
         WHERE spec->>'kind' = 'preagg_cycle' \
           AND spec->'payload'->>'workspace_id' = $1 \
         ORDER BY created_at",
        [ws.to_string().into()],
    ))
    .all(db)
    .await
    .unwrap()
}

/// `reconcile_preagg_schedule` creates the row when absent, and on an
/// existing row only updates `cron_expr`/`enabled` when they actually
/// changed — an unchanged reconcile leaves the next fire slot alone.
#[tokio::test]
async fn reconcile_preagg_schedule_creates_then_leaves_steady_state_alone() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    reconcile_preagg_schedule(&db, ws, std::time::Duration::from_secs(600), true)
        .await
        .unwrap();
    let created = get_schedule(&db, ws, &list_schedules(&db, ws).await.unwrap()[0].id)
        .await
        .unwrap();
    assert_eq!(created.target_kind, "preagg_cycle");
    assert_eq!(created.target_ref, ws.to_string());
    assert!(created.enabled);

    // Same interval, same enabled: steady state, next_run_at untouched.
    reconcile_preagg_schedule(&db, ws, std::time::Duration::from_secs(600), true)
        .await
        .unwrap();
    let unchanged = get_schedule(&db, ws, &created.id).await.unwrap();
    assert_eq!(unchanged.next_run_at, created.next_run_at);

    // A real cadence change moves the fire slot.
    reconcile_preagg_schedule(&db, ws, std::time::Duration::from_secs(1800), true)
        .await
        .unwrap();
    let recadenced = get_schedule(&db, ws, &created.id).await.unwrap();
    assert_ne!(recadenced.cron_expr, created.cron_expr);

    // Disabling flips `enabled` without deleting the row.
    reconcile_preagg_schedule(&db, ws, std::time::Duration::from_secs(1800), false)
        .await
        .unwrap();
    assert!(!get_schedule(&db, ws, &created.id).await.unwrap().enabled);

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// A due per-workspace `preagg_cycle` row enqueues exactly one Global,
/// unforced, untargeted Custom task and advances its cadence; a second tick
/// does not double-enqueue.
#[tokio::test]
async fn due_preagg_row_enqueues_one_global_custom_task() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    let s = create_schedule(
        &db,
        ws,
        ScheduleInput {
            name: "Pre-aggregation cycle".to_string(),
            target_kind: "preagg_cycle".to_string(),
            target_ref: ws.to_string(),
            question: None,
            variables: None,
            cron_expr: health_interval_cron(std::time::Duration::from_secs(600)),
            timezone: "UTC".to_string(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    // Oldest-due by a decade — see the per-tick fire cap invariant note on
    // `due_health_row_enqueues_one_global_custom_task`; the same shared-
    // testcontainer reasoning applies here.
    force_due(&db, &s.id, 10 * 365 * 24 * 3600).await;

    let fired = tick_preagg_schedules(&db).await;
    assert!(fired >= 1, "at least this workspace's row fired");

    let tasks = preagg_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 1, "exactly one queued preagg task for the ws");
    assert_eq!(tasks[0].workspace_id, ws.to_string());
    assert!(
        !tasks[0].force,
        "a scheduled fire honors refresh keys, unforced"
    );
    assert!(
        tasks[0].target.is_none(),
        "a scheduled fire covers every rollup"
    );
    assert!(!tasks[0].scope_owned, "preagg task is TaskScope::Global");

    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert!(after.next_run_at > agentic_runtime::crud::now());
    assert!(after.last_fired_at.is_some());

    tick_preagg_schedules(&db).await;
    assert_eq!(
        preagg_tasks_for(&db, ws).await.len(),
        1,
        "this workspace's row not due → no double-enqueue"
    );

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// Same regression class as `fired_health_run_is_picked_up_by_latency_worker`:
/// the seeded root task_id must equal run_id or the pickup query never
/// matches it and the run hangs `running` forever.
#[tokio::test]
async fn fired_preagg_run_is_picked_up_by_latency_worker() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    let s = create_schedule(
        &db,
        ws,
        ScheduleInput {
            name: "Pre-aggregation cycle".to_string(),
            target_kind: "preagg_cycle".to_string(),
            target_ref: ws.to_string(),
            question: None,
            variables: None,
            cron_expr: health_interval_cron(std::time::Duration::from_secs(600)),
            timezone: "UTC".to_string(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    force_due(&db, &s.id, 10 * 365 * 24 * 3600).await;

    let fired = tick_preagg_schedules(&db).await;
    assert!(fired >= 1, "at least this workspace's row fired");

    let pending = agentic_runtime::crud::find_pending_global_runs(&db, Some(ws))
        .await
        .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "the fired preagg run must be picked up by find_pending_global_runs"
    );

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// `enqueue_preagg_cycle` — the IDE's Rebuild buttons — always forces, and
/// carries the target through untouched (or `None` for "Rebuild all").
#[tokio::test]
async fn enqueue_preagg_cycle_is_forced_and_carries_the_target() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    enqueue_preagg_cycle(
        &db,
        ws,
        Some(("orders".to_string(), "orders_by_month".to_string())),
    )
    .await
    .unwrap();
    let tasks = preagg_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].force, "a Rebuild click always forces");
    let target = tasks[0].target.as_deref().expect("target carried through");
    assert!(target.contains("orders_by_month"));

    enqueue_preagg_cycle(&db, ws, None).await.unwrap();
    let tasks = preagg_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 2, "a second, distinct on-demand run");
    assert!(tasks[1].target.is_none(), "Rebuild all carries no target");

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// The post-promote nudge is a TICK, not a Rebuild click: unforced and
/// untargeted, exactly like a cron fire, so the executor still consults each
/// rollup's refresh key and only builds what a changed definition made stale.
/// Forcing here would rebuild every rollup the promote never touched.
///
/// It also attributes to the workspace's schedule row when there is one, so
/// the run shows up in the same history as its scheduled siblings — with
/// `trigger: "promote"`, which is neither of the two triggers that existed.
#[tokio::test]
async fn a_promote_nudge_is_an_unforced_untargeted_cycle() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    reconcile_preagg_schedule(&db, ws, std::time::Duration::from_secs(600), true)
        .await
        .unwrap();
    let row = &list_schedules(&db, ws).await.unwrap()[0];
    let schedule_id = row.id.clone();
    let next_before = row.next_run_at;

    let run_id = enqueue_preagg_cycle_for_promote(&db, ws)
        .await
        .unwrap()
        .expect("nothing queued yet, so this promote seeds a cycle");

    let tasks = preagg_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 1, "one queued cycle for the workspace");
    assert!(
        !tasks[0].force,
        "a definition change is genuinely stale; the tick was what was missing, not an override"
    );
    assert!(
        tasks[0].target.is_none(),
        "one promote can re-hash rollups across several views"
    );
    assert!(!tasks[0].scope_owned, "preagg task is TaskScope::Global");

    assert_eq!(
        trigger_of(&db, &run_id).await.as_deref(),
        Some("promote"),
        "the run log must be able to say why this fired"
    );

    // The cadence is untouched: the nudge adds a fire, it does not move one.
    // A nudge that also reset `next_run_at` would push the scheduled sweep out
    // by a full interval every time someone promoted.
    assert_eq!(
        get_schedule(&db, ws, &schedule_id)
            .await
            .unwrap()
            .next_run_at,
        next_before
    );

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// A workspace that has never compiled has no schedule row to attribute to,
/// and the nudge must still seed the cycle — a first promote is the case where
/// waiting out a heartbeat is least defensible.
#[tokio::test]
async fn a_promote_nudge_without_a_schedule_row_still_seeds_the_cycle() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    enqueue_preagg_cycle_for_promote(&db, ws)
        .await
        .unwrap()
        .expect("an unattributed cycle is still a cycle");
    assert_eq!(preagg_tasks_for(&db, ws).await.len(), 1);

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// A second promote while a cycle is still QUEUED joins it instead of stacking
/// a second one. The tick cannot double-fire — it CAS-advances `next_run_at` —
/// but this path fires on an event a person repeats, so without the guard a
/// burst of promotes queues a cycle each.
#[tokio::test]
async fn a_promote_joins_a_cycle_that_is_still_queued() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    let first = enqueue_preagg_cycle_for_promote(&db, ws)
        .await
        .unwrap()
        .expect("first promote seeds one");
    let second = enqueue_preagg_cycle_for_promote(&db, ws).await.unwrap();

    assert!(
        second.is_none(),
        "the second promote joined the queued cycle"
    );
    let tasks = preagg_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 1, "still exactly one queued cycle");
    assert!(!first.is_empty());

    // ...and once that cycle is CLAIMED it no longer counts: a running cycle
    // may have loaded its views before this promote landed, so joining one
    // would put the new hash back on the heartbeat — the defect this path
    // exists to fix.
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status = 'claimed' \
         WHERE spec->>'kind' = 'preagg_cycle' \
           AND spec->'payload'->>'workspace_id' = $1",
        [ws.to_string().into()],
    ))
    .await
    .unwrap();

    let third = enqueue_preagg_cycle_for_promote(&db, ws).await.unwrap();
    assert!(
        third.is_some(),
        "a claimed cycle may already have read the old definitions; queue a fresh one"
    );
    assert_eq!(preagg_tasks_for(&db, ws).await.len(), 2);

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// A queued cycle only counts if it is UNTARGETED. The Rebuild button enqueues
/// a targeted one, which covers exactly the rollup it names, so a promote that
/// joined it would have its own rollup silently dropped — this PR's defect,
/// logged as if it had been handled.
#[tokio::test]
async fn a_promote_does_not_join_a_targeted_rebuild() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    // What clicking Rebuild on one rollup leaves in the queue.
    enqueue_preagg_cycle(
        &db,
        ws,
        Some(("orders".to_string(), "orders_by_month".to_string())),
    )
    .await
    .unwrap();

    // A promote that re-hashed something in a different view entirely.
    let seeded = enqueue_preagg_cycle_for_promote(&db, ws).await.unwrap();
    assert!(
        seeded.is_some(),
        "a targeted cycle builds one rollup; it is not a cycle this promote can join"
    );

    let tasks = preagg_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 2);
    assert!(
        tasks[1].target.is_none(),
        "the promote's own cycle is untargeted"
    );

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// `metadata.trigger` on a run, so a test can assert what the run log will say.
async fn trigger_of(db: &DatabaseConnection, run_id: &str) -> Option<String> {
    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        trigger: Option<String>,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT metadata->>'trigger' AS trigger FROM agentic_runs WHERE id = $1",
        [run_id.into()],
    ))
    .one(db)
    .await
    .unwrap()
    .and_then(|r| r.trigger)
}

/// Regression: "Run now" on the Pre-aggregation cycle job failed with
/// `unknown target_kind "preagg_cycle"` — `fire_schedule` only knew the
/// user-created kinds, and unlike `monitor_scan`/`health_eval` (which the host
/// run-now handler special-cases and runs inline) nothing else covered preagg.
/// A manual fire seeds one unforced, untargeted Global task, exactly like the
/// tick — the Rebuild buttons stay the forcing path.
#[tokio::test]
async fn run_now_on_a_preagg_schedule_seeds_an_unforced_cycle() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    let s = create_schedule(
        &db,
        ws,
        ScheduleInput {
            name: "Pre-aggregation cycle".to_string(),
            target_kind: "preagg_cycle".to_string(),
            target_ref: ws.to_string(),
            question: None,
            variables: None,
            cron_expr: health_interval_cron(std::time::Duration::from_secs(600)),
            timezone: "UTC".to_string(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    let before_next = s.next_run_at;

    let run_id = run_schedule_now(&db, ws, &FakeWorkspace, &s.id)
        .await
        .expect("run-now must fire a preagg schedule");

    let tasks = preagg_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 1, "exactly one queued preagg task");
    assert_eq!(tasks[0].workspace_id, ws.to_string());
    assert!(
        !tasks[0].force,
        "a manual fire of the schedule still honors refresh keys"
    );
    assert!(tasks[0].target.is_none(), "covers every rollup");
    assert!(!tasks[0].scope_owned, "preagg task is TaskScope::Global");

    // Run-now is out-of-band: the cadence must be untouched, the run attributed
    // to the schedule, and labelled `manual` in the run history.
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert_eq!(
        after.next_run_at, before_next,
        "run-now doesn't advance cron"
    );
    assert_eq!(after.last_run_id.as_deref(), Some(run_id.as_str()));
    assert!(after.last_error.is_none(), "no last_error on a good fire");

    #[derive(sea_orm::FromQueryResult)]
    struct RunRow {
        source_type: Option<String>,
        schedule_id: Option<String>,
        trigger: Option<String>,
    }
    let run = RunRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_type, schedule_id, metadata->>'trigger' AS trigger \
         FROM agentic_runs WHERE id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("seeded run row");
    assert_eq!(run.source_type.as_deref(), Some("preagg_cycle"));
    assert_eq!(run.schedule_id.as_deref(), Some(s.id.as_str()));
    assert_eq!(run.trigger.as_deref(), Some("manual"));

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// A due per-workspace `health_eval` row enqueues exactly one Global
/// `health_eval_workspace` Custom task and advances its cadence; a second tick
/// does not double-enqueue.
#[tokio::test]
async fn due_health_row_enqueues_one_global_custom_task() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    // Per-workspace health row: target_ref = workspace id, interval sentinel.
    let s = create_schedule(
        &db,
        ws,
        ScheduleInput {
            name: "Health check".to_string(),
            target_kind: "health_eval".to_string(),
            target_ref: ws.to_string(),
            question: None,
            variables: None,
            cron_expr: health_interval_cron(std::time::Duration::from_secs(600)),
            timezone: "UTC".to_string(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    // Due by a decade, NOT a few seconds — and the margin is the point, not a
    // bigger number to win a race with.
    //
    // `tick_health_schedules` selects `ORDER BY next_run_at ASC LIMIT
    // MAX_HEALTH_FIRES_PER_TICK` (256), and this testcontainer is shared and
    // long-lived, so it carries due `health_eval` rows left by other tests. A
    // row due 5s ago sorts behind all of them and falls outside the cap: this
    // workspace never fires, while `fired >= 1` still passes on everyone
    // else's rows. The assertions below are about *this* workspace, so the row
    // has to be inside the cap for them to mean anything.
    //
    // The invariant a decade buys: for this row to sort outside 256, there
    // must be 256 rows due *more than ten years* ago. Nothing creates those.
    // A forced row cannot linger at its forced timestamp either — the tick's
    // `cas_advance_next_run` advances `next_run_at` before it even attempts
    // the enqueue, so every forced row the tick sees is pushed back into the
    // future whether the fire succeeds or not. That is an invariant about what
    // rows can exist, not a bet on how many have accumulated.
    force_due(&db, &s.id, 10 * 365 * 24 * 3600).await;

    // `tick_health_schedules` is a global tick and the testcontainer is shared
    // across tests, so the returned `fired` count includes other workspaces'
    // due rows — assert per-workspace effects instead.
    let fired = tick_health_schedules(&db).await;
    assert!(fired >= 1, "at least this workspace's row fired");

    let tasks = health_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 1, "exactly one queued health task for the ws");
    assert_eq!(tasks[0].workspace_id, ws.to_string());
    assert!(!tasks[0].scope_owned, "health task is TaskScope::Global");

    // Cadence advanced ~600s into the future; this workspace's row is not due
    // again, so a second tick must not enqueue a second task for it.
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert!(after.next_run_at > agentic_runtime::crud::now());
    assert!(after.last_fired_at.is_some());

    tick_health_schedules(&db).await;
    assert_eq!(
        health_tasks_for(&db, ws).await.len(),
        1,
        "this workspace's row not due → no double-enqueue"
    );

    // Don't leave a row behind to become someone else's backlog — see
    // `health_tick_caps_fires_per_pass` for why that matters here.
    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// Regression: the health run a tick seeds must be visible to the latency
/// worker's pickup query (`find_pending_global_runs`), or it sits `running`
/// forever and the dashboard shows "Health check" hung indefinitely while a
/// manual run-now (which evaluates inline) succeeds.
///
/// The bug: `start_health_eval_run` enqueued the root task with a custom id
/// (`health_eval:{ws}:{fire_slot}`) instead of `task_id = run_id`. The pickup
/// query's `q.task_id = r.id OR q.task_id LIKE r.id || '.%'` clause then never
/// matched, so the run was never driven. Every other Global seed uses
/// `task_id == run_id`; this test pins the health path to the same contract.
#[tokio::test]
async fn fired_health_run_is_picked_up_by_latency_worker() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    let s = create_schedule(
        &db,
        ws,
        ScheduleInput {
            name: "Health check".to_string(),
            target_kind: "health_eval".to_string(),
            target_ref: ws.to_string(),
            question: None,
            variables: None,
            cron_expr: health_interval_cron(std::time::Duration::from_secs(600)),
            timezone: "UTC".to_string(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    // Oldest-due by a decade — see the invariant note on the per-tick fire cap
    // in `due_health_row_enqueues_one_global_custom_task`.
    force_due(&db, &s.id, 10 * 365 * 24 * 3600).await;

    let fired = tick_health_schedules(&db).await;
    assert!(fired >= 1, "at least this workspace's row fired");

    // The seeded Global run must be returned by the latency worker's selection
    // (scoped to this workspace), with a non-`scope_owned` queued root task —
    // exactly what `find_pending_global_runs` keys off `task_id = run_id`.
    let pending = agentic_runtime::crud::find_pending_global_runs(&db, Some(ws))
        .await
        .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "the fired health run must be picked up by find_pending_global_runs; \
         a custom root task_id (≠ run_id) makes it invisible and it hangs forever",
    );
    assert_eq!(pending[0].workspace_id, ws);

    // The run must carry `schedule_id` so the per-job run-history query
    // (`WHERE schedule_id = $1`) surfaces scheduled fires under "Recent runs".
    // A plain `insert_run` leaves it NULL → the fire ran but is invisible on
    // the job page (only manual run-now, which stamps it, would show).
    #[derive(sea_orm::FromQueryResult)]
    struct ScheduleIdRow {
        schedule_id: Option<String>,
    }
    let row = ScheduleIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT schedule_id FROM agentic_runs WHERE id = $1",
        [pending[0].run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("seeded health run row exists");
    assert_eq!(
        row.schedule_id.as_deref(),
        Some(s.id.as_str()),
        "scheduled health run must stamp schedule_id or it won't appear in the job's run history",
    );

    delete_workspace_schedules(&db, ws).await.unwrap();
}

/// An operator-triggered eval (`enqueue_health_eval`) enqueues the same Global
/// `health_eval_workspace` Custom task the scheduled fire does — the heavy eval
/// runs on the worker fleet, not inline in the HTTP handler — and attributes the
/// run to the workspace's health schedule when one exists (so manual fires show
/// under the job's run history alongside scheduled ones).
#[tokio::test]
async fn manual_health_eval_enqueues_global_custom_task() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    // A reconciled health row exists (created at compile/startup); the manual
    // run should attribute to it.
    reconcile_health_schedule(&db, ws, std::time::Duration::from_secs(600), true)
        .await
        .unwrap();
    let schedule_id = health_rows_for(&db, ws).await[0].id.clone();

    let run_id = enqueue_health_eval(&db, ws, false).await.unwrap();
    assert!(
        !run_id.is_empty(),
        "returns the enqueued run id for polling"
    );

    let tasks = health_tasks_for(&db, ws).await;
    assert_eq!(tasks.len(), 1, "manual eval enqueues exactly one task");
    assert_eq!(tasks[0].workspace_id, ws.to_string());
    assert!(
        !tasks[0].scope_owned,
        "manual health task is TaskScope::Global, drained by the worker fleet",
    );

    #[derive(sea_orm::FromQueryResult)]
    struct ScheduleIdRow {
        schedule_id: Option<String>,
    }
    let row = ScheduleIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT schedule_id FROM agentic_runs WHERE id = $1",
        [run_id.into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("manual health run row exists");
    assert_eq!(
        row.schedule_id.as_deref(),
        Some(schedule_id.as_str()),
        "manual eval attributes to the workspace's health schedule when present",
    );
}

/// A manual eval fired before the workspace's health row exists
/// (pre-first-compile) still enqueues the task — the run is just inserted
/// unattributed (no `schedule_id` to stamp), exercising the `insert_run` branch.
#[tokio::test]
async fn manual_health_eval_without_schedule_row_still_enqueues() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    let run_id = enqueue_health_eval(&db, ws, false).await.unwrap();
    assert!(!run_id.is_empty());

    let tasks = health_tasks_for(&db, ws).await;
    assert_eq!(
        tasks.len(),
        1,
        "enqueues even with no schedule row (insert_run branch)",
    );
    assert_eq!(tasks[0].workspace_id, ws.to_string());
}

/// Total `health_eval_workspace` tasks queued across the given workspaces.
async fn enqueued_for(db: &DatabaseConnection, wss: &[uuid::Uuid]) -> usize {
    let mut n = 0;
    for ws in wss {
        n += health_tasks_for(db, *ws).await.len();
    }
    n
}

/// The per-tick cap bounds a post-outage burst: with more due rows than
/// `MAX_HEALTH_FIRES_PER_TICK` (every workspace elapsed together), one tick fires
/// at most the cap and the backlog drains over the next tick — not one N-wide
/// enqueue spike. Regression guard for the `ORDER BY next_run_at … LIMIT` in
/// `tick_health_schedules`; without it a future change to the due query could
/// silently reintroduce the burst.
#[tokio::test]
async fn health_tick_caps_fires_per_pass() {
    let Some(db) = test_db().await else { return };
    // Mirror MAX_HEALTH_FIRES_PER_TICK in scheduler.rs.
    const CAP: usize = 256;
    const EXTRA: usize = 20;
    let total = CAP + EXTRA;

    // Seed > CAP per-workspace health rows, all forced due (staggered into the
    // past so ordering is well-defined). Unique workspace ids so we can count
    // exactly our own enqueues against the shared container.
    let mut wss = Vec::with_capacity(total);
    for i in 0..total {
        let ws = uuid::Uuid::new_v4();
        let s = create_schedule(
            &db,
            ws,
            ScheduleInput {
                name: "Health check".to_string(),
                target_kind: "health_eval".to_string(),
                target_ref: ws.to_string(),
                question: None,
                variables: None,
                cron_expr: health_interval_cron(std::time::Duration::from_secs(600)),
                timezone: "UTC".to_string(),
                enabled: true,
            },
        )
        .await
        .unwrap();
        force_due(&db, &s.id, (i + 1) as i64).await;
        wss.push(ws);
    }

    // One tick is bounded by the cap even though all `total` rows are due — the
    // regression this guards: without the LIMIT this fires all `total` at once.
    // Robust to any unrelated due rows the shared/reused container carries: with
    // >= CAP rows due, a capped tick fires exactly CAP.
    let fired1 = tick_health_schedules(&db).await;
    assert_eq!(fired1, CAP, "one tick fires at most the per-tick cap");

    // Not all of *our* rows drained in that single pass — a backlog remains.
    let after1 = enqueued_for(&db, &wss).await;
    assert!(after1 <= CAP, "our fired count can't exceed the cap");
    assert!(
        after1 < total,
        "a backlog of our rows remains for the next tick"
    );

    // Drain the rest; every one of our workspaces ends up enqueued exactly once.
    let mut ticks = 1;
    while tick_health_schedules(&db).await > 0 {
        ticks += 1;
        assert!(ticks < 100, "health tick must converge");
    }
    assert!(
        ticks >= 2,
        "a more-than-cap backlog needs more than one tick"
    );
    assert_eq!(
        enqueued_for(&db, &wss).await,
        total,
        "every seeded workspace fired exactly once across the ticks"
    );

    // Clean up the 276 rows this test seeded. `tick_health_schedules` is
    // global and `agentic_schedules` has no FK to cascade off, so a row left
    // here outlives the test on the shared, reused testcontainer and comes due
    // again 600s later — forever. This one test is the dominant contributor to
    // that backlog (276 rows per run, vs one apiece from its neighbours), and
    // the backlog is what pushed the two `health_eval` assertions above outside
    // `MAX_HEALTH_FIRES_PER_TICK` and made them flake. Those two now force
    // their rows oldest-due so ordering can't hurt them; this stops the
    // pressure at its source, and keeps every later tick in the file cheap
    // rather than draining thousands of strangers' rows.
    //
    // Best-effort by construction: a panic above skips it. That is acceptable
    // — this bounds normal growth, it isn't a correctness guarantee, and the
    // decade-margin above is what the assertions actually rely on.
    for ws in &wss {
        delete_workspace_schedules(&db, *ws).await.unwrap();
    }
}

/// `delete_workspace_schedules` removes every schedule row for a workspace
/// (regardless of `target_kind`) and returns the count, while leaving other
/// workspaces' rows untouched. This is the cleanup `delete_workspace` runs so a
/// deleted workspace's `health_eval` row doesn't keep firing into the dead-letter
/// queue (schedules carry a plain `workspace_id`, no FK, so nothing cascades).
#[tokio::test]
async fn delete_workspace_schedules_removes_all_for_ws_only() {
    let Some(db) = test_db().await else { return };
    let ws_a = uuid::Uuid::new_v4();
    let ws_b = uuid::Uuid::new_v4();

    // ws_a: a workflow schedule + its health_eval row (the orphan-prone one).
    create_schedule(&db, ws_a, input("wf-a", &uniq_ref(), "0 9 * * *"))
        .await
        .unwrap();
    reconcile_health_schedule(&db, ws_a, std::time::Duration::from_secs(600), true)
        .await
        .unwrap();
    // ws_b: its own health row — must survive ws_a's deletion.
    reconcile_health_schedule(&db, ws_b, std::time::Duration::from_secs(600), true)
        .await
        .unwrap();

    let removed = delete_workspace_schedules(&db, ws_a).await.unwrap();
    assert_eq!(removed, 2, "both of ws_a's rows removed");
    assert!(
        list_schedules(&db, ws_a).await.unwrap().is_empty(),
        "ws_a has no schedules left — including its health_eval row"
    );
    assert_eq!(
        health_rows_for(&db, ws_b).await.len(),
        1,
        "ws_b's health row is untouched"
    );

    // Idempotent: a second delete removes nothing (no error).
    assert_eq!(delete_workspace_schedules(&db, ws_a).await.unwrap(), 0);
}

async fn health_rows_for(
    db: &DatabaseConnection,
    ws: uuid::Uuid,
) -> Vec<agentic_runtime::entity::schedule::Model> {
    use agentic_runtime::entity::schedule;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    schedule::Entity::find()
        .filter(schedule::Column::TargetKind.eq("health_eval"))
        .filter(schedule::Column::WorkspaceId.eq(ws))
        .all(db)
        .await
        .unwrap()
}

/// `reconcile_health_schedule` creates the per-workspace row, is a no-op on an
/// unchanged cadence (next fire slot preserved), and updates the cron on change.
#[tokio::test]
async fn reconcile_creates_then_updates_idempotently() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();

    reconcile_health_schedule(&db, ws, std::time::Duration::from_secs(1800), true)
        .await
        .unwrap();
    let rows = health_rows_for(&db, ws).await;
    assert_eq!(rows.len(), 1, "creates exactly one row");
    assert_eq!(rows[0].cron_expr, "@interval:1800");
    let first_next = rows[0].next_run_at;

    // Same cadence → no churn, next_run_at preserved.
    reconcile_health_schedule(&db, ws, std::time::Duration::from_secs(1800), true)
        .await
        .unwrap();
    let rows = health_rows_for(&db, ws).await;
    assert_eq!(rows.len(), 1, "still one row");
    assert_eq!(
        rows[0].next_run_at, first_next,
        "unchanged cadence preserves the fire slot"
    );

    // Changed cadence → cron updated.
    reconcile_health_schedule(&db, ws, std::time::Duration::from_secs(3600), true)
        .await
        .unwrap();
    let rows = health_rows_for(&db, ws).await;
    assert_eq!(rows.len(), 1, "still one row after update");
    assert_eq!(rows[0].cron_expr, "@interval:3600");

    // Disable → enabled flips, still one row.
    reconcile_health_schedule(&db, ws, std::time::Duration::from_secs(3600), false)
        .await
        .unwrap();
    let rows = health_rows_for(&db, ws).await;
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].enabled, "enabled flag reconciled to false");
}

// ── App-function job (manual/API trigger, no schedule) ───────────────────────

/// `enqueue_app_function_job` is the ad-hoc "run this function as a job now"
/// path (no schedule row). It seeds a Global `app_function` run stamped
/// `trigger="manual"` with no `schedule_id`, and enqueues a queued Custom task
/// carrying the app id / function name and the resolved retry policy — the
/// contract `trigger_function_job` (host) and the coordinator monitoring rely on.
#[tokio::test]
async fn app_function_job_seeds_manual_run_with_policy() {
    use agentic_pipeline::scheduler::enqueue_app_function_job;
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();
    let app_id = uuid::Uuid::new_v4().to_string();
    let policy = agentic_core::delegation::TaskPolicy {
        retry: Some(agentic_core::delegation::RetryPolicy {
            max_retries: 2,
            backoff: agentic_core::delegation::BackoffStrategy::Exponential {
                initial_delay_ms: 1000,
                max_delay_ms: 30000,
            },
            retry_on: vec![],
        }),
        fallback_targets: vec![],
    };

    let run_id = enqueue_app_function_job(
        &db,
        &app_id,
        "refresh-token",
        ws,
        Some(policy),
        "manual",
        Some(serde_json::json!({ "store": 7 })),
        Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string()),
    )
    .await
    .expect("enqueue succeeds");

    // Run row: app_function source, manual trigger, no schedule_id (ad-hoc), and
    // scoped to the caller's workspace.
    #[derive(FromQueryResult)]
    struct RunRow {
        source_type: Option<String>,
        schedule_id: Option<String>,
        trigger: Option<String>,
        workspace_id: String,
    }
    let row = RunRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_type, schedule_id, metadata->>'trigger' AS trigger, \
                workspace_id::text AS workspace_id \
         FROM agentic_runs WHERE id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("run row exists for the seeded job");
    assert_eq!(row.source_type.as_deref(), Some("app_function"));
    assert_eq!(row.trigger.as_deref(), Some("manual"));
    assert!(
        row.schedule_id.is_none(),
        "an ad-hoc job carries no schedule_id"
    );
    assert_eq!(row.workspace_id, ws.to_string());

    // Queue row: Global (driverless), queued, Custom `app_function` spec with the
    // app/function payload, and the retry policy attached to the task.
    #[derive(FromQueryResult)]
    struct QRow {
        scope_owned: bool,
        queue_status: String,
        kind: Option<String>,
        payload_app: Option<String>,
        payload_fn: Option<String>,
        payload_input_store: Option<i64>,
        max_retries: Option<i64>,
    }
    let q = QRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_owned, queue_status, \
                spec->>'kind' AS kind, \
                spec->'payload'->>'app_id' AS payload_app, \
                spec->'payload'->>'function_name' AS payload_fn, \
                (spec->'payload'->'input'->>'store')::int8 AS payload_input_store, \
                (policy->'retry'->>'max_retries')::int8 AS max_retries \
         FROM agentic_task_queue WHERE task_id = $1",
        [run_id.clone().into()],
    ))
    .one(&db)
    .await
    .unwrap()
    .expect("queue row exists for the seeded job");
    assert!(!q.scope_owned, "manual job seeds a Global task");
    assert_eq!(q.queue_status, "queued");
    assert_eq!(q.kind.as_deref(), Some("app_function"));
    assert_eq!(q.payload_app.as_deref(), Some(app_id.as_str()));
    assert_eq!(q.payload_fn.as_deref(), Some("refresh-token"));
    assert_eq!(
        q.payload_input_store,
        Some(7),
        "the input params ride on the enqueued task for the worker to replay"
    );
    assert_eq!(
        q.max_retries,
        Some(2),
        "the resolved retry policy rides on the enqueued task"
    );
}

// monitor_scan schedules
//
// `tick_monitor_schedules` is a second, near-duplicate copy of the fire path
// above — its own due query, its own CAS, its own misfire accounting — and
// none of it had ever executed: `OXY_INPROC_GLOBAL_WORKER` is off by default,
// so every scan on record arrived by POST. The cases below are the ones the
// workflow variant already covers, re-aimed at the copy that ships unexercised.

/// A `PlatformContext` with no `MonitorScanPort`, so the spawned scan fails
/// immediately. Everything under test here happens *before* the spawn — the
/// CAS, the run row, the misfire accounting — and a scan that cannot run keeps
/// the test off the warehouse.
struct FakePlatform;

#[async_trait]
impl agentic_pipeline::platform::ProjectContext for FakePlatform {
    async fn resolve_connector(
        &self,
        _db_name: &str,
    ) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _model_ref: Option<&str>,
        _has_explicit_model: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _var_name: &str) -> Option<String> {
        None
    }
}

#[async_trait]
impl agentic_automation::WorkspaceContext for FakePlatform {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        Some(std::path::Path::new(""))
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err(format!("fake platform: connector '{name}' unavailable"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("fake platform: integration '{name}' unavailable"))
    }
    async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _r: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("unused".into())
    }
}

fn monitor_input(name: &str, granularity: &str, cron_expr: &str) -> ScheduleInput {
    ScheduleInput {
        name: name.to_string(),
        target_kind: "monitor_scan".to_string(),
        // The monitor tick reads `variables.granularity` and never the ref.
        target_ref: "monitor".to_string(),
        question: None,
        variables: Some(serde_json::json!({ "granularity": granularity })),
        cron_expr: cron_expr.to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
    }
}

/// Runs seeded by the monitor tick are titled `Anomaly scan (<granularity>)`
/// and carry the schedule id, which is what makes them countable per test on a
/// shared container.
async fn monitor_run_count(db: &DatabaseConnection, schedule_id: &str) -> i64 {
    #[derive(sea_orm::FromQueryResult)]
    struct C {
        c: i64,
    }
    C::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*)::int8 AS c FROM agentic_runs WHERE schedule_id = $1",
        [schedule_id.into()],
    ))
    .one(db)
    .await
    .unwrap()
    .map(|r| r.c)
    .unwrap_or(0)
}

#[tokio::test]
async fn two_replicas_racing_the_same_due_row_produce_one_claim() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();
    let s = create_schedule(&db, ws, monitor_input("mon-conc", "day", "0 6 * * *"))
        .await
        .unwrap();
    force_due(&db, &s.id, 3600).await;

    let (db1, db2) = (db.clone(), db.clone());
    let (p1, p2): (Arc<dyn agentic_pipeline::platform::PlatformContext>, _) =
        (Arc::new(FakePlatform), Arc::new(FakePlatform));
    let (a, b) = tokio::join!(
        async move { tick_monitor_schedules(&db1, ws, p1).await },
        async move {
            tick_monitor_schedules(
                &db2,
                ws,
                p2 as Arc<dyn agentic_pipeline::platform::PlatformContext>,
            )
            .await
        },
    );

    assert_eq!(
        a + b,
        1,
        "the CAS on next_run_at must let exactly one replica fire (a={a}, b={b})"
    );
    assert_eq!(
        monitor_run_count(&db, &s.id).await,
        1,
        "and exactly one scan run is seeded"
    );
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert!(after.next_run_at > agentic_runtime::crud::now());
    assert!(after.last_fired_at.is_some());
}

#[tokio::test]
async fn a_missed_window_fires_once_and_resumes() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();
    let s = create_schedule(&db, ws, monitor_input("mon-misfire", "day", "0 6 * * *"))
        .await
        .unwrap();
    // Three days overdue: three missed daily slots.
    force_due(&db, &s.id, 3 * 24 * 3600).await;

    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = Arc::new(FakePlatform);
    assert_eq!(tick_monitor_schedules(&db, ws, platform.clone()).await, 1);

    assert_eq!(
        monitor_run_count(&db, &s.id).await,
        1,
        "a three-day gap collapses to a single catch-up scan, not three"
    );
    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert!(
        after.next_run_at > agentic_runtime::crud::now(),
        "next_run_at jumps to the next future slot, not the next missed one"
    );
    assert!(
        after.missed_runs >= 3,
        "the skipped occurrences are still counted, not silently dropped: {}",
        after.missed_runs
    );

    // An immediate second tick must not re-fire — the row is no longer due.
    assert_eq!(tick_monitor_schedules(&db, ws, platform).await, 0);
    assert_eq!(monitor_run_count(&db, &s.id).await, 1);
}

#[tokio::test]
async fn a_misconfigured_row_is_not_advanced() {
    let Some(db) = test_db().await else { return };
    let ws = uuid::Uuid::new_v4();
    let mut bad = monitor_input("mon-nogran", "day", "0 6 * * *");
    bad.variables = Some(serde_json::json!({}));
    let s = create_schedule(&db, ws, bad).await.unwrap();
    force_due(&db, &s.id, 3600).await;
    let due_before = get_schedule(&db, ws, &s.id).await.unwrap().next_run_at;

    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = Arc::new(FakePlatform);
    assert_eq!(tick_monitor_schedules(&db, ws, platform).await, 0);

    let after = get_schedule(&db, ws, &s.id).await.unwrap();
    assert_eq!(
        after.next_run_at, due_before,
        "a data error must leave the row due, so it stays visible on the next \
         tick instead of being silently skipped forward"
    );
    assert!(
        after.last_error.is_some(),
        "and the reason is recorded rather than only logged"
    );
    assert_eq!(monitor_run_count(&db, &s.id).await, 0);
}
