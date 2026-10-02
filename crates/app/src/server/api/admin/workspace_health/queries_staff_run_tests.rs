//! Staff's runs in a workspace are not the workspace's health.
//!
//! Two kinds of run in `agentic_runs` carry a customer's `workspace_id` and
//! are Oxy staff's work: a workspace preview's dry run, and a custom-app
//! check run queued in `staging` or a sandbox. A failure of either says
//! nothing about what production serves, so neither is in the run-liveness
//! numerator or denominator, nor in the dead-letter count. A production
//! app-function run — a schedule, a webhook, Run now — is the workspace's own
//! and is counted exactly as before.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{exclude_daemon_runs, gather_signals};
use crate::server::api::admin::workspace_health::evaluator::HealthThresholds;
use crate::server::test_support::{SKIP_MSG, test_db};

/// One failed root run of `source_type` in `ws`, carrying `metadata`. Returns
/// its id.
async fn failed_run(
    db: &DatabaseConnection,
    ws: Uuid,
    source_type: &str,
    metadata: Option<Value>,
) -> String {
    let run_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO agentic_runs \
            (id, workspace_id, question, task_status, source_type, metadata, attempt, \
             created_at, updated_at) \
         VALUES ($1, $2, '', 'failed', $3, $4, 0, now(), now())",
        [
            run_id.clone().into(),
            ws.into(),
            source_type.into(),
            metadata.into(),
        ],
    ))
    .await
    .expect("seed a run");
    run_id
}

/// A dead-lettered queue task for `run_id`: claimed its three times and given up on.
async fn dead_task(db: &DatabaseConnection, run_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO agentic_task_queue \
            (task_id, run_id, queue_status, spec, visibility_timeout_secs, claim_count, \
             max_claims, scope_owned, available_at, created_at, updated_at) \
         VALUES ($1, $2, 'dead', '{}'::jsonb, 60, 3, 3, false, now(), now(), now())",
        [Uuid::new_v4().to_string().into(), run_id.into()],
    ))
    .await
    .expect("seed a dead task");
}

/// A sandbox or staging check that fails is staff's failure. With only such
/// runs in the window the workspace reports no runs at all; a production
/// app-function run failing beside them is the one failure counted.
#[tokio::test]
async fn a_failed_check_run_outside_production_is_not_a_workspace_failure() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let thresholds = HealthThresholds::default();
    let ws = Uuid::new_v4();
    for environment in ["staging", "dev-a1", "dev-a1"] {
        let metadata = json!({ "trigger": "manual", "environment": environment });
        failed_run(&db, ws, "app_function", Some(metadata)).await;
    }

    let signals = gather_signals(&db, &thresholds, Some(ws)).await.unwrap();
    let counted = signals
        .iter()
        .find(|s| s.workspace_id == ws)
        .map(|s| (s.total_runs, s.failed_runs));
    assert!(
        matches!(counted, None | Some((0, 0))),
        "check runs outside production were counted as the workspace's: {counted:?}"
    );

    // Production's app-function runs are the workspace's own, stamped or not.
    failed_run(
        &db,
        ws,
        "app_function",
        Some(json!({ "trigger": "manual" })),
    )
    .await;
    let named = json!({ "trigger": "schedule", "environment": "production" });
    failed_run(&db, ws, "app_function", Some(named)).await;
    failed_run(&db, ws, "app_function", None).await;

    let signals = gather_signals(&db, &thresholds, Some(ws)).await.unwrap();
    let mine = signals.iter().find(|s| s.workspace_id == ws).unwrap();
    assert_eq!(
        (mine.total_runs, mine.failed_runs),
        (3, 3),
        "a failed production app-function run must still count"
    );
}

/// The Queue dimension too: a sandbox or staging check whose task was
/// dead-lettered is staff's dead job, not the workspace's. Production's dead
/// app-function task is the one counted.
#[tokio::test]
async fn a_dead_check_task_outside_production_is_not_the_workspaces_dead_letter() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let thresholds = HealthThresholds::default();
    let ws = Uuid::new_v4();
    for environment in ["staging", "dev-a1"] {
        let metadata = json!({ "trigger": "manual", "environment": environment });
        let check = failed_run(&db, ws, "app_function", Some(metadata)).await;
        dead_task(&db, &check).await;
    }

    let signals = gather_signals(&db, &thresholds, Some(ws)).await.unwrap();
    let dead = signals
        .iter()
        .find(|s| s.workspace_id == ws)
        .map(|s| s.dead_letter_count);
    assert!(
        matches!(dead, None | Some(0)),
        "a dead check task outside production was counted as the workspace's: {dead:?}"
    );

    let metadata = Some(json!({ "trigger": "manual" }));
    let production = failed_run(&db, ws, "app_function", metadata).await;
    dead_task(&db, &production).await;

    let signals = gather_signals(&db, &thresholds, Some(ws)).await.unwrap();
    let mine = signals.iter().find(|s| s.workspace_id == ws).unwrap();
    assert_eq!(
        mine.dead_letter_count, 1,
        "a dead production app-function task must still count"
    );
}

/// The exclusion reads the runtime's one statement of the rule, under the
/// alias the dead-letter join gives `agentic_runs`.
#[test]
fn the_exclusion_is_the_runtimes_customer_run_rule() {
    for alias in ["", "r."] {
        let sql = exclude_daemon_runs(alias);
        let rule = agentic_runtime::crud::customer_run_sql(alias);
        assert!(sql.ends_with(&format!(" AND {rule}")), "{alias:?}: {sql}");
    }
    assert!(
        exclude_daemon_runs("r.").contains("COALESCE(r.metadata->>'environment', 'production')"),
        "a check run outside production is not the workspace's run"
    );
}
