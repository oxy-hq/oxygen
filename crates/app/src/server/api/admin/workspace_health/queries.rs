use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement};
use std::collections::HashMap;
use uuid::Uuid;

use super::evaluator::{HealthThresholds, WorkspaceSignals};

/// Run `source_type`s the health signal must ignore: platform daemon runs the
/// workspace never asked for.
///
/// `health_eval_workspace` above all — those rows are seeded by *this very
/// health check* (`start_health_eval_run` inserts one `agentic_runs` row per
/// workspace per `health_check.interval`, default 1h). Counting them makes the
/// check grade itself: an idle workspace's entire denominator is health runs, so
/// a broken eval task reads as "23/24 runs failed (96%)" on every tenant
/// simultaneously — including empty demo/test orgs that ran nothing at all.
///
/// The test is *whose failure it is*, not whether a human pressed a button.
/// A `health_eval_workspace` or `preagg_cycle` failure is a platform problem —
/// it says nothing about the tenant, and both fire on a timer, so they also
/// reach workspaces with no content at all. Other auto-generated runs are
/// deliberately **kept** counted, because their failure *is* a workspace fault
/// and they only exist where the workspace created the work:
/// - `compile` (auto/lazy compile on workspace-context load) fails when the
///   tenant's own `config.yml` / YAML is broken — exactly what an unhealthy
///   workspace looks like.
/// - `monitor_scan` only runs if the workspace defined a `.monitor.yml`, and
///   fails on that monitor's definition or warehouse.
///
/// Deliberately kept local instead of folded into `agentic_runtime`'s
/// `SYSTEM_SOURCE_TYPES`: that list also drives the coordinator run feed and the
/// per-job "Recent runs" history, where scheduled health fires are meant to stay
/// visible (see `start_health_eval_run`'s `schedule_id` stamping). A new daemon
/// `source_type` therefore has to be weighed against **both** lists.
///
/// `preview_analyze` is staff checking a *branch* (workspace previews),
/// `preview_schema_drop` the TTL drop of a preview's schemas, and
/// `preview_compare` a preview build's compare with live, `preview_airway_sample`
/// staff sampling a branch's pipeline: no such failure says anything about the
/// workspace production serves. Nor does `custom_app_staging_migrations`, a
/// custom app's staging homes migrated after a publish, nor a sandbox's
/// (`custom_app_sandbox_migrations`) or its teardown
/// (`custom_app_sandbox_teardown`): staff trying a build, not the tenant's work.
const NON_WORKSPACE_RUN_SOURCES: &[&str] = &[
    "health_eval_workspace",
    "preagg_cycle",
    "preview_analyze",
    "preview_schema_drop",
    "preview_compare",
    "preview_airway_sample",
    "custom_app_staging_migrations",
    "custom_app_sandbox_migrations",
    "custom_app_sandbox_teardown",
];

/// SQL predicate excluding [`NON_WORKSPACE_RUN_SOURCES`], for a table aliased
/// `alias` (empty for an unaliased `agentic_runs`).
///
/// Rows with a NULL `source_type` are explicitly kept: a bare `NOT IN` evaluates
/// to NULL for them, which the WHERE clause treats as false and would silently
/// drop legacy runs out of the denominator.
///
/// Two kinds of staff run cannot be told apart by `source_type` — a held
/// preview procedure run is an ordinary `workflow` run, and a custom-app check
/// run queued in `staging` or a sandbox an ordinary `app_function` run — so
/// they are told apart by what their seeder stamped on `metadata`. A staffer's
/// dry run of a branch, or check of an unpromoted build, failing says nothing
/// about the workspace production serves. That rule is not restated here: it
/// is `agentic_runtime`'s `customer_run_sql`, the predicate the coordinator
/// run feed hides the same runs with, so the two cannot drift. A production
/// `app_function` run carries no environment and is counted as before.
///
/// `agentic_runs` has no supporting index at all for the `created_at` window
/// every caller here filters on (PR #3381 review nit 3), so every one of
/// these queries is already an unbounded scan *before* this predicate — a
/// pre-existing condition, out of this predicate's scope to fix. Against
/// that baseline, the `metadata` predicate is a cheap residual: two more
/// per-row JSONB reads on a scan that was already reading every row in the
/// window. It stops being cheap the day
/// `agentic_runs` gets a `created_at` index and this predicate is the thing
/// keeping it non-sargable — see the follow-up in
/// `internal-docs/workspace-previews.md`.
fn exclude_daemon_runs(alias: &str) -> String {
    let col = format!("{alias}source_type");
    let list = NON_WORKSPACE_RUN_SOURCES
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        " AND ({col} IS NULL OR {col} NOT IN ({list})) AND {}",
        agentic_runtime::crud::customer_run_sql(alias)
    )
}

