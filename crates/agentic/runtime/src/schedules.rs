//! The schedule rows the compile worker reconciles, and the preagg-cycle seed a
//! promote enqueues.
//!
//! Moved down from `agentic_pipeline::scheduler`, which re-exports every item
//! here at its old path, so `oxy-compile-runtime` can reconcile a workspace's
//! `health_eval` / `preagg_cycle` rows and nudge a promote's cycle without
//! depending on the facade (crate-tiers stage C). Everything here needs only
//! this crate's schedule entity, cron engine and run/queue CRUD. The tick, the
//! CRUD that the HTTP layer drives, and the seeds that start workflow, airway
//! and agent runs stay in the pipeline.

use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, Statement, TransactionTrait,
};
use serde::Deserialize;

use crate::cron::{next_occurrence_after, validate_cron};
use crate::lifecycle::crud::runs::insert_run_with_schedule;
use crate::lifecycle::entity::schedule;

/// Create/update payload. Deserialized straight from the HTTP body.
#[derive(Debug, Clone, Deserialize)]
pub struct ScheduleInput {
    pub name: String,
    /// `"workflow"` | `"airway"` | `"agent"`.
    pub target_kind: String,
    /// `workflow_ref` / `pipeline_ref` / `agent_id`, workspace-relative.
    pub target_ref: String,
    /// Free-text question — required when `target_kind = "agent"`,
    /// ignored otherwise. Stored on `agentic_schedules.question`.
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub variables: Option<serde_json::Value>,
    pub cron_expr: String,
    #[serde(default = "default_tz")]
    pub timezone: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_tz() -> String {
    "UTC".to_string()
}
fn default_enabled() -> bool {
    true
}

/// Validation error vs. an internal DB error — lets the HTTP layer map to
/// 400 vs 500 without string-sniffing.
#[derive(Debug)]
pub enum ScheduleError {
    Invalid(String),
    Db(DbErr),
    NotFound,
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduleError::Invalid(m) => write!(f, "{m}"),
            ScheduleError::Db(e) => write!(f, "db error: {e}"),
            ScheduleError::NotFound => write!(f, "schedule not found"),
        }
    }
}

impl From<DbErr> for ScheduleError {
    fn from(e: DbErr) -> Self {
        ScheduleError::Db(e)
    }
}

/// Validate a create/update payload. Public so the pipeline's update and
/// fire paths, which stay in `agentic_pipeline::scheduler`, share it.
pub fn validate_input(input: &ScheduleInput) -> Result<(), ScheduleError> {
    if input.name.trim().is_empty() {
        return Err(ScheduleError::Invalid("name must not be empty".into()));
    }
    if !matches!(
        input.target_kind.as_str(),
        "workflow"
            | "airway"
            | "agent"
            | "monitor_scan"
            | "health_eval"
            | "preagg_cycle"
            | "function"
    ) {
        return Err(ScheduleError::Invalid(format!(
            "target_kind must be 'workflow', 'airway', 'agent', 'monitor_scan', \
             'health_eval', 'preagg_cycle', or 'function', got {:?}",
            input.target_kind
        )));
    }
    if input.target_ref.trim().is_empty() {
        return Err(ScheduleError::Invalid(
            "target_ref must not be empty".into(),
        ));
    }
    if input.target_kind == "agent"
        && input
            .question
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
    {
        return Err(ScheduleError::Invalid(
            "question must not be empty for agent schedules".into(),
        ));
    }
    if input.cron_expr.starts_with(INTERVAL_PREFIX) {
        // Interval-sentinel cadence for per-workspace `health_eval` /
        // `preagg_cycle` rows — validated by `next_run_for`, not the cron
        // engine.
        return Ok(());
    }
    validate_cron(&input.cron_expr, &input.timezone).map_err(ScheduleError::Invalid)
}

/// Sentinel prefix stored in a schedule row's `cron_expr` for per-workspace
/// `health_eval` rows. The cadence is arbitrary (`health_check.interval` in
/// `config.yml`) and doesn't always map to a clean cron, so health rows encode
/// `@interval:<secs>` and `next_run_for` advances by that many seconds.
const INTERVAL_PREFIX: &str = "@interval:";