#[derive(Debug, FromQueryResult)]
struct RunAggRow {
    workspace_id: Uuid,
    total_runs: i64,
    failed_runs: i64,
    timed_out_runs: i64,
}

#[derive(Debug, FromQueryResult)]
struct AnomalyAggRow {
    workspace_id: Uuid,
    open_high: i64,
    open_medium: i64,
}

#[derive(Debug, FromQueryResult)]
struct DeadLetterRow {
    workspace_id: Uuid,
    dead_letter_count: i64,
}

/// Gather per-workspace signal counts across all tenants in one pass.
///
/// Each sub-query is grouped by `workspace_id`; results are merged in memory.
/// Only workspaces with ANY activity in the window, an open anomaly, or a
/// dead-letter entry appear in the output.
pub async fn gather_signals(
    db: &DatabaseConnection,
    t: &HealthThresholds,
    workspace_id: Option<Uuid>,
) -> Result<Vec<WorkspaceSignals>, DbErr> {
    let mut map: HashMap<Uuid, WorkspaceSignals> = HashMap::new();

    // 1. Run liveness — failed / timed_out / total over the window.
    //
    // `task_status` is nullable in the schema; NULL rows are excluded from the
    // FILTER aggregates naturally (SQL ignores NULLs in COUNT FILTER).
    //
    // Daemon runs are excluded from both numerator and denominator — most
    // importantly this check's own `health_eval_workspace` runs, which would
    // otherwise make every idle workspace's failure rate a measurement of the
    // health checker rather than of the workspace.
    //
    // make_interval's `hours` param is int4; bind i32 so Postgres doesn't see a
    // bigint (which fails named-arg function resolution). An optional
    // workspace_id filter binds $2 when scoping to a single workspace.
    let mut run_sql = String::from(
        "SELECT workspace_id, \
                COUNT(*)::bigint AS total_runs, \
                COUNT(*) FILTER (WHERE task_status = 'failed')::bigint AS failed_runs, \
                COUNT(*) FILTER (WHERE task_status = 'timed_out')::bigint AS timed_out_runs \
         FROM agentic_runs \
         WHERE created_at > now() - make_interval(hours => $1)",
    );
    run_sql.push_str(&exclude_daemon_runs(""));
    let mut run_params: Vec<sea_orm::Value> = vec![(t.window_hours as i32).into()];
    if let Some(ws) = workspace_id {
        run_sql.push_str(" AND workspace_id = $2");
        run_params.push(ws.into());
    }
    run_sql.push_str(" GROUP BY workspace_id");
    let run_rows = RunAggRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        run_sql,
        run_params,
    ))
    .all(db)
    .await?;
    for r in run_rows {
        ensure_entry(&mut map, r.workspace_id);
        let s = map.get_mut(&r.workspace_id).unwrap();
        s.total_runs = r.total_runs;
        s.failed_runs = r.failed_runs;
        s.timed_out_runs = r.timed_out_runs;
    }

    // 2. Open anomalies by severity (status = 'new'). The optional workspace_id
    // filter is the only bound param here, so it binds $1.
    let mut anomaly_sql = String::from(
        "SELECT workspace_id, \
                COUNT(*) FILTER (WHERE severity = 'high')::bigint AS open_high, \
                COUNT(*) FILTER (WHERE severity = 'medium')::bigint AS open_medium \
         FROM metric_anomalies \
         WHERE status = 'new'",
    );
    let mut anomaly_params: Vec<sea_orm::Value> = Vec::new();
    if let Some(ws) = workspace_id {
        anomaly_sql.push_str(" AND workspace_id = $1");
        anomaly_params.push(ws.into());
    }
    anomaly_sql.push_str(" GROUP BY workspace_id");
    let anomaly_rows = AnomalyAggRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        anomaly_sql,
        anomaly_params,
    ))
    .all(db)
    .await?;
    for r in anomaly_rows {
        ensure_entry(&mut map, r.workspace_id);
        let s = map.get_mut(&r.workspace_id).unwrap();
        s.open_high_anomalies = r.open_high;
        s.open_medium_anomalies = r.open_medium;
    }

    // 3. Dead-letter queue entries (queue_status = 'dead').
    //
    // `agentic_task_queue` has no `workspace_id` column; workspace attribution
    // is obtained by joining to `agentic_runs` via the `run_id` FK.
    // The INNER JOIN is safe: `run_id` is NOT NULL with an ON DELETE CASCADE FK,
    // so every dead task has a live run row. Caveat: an orphaned dead task (only
    // possible if a run row is deleted via direct SQL bypassing CASCADE) would be
    // undercounted.
    // Daemon runs are excluded here too: a dead-lettered health-eval task would
    // otherwise flag the Queue dimension unhealthy on every workspace at once,
    // the same self-grading loop as the job-liveness signal above.
    let mut dl_sql = String::from(
        "SELECT r.workspace_id, COUNT(*)::bigint AS dead_letter_count \
         FROM agentic_task_queue q \
         JOIN agentic_runs r ON r.id = q.run_id \
         WHERE q.queue_status = 'dead'",
    );
    dl_sql.push_str(&exclude_daemon_runs("r."));
    let mut dl_params: Vec<sea_orm::Value> = Vec::new();
    if let Some(ws) = workspace_id {
        dl_sql.push_str(" AND r.workspace_id = $1");
        dl_params.push(ws.into());
    }
    dl_sql.push_str(" GROUP BY r.workspace_id");
    let dl_rows = DeadLetterRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        dl_sql,
        dl_params,
    ))
    .all(db)
    .await?;
    for r in dl_rows {
        ensure_entry(&mut map, r.workspace_id);
        map.get_mut(&r.workspace_id).unwrap().dead_letter_count = r.dead_letter_count;
    }

    // 4. Airway pipeline status on most-recent run per workspace.
    apply_airway_status(db, t, &mut map, workspace_id).await?;

    Ok(map.into_values().collect())
}