/// Encode a per-workspace health cadence as a schedule `cron_expr` sentinel.
pub fn health_interval_cron(interval: std::time::Duration) -> String {
    format!("{INTERVAL_PREFIX}{}", interval.as_secs())
}

/// Compute the next fire time for a `cron_expr`. Health rows carry an
/// `@interval:<secs>` sentinel (arbitrary cadence that isn't a clean cron) and
/// advance by that many seconds from `after`; every other expression uses the
/// cron engine unchanged. This is the single seam both the CRUD create/update
/// path and the tick use, so a sentinel cadence never reaches `validate_cron`
/// or `next_occurrence_after`.
pub fn next_after(
    cron_expr: &str,
    timezone: &str,
    after: chrono::DateTime<chrono::Utc>,
) -> Result<chrono::DateTime<chrono::FixedOffset>, String> {
    if let Some(rest) = cron_expr.strip_prefix(INTERVAL_PREFIX) {
        let secs: i64 = rest
            .parse()
            .map_err(|_| format!("invalid interval sentinel: {cron_expr:?}"))?;
        return Ok((after + chrono::Duration::seconds(secs)).fixed_offset());
    }
    next_occurrence_after(cron_expr, timezone, after).map(|n| n.fixed_offset())
}

pub async fn create_schedule(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    input: ScheduleInput,
) -> Result<schedule::Model, ScheduleError> {
    validate_input(&input)?;
    let now = crate::lifecycle::crud::now();
    let next = next_after(&input.cron_expr, &input.timezone, chrono::Utc::now())
        .map_err(ScheduleError::Invalid)?;
    // Only persist a question for agent schedules; automation / airway
    // rows ignore it. Trim before storing so accidental whitespace
    // doesn't slip through.
    let question = if input.target_kind == "agent" {
        input.question.as_deref().map(|q| q.trim().to_string())
    } else {
        None
    };
    let model = schedule::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        workspace_id: Set(workspace_id),
        project_id: Set(None),
        branch_id: Set(None),
        name: Set(input.name),
        target_kind: Set(input.target_kind),
        target_ref: Set(input.target_ref),
        question: Set(question),
        variables: Set(input.variables),
        cron_expr: Set(input.cron_expr),
        timezone: Set(input.timezone),
        enabled: Set(input.enabled),
        next_run_at: Set(next),
        last_fired_at: Set(None),
        last_run_id: Set(None),
        last_error: Set(None),
        missed_runs: Set(0),
        last_missed_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(schedule::Entity::insert(model)
        .exec_with_returning(db)
        .await?)
}

/// User-facing name for the per-workspace health-eval schedule row (shown in the
/// Schedules surface). Reconciled onto existing rows so a rename propagates on
/// the next compile/startup pass.
pub const HEALTH_SCHEDULE_NAME: &str = "Health check";

/// Idempotently reconcile a workspace's single `health_eval` schedule row to the
/// given cadence/enabled state. Creates the row when absent; on an existing row,
/// updates `cron_expr`/`enabled` (and recomputes `next_run_at`) only when the
/// cadence or enabled flag actually changed — an unchanged reconcile preserves
/// the next fire slot. Called from the compile worker (config.yml is the source
/// of truth), workspace-create, and startup reconcile. The row's `target_ref`
/// is the workspace id.
pub async fn reconcile_health_schedule(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    interval: std::time::Duration,
    enabled: bool,
) -> Result<(), ScheduleError> {
    let cron = health_interval_cron(interval);
    let existing = schedule::Entity::find()
        .filter(schedule::Column::TargetKind.eq("health_eval"))
        .filter(schedule::Column::TargetRef.eq(workspace_id.to_string()))
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await?;

    match existing {
        None => {
            create_schedule(
                db,
                workspace_id,
                ScheduleInput {
                    name: HEALTH_SCHEDULE_NAME.to_string(),
                    target_kind: "health_eval".to_string(),
                    target_ref: workspace_id.to_string(),
                    question: None,
                    variables: None,
                    cron_expr: cron,
                    timezone: "UTC".to_string(),
                    enabled,
                },
            )
            .await?;
        }
        Some(row)
            if row.cron_expr != cron
                || row.enabled != enabled
                || row.name != HEALTH_SCHEDULE_NAME =>
        {
            // Only recompute the fire slot when the cadence actually changed;
            // a rename / enable-toggle must not reset next_run_at.
            let cadence_changed = row.cron_expr != cron;
            let next = if cadence_changed {
                Some(
                    next_after(&cron, &row.timezone, chrono::Utc::now())
                        .map_err(ScheduleError::Invalid)?,
                )
            } else {
                None
            };
            let mut active: schedule::ActiveModel = row.into();
            active.cron_expr = Set(cron);
            active.enabled = Set(enabled);
            active.name = Set(HEALTH_SCHEDULE_NAME.to_string());
            if let Some(next) = next {
                active.next_run_at = Set(next);
            }
            active.updated_at = Set(crate::lifecycle::crud::now());
            active.update(db).await?;
        }
        Some(_) => {
            // Cadence, enabled, and name all unchanged — leave the row (and its
            // next fire slot) untouched. Steady-state path on every compile.
        }
    }
    Ok(())
}

// ── Pre-aggregation cycle scheduling ────────────────────────────────────────
//
// Mirrors the health-eval block above exactly, one layer down: build a
// rollup rather than evaluate one. Kept as separate functions rather than a
// generic "reconcile a singleton per-workspace schedule" abstraction — the
// health path is exercised by a full test suite already, and a shared
// abstraction is not worth the risk of a subtle regression there for a
// second caller with genuinely different payload shape (health carries only
// `force_smoke`; a preagg cycle carries an optional rollup target).

/// User-facing name for the per-workspace pre-aggregation schedule row.
pub const PREAGG_SCHEDULE_NAME: &str = "Pre-aggregation cycle";

/// Idempotently reconcile a workspace's single `preagg_cycle` schedule row to
/// the given cadence/enabled state. Same contract as
/// [`reconcile_health_schedule`]: creates the row when absent, updates it only
/// when cadence/enabled/name actually changed (preserving the next fire slot
/// otherwise), and leaves it alone on an unchanged reconcile. Called from the
/// compile worker (`config.yml` is the source of truth), and startup reconcile.
pub async fn reconcile_preagg_schedule(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    interval: std::time::Duration,
    enabled: bool,
) -> Result<(), ScheduleError> {
    let cron = health_interval_cron(interval);
    let existing = schedule::Entity::find()
        .filter(schedule::Column::TargetKind.eq("preagg_cycle"))
        .filter(schedule::Column::TargetRef.eq(workspace_id.to_string()))
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await?;

    match existing {
        None => {
            create_schedule(
                db,
                workspace_id,
                ScheduleInput {
                    name: PREAGG_SCHEDULE_NAME.to_string(),
                    target_kind: "preagg_cycle".to_string(),
                    target_ref: workspace_id.to_string(),
                    question: None,
                    variables: None,
                    cron_expr: cron,
                    timezone: "UTC".to_string(),
                    enabled,
                },
            )
            .await?;
        }
        Some(row)
            if row.cron_expr != cron
                || row.enabled != enabled
                || row.name != PREAGG_SCHEDULE_NAME =>
        {
            let cadence_changed = row.cron_expr != cron;
            let next = if cadence_changed {
                Some(
                    next_after(&cron, &row.timezone, chrono::Utc::now())
                        .map_err(ScheduleError::Invalid)?,
                )
            } else {
                None
            };
            let mut active: schedule::ActiveModel = row.into();
            active.cron_expr = Set(cron);
            active.enabled = Set(enabled);
            active.name = Set(PREAGG_SCHEDULE_NAME.to_string());
            if let Some(next) = next {
                active.next_run_at = Set(next);
            }
            active.updated_at = Set(crate::lifecycle::crud::now());
            active.update(db).await?;
        }
        Some(_) => {}
    }
    Ok(())
}