/// Mark the Airway flags from each workspace's most recent Airway run in window.
///
/// Airway runs carry `source_type = 'airway'` (set by the HTTP handler in
/// `crates/agentic/http/src/routes/airway.rs` before inserting the run row).
async fn apply_airway_status(
    db: &DatabaseConnection,
    t: &HealthThresholds,
    map: &mut HashMap<Uuid, WorkspaceSignals>,
    workspace_id: Option<Uuid>,
) -> Result<(), DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct AirwayRow {
        workspace_id: Uuid,
        task_status: String,
    }
    // DISTINCT ON returns the latest Airway run per workspace within the window.
    // Window binds $1; the optional workspace_id filter binds $2, before ORDER BY.
    let mut sql = String::from(
        "SELECT DISTINCT ON (workspace_id) workspace_id, \
                COALESCE(task_status, '') AS task_status \
         FROM agentic_runs \
         WHERE source_type = 'airway' \
           AND created_at > now() - make_interval(hours => $1)",
    );
    let mut params: Vec<sea_orm::Value> = vec![(t.window_hours as i32).into()];
    if let Some(ws) = workspace_id {
        sql.push_str(" AND workspace_id = $2");
        params.push(ws.into());
    }
    sql.push_str(" ORDER BY workspace_id, created_at DESC");
    let rows = AirwayRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        params,
    ))
    .all(db)
    .await?;
    for r in rows {
        map.entry(r.workspace_id)
            .or_insert_with(|| blank_signals(r.workspace_id));
        let s = map.get_mut(&r.workspace_id).unwrap();
        s.airway_last_run_failed = r.task_status == "failed" || r.task_status == "timed_out";
        s.airway_completed_with_errors = r.task_status == "completed_with_errors";
    }
    Ok(())
}

/// Human-readable label for a workspace: its own name plus the owning org name
/// (org is nullable). Used to render Slack alerts as "name (org)" instead of a
/// bare UUID.
#[derive(Debug, Clone)]
pub struct WorkspaceLabel {
    pub name: String,
    pub org_name: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct WorkspaceLabelRow {
    id: Uuid,
    name: String,
    org_name: Option<String>,
}

/// Fetch display labels (workspace name + org name) for every workspace, keyed
/// by id. A LEFT JOIN keeps workspaces whose `org_id` is NULL. Cheap enough to
/// fetch unfiltered — the alert path only looks up the few ids it needs.
pub async fn gather_workspace_labels(
    db: &DatabaseConnection,
) -> Result<HashMap<Uuid, WorkspaceLabel>, DbErr> {
    let rows = WorkspaceLabelRow::find_by_statement(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT w.id AS id, w.name AS name, o.name AS org_name \
         FROM workspaces w \
         LEFT JOIN organizations o ON o.id = w.org_id"
            .to_string(),
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.id,
                WorkspaceLabel {
                    name: r.name,
                    org_name: r.org_name,
                },
            )
        })
        .collect())
}

fn ensure_entry(map: &mut HashMap<Uuid, WorkspaceSignals>, ws: Uuid) {
    map.entry(ws).or_insert_with(|| blank_signals(ws));
}

fn blank_signals(workspace_id: Uuid) -> WorkspaceSignals {
    WorkspaceSignals::empty(workspace_id)
}