/// Seed a `TaskScope::Global` run carrying the per-workspace preagg-cycle
/// Custom task. The fleet's `PreaggTaskExecutor` (registered via the host's
/// `CustomTaskRegistry`) drains it, builds a fresh workspace context from
/// `workspace_id`, and rebuilds whatever rollups are stale (or, if `force`, all
/// of `target` — or every declared rollup when `target` is absent).
///
/// Root `task_id == run_id`, same requirement as every other Global seed — see
/// `start_health_eval_run`'s doc for why a different id makes the run invisible
/// to the pickup query.
pub async fn start_preagg_cycle_run(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    schedule_id: Option<&str>,
    force: bool,
    target: Option<(String, String)>,
    trigger: &str,
) -> Result<String, String> {
    use crate::lifecycle::crud::runs::insert_run;
    use crate::orchestrator::crud::queue::{TaskScope, enqueue_task};
    use agentic_core::delegation::TaskSpec;

    let run_id = uuid::Uuid::new_v4().to_string();
    // Stamp the trigger (`scheduled` for a cron fire, `manual` for a run-now or
    // a Rebuild click) so the run history labels a preagg run the same way it
    // labels workflow/airway/agent ones.
    let mut metadata = serde_json::json!({});
    stamp_trigger_metadata(&mut metadata, &Some(trigger.to_string()), &None, &None);
    // The run row and its queue row commit together: a run committed alone is
    // "never started" to a booting server's cleanup, which fails it.
    let txn = db.begin().await.map_err(|e| e.to_string())?;
    match schedule_id {
        Some(schedule_id) => insert_run_with_schedule(
            &txn,
            &run_id,
            PREAGG_SCHEDULE_NAME,
            None,
            "preagg_cycle",
            Some(metadata),
            schedule_id,
            workspace_id,
        )
        .await
        .map_err(|e| e.to_string())?,
        None => insert_run(
            &txn,
            &run_id,
            PREAGG_SCHEDULE_NAME,
            None,
            "preagg_cycle",
            Some(metadata),
            workspace_id,
        )
        .await
        .map_err(|e| e.to_string())?,
    }

    let target_json =
        target.map(|(view, rollup)| serde_json::json!({ "view": view, "rollup": rollup }));
    let spec = TaskSpec::Custom {
        kind: "preagg_cycle".into(),
        payload: serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "force": force,
            "target": target_json,
        }),
    };
    enqueue_task(&txn, &run_id, &run_id, None, &spec, None, TaskScope::Global)
        .await
        .map_err(|e| e.to_string())?;
    txn.commit().await.map_err(|e| e.to_string())?;
    Ok(run_id)
}

/// Enqueue the cycle a **promote** owes, at the moment the definitions
/// changed rather than up to a heartbeat later.
///
/// [`reconcile_preagg_schedule`] runs on every promoted compile but only ever
/// reconciles the *cadence* — it preserves `next_run_at` unless the cron
/// expression itself moved, which is right for a rename or an enable-toggle
/// and wrong for the one case that has new work: an edit that changes what a
/// rollup IS. The hash such an edit produces has no built artifact, so the
/// refresh keys already read it as stale; what was missing is a tick. With no
/// `heartbeat:` configured that tick is up to ten minutes away, and at the
/// configured ceiling up to twenty-four hours, with nothing anywhere saying a
/// build is pending.
///
/// **Not** `force`, unlike `agentic_pipeline::scheduler::enqueue_preagg_cycle`. Forcing is the operator
/// answering "is it stale" by hand; here the refresh keys answer it correctly
/// on their own, and forcing would additionally rebuild every rollup the
/// promote did not touch. Untargeted for the same reason the tick is: one
/// promote can re-hash rollups across several views.
///
/// `Ok(None)` means a cycle for this workspace is already **queued** and this
/// promote joined it. Unlike the tick — which cannot double-fire, because it
/// CAS-advances `next_run_at` — and unlike a Rebuild click, which an operator
/// expects to produce a run, this path fires on an event a person can trigger
/// repeatedly, so it needs its own guard or a rapid series of promotes stacks
/// a cycle each.
///
/// The guard counts a queued cycle only when it is **untargeted**, and
/// deliberately not one that is `claimed`. Both exclusions are the same
/// question — *will the row I am joining actually build my hash?* — and a row
/// fails it two ways. A `claimed` cycle has started and may have loaded its
/// views before this promote landed. A **targeted** one covers exactly the
/// rollup it names (`PreaggCycleRequest::covers`), so joining the Rebuild
/// button's row means a promote to some other view is silently dropped — the
/// exact defect this path exists to fix, logged as if it had been handled.
///
/// `force` needs no such exclusion: a forced untargeted cycle rebuilds every
/// declared rollup, the new hash included, so joining one is correct.
///
/// What is left is promote-during-a-running-cycle, which can queue a second —
/// the same shape the Rebuild button has always had. What bounds it is that
/// the next promote then joins that queued row rather than adding a third.
pub async fn enqueue_preagg_cycle_for_promote(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> Result<Option<String>, String> {
    if preagg_cycle_is_queued(db, workspace_id).await? {
        return Ok(None);
    }
    let schedule_id = preagg_schedule_id(db, workspace_id).await?;
    start_preagg_cycle_run(
        db,
        workspace_id,
        schedule_id.as_deref(),
        false,
        None,
        "promote",
    )
    .await
    .map(Some)
}

/// Whether an **untargeted** `preagg_cycle` task for this workspace is sitting
/// in the queue unclaimed — i.e. whether there is a cycle a promote can join
/// and still get its rollup built. See [`enqueue_preagg_cycle_for_promote`]
/// for why `claimed` and targeted rows deliberately do not count.
///
/// `->>` yields SQL `NULL` for a JSON `null` as well as an absent key, and
/// `start_preagg_cycle_run` writes `"target": null` for an untargeted cycle,
/// so `IS NULL` covers both spellings. The count rides the partial poll index
/// on `queue_status = 'queued'`.
async fn preagg_cycle_is_queued(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> Result<bool, String> {
    #[derive(sea_orm::FromQueryResult)]
    struct Queued {
        queued: i64,
    }
    let row = Queued::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*)::int8 AS queued FROM agentic_task_queue \
         WHERE queue_status = 'queued' \
           AND spec->>'kind' = 'preagg_cycle' \
           AND spec->'payload'->>'workspace_id' = $1 \
           AND spec->'payload'->>'target' IS NULL",
        [workspace_id.to_string().into()],
    ))
    .one(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(row.is_some_and(|r| r.queued > 0))
}

/// The workspace's `preagg_cycle` schedule row id, when it has one. `None`
/// before the first compile, or when pre-aggregation is disabled entirely —
/// see `agentic_pipeline::scheduler::enqueue_preagg_cycle` on why an unattributed run is still correct.
pub async fn preagg_schedule_id(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> Result<Option<String>, String> {
    Ok(schedule::Entity::find()
        .filter(schedule::Column::TargetKind.eq("preagg_cycle"))
        .filter(schedule::Column::TargetRef.eq(workspace_id.to_string()))
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .map(|r| r.id))
}

/// Merge run-provenance fields (trigger source, logical date, retry-of)
/// into a run's `metadata` JSONB.
///
/// Called from both seed paths so the same convention applies whether the
/// run was scheduled, manually triggered, backfilled, or retried.
/// `metadata` is mutated in place — the surrounding seed builds the rest
/// of the JSON object first; this just stamps the well-known keys when
/// each field is set.
pub fn stamp_trigger_metadata(
    metadata: &mut serde_json::Value,
    trigger: &Option<String>,
    logical_date: &Option<chrono::DateTime<chrono::Utc>>,
    retry_of: &Option<String>,
) {
    let serde_json::Value::Object(map) = metadata else {
        return;
    };
    if let Some(t) = trigger {
        map.insert("trigger".to_string(), serde_json::Value::String(t.clone()));
    }
    if let Some(ld) = logical_date {
        map.insert(
            "logical_date".to_string(),
            serde_json::Value::String(ld.to_rfc3339()),
        );
    }
    if let Some(r) = retry_of {
        map.insert("retry_of".to_string(), serde_json::Value::String(r.clone()));
    }
}