#[cfg(test)]
#[path = "queries_staff_run_tests.rs"]
mod staff_run_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_support::{SKIP_MSG, test_db};
    use sea_orm::ConnectionTrait;

    #[tokio::test]
    async fn gathers_failed_runs_per_workspace() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = Uuid::new_v4();
        // Seed three failed runs for `ws` in-window.
        // `agentic_runs.id` is a String PK (not UUID); `question`, `attempt`,
        // `created_at`, and `updated_at` are NOT NULL.
        for _ in 0..3 {
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO agentic_runs \
                    (id, workspace_id, question, task_status, attempt, created_at, updated_at) \
                 VALUES ($1, $2, '', 'failed', 0, now(), now())",
                [Uuid::new_v4().to_string().into(), ws.into()],
            ))
            .await
            .unwrap();
        }
        let signals = gather_signals(&db, &HealthThresholds::default(), None)
            .await
            .unwrap();
        let mine = signals.iter().find(|s| s.workspace_id == ws).unwrap();
        assert_eq!(mine.failed_runs, 3);
        assert_eq!(mine.total_runs, 3);
    }

    /// The health check must never count its own eval runs. Regression test for
    /// the alert storm where every workspace — including empty demo orgs — read
    /// "23/24 runs failed (96%)": the hourly `health_eval_workspace` run rows are
    /// the entire denominator for an idle workspace, so a failing eval task made
    /// the checker report on itself, once per tenant.
    #[tokio::test]
    async fn health_eval_runs_are_not_counted_as_workspace_runs() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = Uuid::new_v4();
        // 24 failed health-eval runs — one full day at the default 1h cadence —
        // plus one real failed workspace run.
        for _ in 0..24 {
            insert_run(&db, ws, "failed", Some("health_eval_workspace")).await;
        }
        insert_run(&db, ws, "failed", Some("analytics")).await;

        let signals = gather_signals(&db, &HealthThresholds::default(), Some(ws))
            .await
            .unwrap();
        let mine = signals.iter().find(|s| s.workspace_id == ws).unwrap();
        assert_eq!(
            mine.total_runs, 1,
            "health-eval runs must be excluded from the denominator"
        );
        assert_eq!(
            mine.failed_runs, 1,
            "health-eval runs must be excluded from the numerator"
        );
    }

    /// A sandbox's teardown and its migrations are staff trying a build, filed
    /// in the app's workspace: failing, they say nothing about the workspace
    /// production serves, and must not move its failure rate.
    #[tokio::test]
    async fn sandbox_task_runs_are_not_counted_as_workspace_runs() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = Uuid::new_v4();
        for kind in [
            "custom_app_sandbox_teardown",
            "custom_app_sandbox_migrations",
        ] {
            insert_run(&db, ws, "failed", Some(kind)).await;
        }
        insert_run(&db, ws, "failed", Some("analytics")).await;

        let signals = gather_signals(&db, &HealthThresholds::default(), Some(ws))
            .await
            .unwrap();
        let mine = signals.iter().find(|s| s.workspace_id == ws).unwrap();
        assert_eq!((mine.total_runs, mine.failed_runs), (1, 1));
    }

    /// A NULL `source_type` is workspace work of unknown provenance, not a
    /// daemon run — a bare `NOT IN` would evaluate NULL and silently drop it.
    #[tokio::test]
    async fn null_source_type_runs_are_still_counted() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = Uuid::new_v4();
        insert_run(&db, ws, "failed", None).await;

        let signals = gather_signals(&db, &HealthThresholds::default(), Some(ws))
            .await
            .unwrap();
        let mine = signals.iter().find(|s| s.workspace_id == ws).unwrap();
        assert_eq!(mine.total_runs, 1);
        assert_eq!(mine.failed_runs, 1);
    }

    /// `compile` and `monitor_scan` rows are auto-generated too, but their
    /// failures are the workspace's own (a broken `config.yml`, a broken
    /// `.monitor.yml`) and they only exist where the workspace created the work.
    /// Locks in that the exclusion stayed narrow — see
    /// [`NON_WORKSPACE_RUN_SOURCES`].
    #[tokio::test]
    async fn workspace_attributable_daemon_runs_stay_counted() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = Uuid::new_v4();
        insert_run(&db, ws, "failed", Some("compile")).await;
        insert_run(&db, ws, "failed", Some("monitor_scan")).await;
        insert_run(&db, ws, "failed", Some("health_eval_workspace")).await;

        let signals = gather_signals(&db, &HealthThresholds::default(), Some(ws))
            .await
            .unwrap();
        let mine = signals.iter().find(|s| s.workspace_id == ws).unwrap();
        assert_eq!(
            mine.total_runs, 2,
            "compile and monitor_scan are workspace faults and must stay counted"
        );
        assert_eq!(mine.failed_runs, 2);
    }

    /// Seed one `agentic_runs` row. `source_type` is nullable, so `None` writes
    /// a genuine SQL NULL.
    async fn insert_run(
        db: &DatabaseConnection,
        ws: Uuid,
        task_status: &str,
        source_type: Option<&str>,
    ) {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO agentic_runs \
                (id, workspace_id, question, task_status, source_type, attempt, created_at, updated_at) \
             VALUES ($1, $2, '', $3, $4, 0, now(), now())",
            [
                Uuid::new_v4().to_string().into(),
                ws.into(),
                task_status.into(),
                source_type.map(str::to_string).into(),
            ],
        ))
        .await
        .unwrap();
    }

    #[test]
    fn daemon_exclusion_keeps_null_source_types() {
        let sql = exclude_daemon_runs("");
        assert!(sql.contains("source_type IS NULL"), "got: {sql}");
        assert!(sql.contains("'health_eval_workspace'"), "got: {sql}");
    }

    #[test]
    fn daemon_exclusion_qualifies_aliased_columns() {
        let sql = exclude_daemon_runs("r.");
        assert!(sql.contains("r.source_type IS NULL"), "got: {sql}");
        assert!(sql.contains("r.source_type NOT IN"), "got: {sql}");
        assert!(
            sql.contains("r.metadata->>'trigger' IS DISTINCT FROM 'preview'"),
            "a preview dry run is not the workspace's run: {sql}"
        );
    }

    #[tokio::test]
    async fn scoped_gather_returns_only_target_workspace() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let target = Uuid::new_v4();
        let other = Uuid::new_v4();
        for ws in [target, other] {
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO agentic_runs \
                    (id, workspace_id, question, task_status, attempt, created_at, updated_at) \
                 VALUES ($1, $2, '', 'failed', 0, now(), now())",
                [Uuid::new_v4().to_string().into(), ws.into()],
            ))
            .await
            .unwrap();
        }
        let signals = gather_signals(&db, &HealthThresholds::default(), Some(target))
            .await
            .unwrap();
        assert!(signals.iter().all(|s| s.workspace_id == target));
        assert!(signals.iter().any(|s| s.workspace_id == target));
    }
}
