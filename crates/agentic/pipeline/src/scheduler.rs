//! Phase 2 scheduler: the periodic tick + schedule CRUD facade.
//!
//! Composition only — no execution logic. The tick reads due
//! `agentic_schedules` rows, CAS-advances `next_run_at`, and fires via the
//! existing seed fns with `TaskScope::Global` so the Phase-1 standalone
//! consumer drives them.
//!
//! - Exactly-once across replicas without leader election: the advance is
//!   a conditional UPDATE on the observed `next_run_at` (mirrors the
//!   `run_sequences` / driver-lease CAS idiom). Only the replica whose
//!   UPDATE affects one row fires the schedule.
//! - Misfire = run-once-then-resume: `next_run_at` is recomputed *strictly
//!   after now* ([`agentic_runtime::cron::next_occurrence_after`]), so
//!   missed slots during an outage collapse to one catch-up run.

use std::sync::Arc;

use agentic_core::hub_task::spawn_with_hub;
use agentic_runtime::cron::{
    count_occurrences_between, next_occurrence_after, occurrences_between, validate_cron,
};
use agentic_runtime::entity::schedule;
use agentic_runtime::lifecycle::crud::runs::{
    insert_run_with_schedule, update_run_done, update_run_failed,
};
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, Statement,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::agent_run::{StartAgentRequest, start_agent_run};
use crate::airway_run::{StartAirwayRequest, start_airway_run};
use crate::automation_run::{StartAutomationRequest, start_automation_run};
use crate::platform::PlatformContext;

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

fn validate_input(input: &ScheduleInput) -> Result<(), ScheduleError> {
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

/// Max `health_eval` rows a single [`tick_health_schedules`] pass will fire.
/// Bounds the enqueue burst after a long outage: when every workspace's
/// `next_run_at` has elapsed together, the oldest-due rows fire first
/// (`ORDER BY next_run_at ASC`) and the backlog drains over successive ticks
/// instead of one N-wide spike. Comfortably above the steady-state per-tick due
/// count (tick interval ≪ health interval, so only a small fraction is due each
/// pass), so it never throttles normal operation.
const MAX_HEALTH_FIRES_PER_TICK: u64 = 256;

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

/// Compute the next fire time for a schedule row. Convenience wrapper over
/// [`next_after`] for the tick, which already holds the row.
pub fn next_run_for(
    s: &schedule::Model,
    after: chrono::DateTime<chrono::Utc>,
) -> Result<chrono::DateTime<chrono::FixedOffset>, String> {
    next_after(&s.cron_expr, &s.timezone, after)
}

// ── CRUD (workspace-scoped) ─────────────────────────────────────────────────
//
// All CRUD operations are scoped by `workspace_id` (§12 FU4b): list filters,
// get/update/delete return `NotFound` on cross-workspace access, create
// stamps the row. The handler supplies `workspace_id` from the
// `/{workspace_id}/...` path; clients never set it via the body.

pub async fn list_schedules(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> Result<Vec<schedule::Model>, ScheduleError> {
    Ok(schedule::Entity::find()
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .all(db)
        .await?)
}

pub async fn get_schedule(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    id: &str,
) -> Result<schedule::Model, ScheduleError> {
    schedule::Entity::find_by_id(id.to_string())
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await?
        .ok_or(ScheduleError::NotFound)
}

pub async fn create_schedule(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    input: ScheduleInput,
) -> Result<schedule::Model, ScheduleError> {
    validate_input(&input)?;
    let now = agentic_runtime::crud::now();
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

pub async fn update_schedule(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    id: &str,
    input: ScheduleInput,
) -> Result<schedule::Model, ScheduleError> {
    validate_input(&input)?;
    // Cross-workspace updates surface as NotFound (not Forbidden), so the
    // existence of a schedule in another workspace is not probeable.
    let existing = get_schedule(db, workspace_id, id).await?;
    let cadence_changed =
        existing.cron_expr != input.cron_expr || existing.timezone != input.timezone;
    let next = if cadence_changed {
        next_after(&input.cron_expr, &input.timezone, chrono::Utc::now())
            .map_err(ScheduleError::Invalid)?
    } else {
        existing.next_run_at
    };
    let question = if input.target_kind == "agent" {
        input.question.as_deref().map(|q| q.trim().to_string())
    } else {
        None
    };
    let model = schedule::ActiveModel {
        id: Set(existing.id),
        workspace_id: Set(existing.workspace_id),
        project_id: Set(existing.project_id),
        branch_id: Set(existing.branch_id),
        name: Set(input.name),
        target_kind: Set(input.target_kind),
        target_ref: Set(input.target_ref),
        question: Set(question),
        variables: Set(input.variables),
        cron_expr: Set(input.cron_expr),
        timezone: Set(input.timezone),
        enabled: Set(input.enabled),
        next_run_at: Set(next),
        last_fired_at: Set(existing.last_fired_at),
        last_run_id: Set(existing.last_run_id),
        last_error: Set(existing.last_error),
        // Carry the audit fields across — an edit isn't a reset, and
        // dropping them would erase visible "you missed N runs" state.
        missed_runs: Set(existing.missed_runs),
        last_missed_at: Set(existing.last_missed_at),
        created_at: Set(existing.created_at),
        updated_at: Set(agentic_runtime::crud::now()),
    };
    Ok(schedule::Entity::update(model).exec(db).await?)
}

pub async fn delete_schedule(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    id: &str,
) -> Result<(), ScheduleError> {
    let res = schedule::Entity::delete_many()
        .filter(schedule::Column::Id.eq(id.to_string()))
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .exec(db)
        .await?;
    if res.rows_affected == 0 {
        return Err(ScheduleError::NotFound);
    }
    Ok(())
}

/// Delete every schedule row belonging to a workspace. Called when a workspace
/// is deleted: schedules carry a plain `workspace_id` column with no FK, so the
/// database will not clean them up. An orphaned `health_eval` row in particular
/// keeps being selected by [`tick_health_schedules`] and enqueues health-eval
/// tasks for a workspace that no longer exists, piling up in the dead-letter
/// queue. Returns the number of rows removed.
pub async fn delete_workspace_schedules(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> Result<u64, ScheduleError> {
    let res = schedule::Entity::delete_many()
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

/// Fire a schedule out-of-band, now, without touching `next_run_at` (the
/// recurring cadence is unaffected). Returns the seeded run id.
pub async fn run_schedule_now(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    workspace: &dyn crate::WorkflowWorkspaceContext,
    id: &str,
) -> Result<String, ScheduleError> {
    let s = get_schedule(db, workspace_id, id).await?;
    // `manual` distinguishes operator-fired runs from `scheduled` (the tick)
    // and `backfill` (out-of-band replay) in the run log.
    match fire_schedule(db, workspace, &s, "manual").await {
        Ok(FireOutcome::Seeded(run_id)) => {
            record_fire_success(db, &s.id, &run_id).await;
            Ok(run_id)
        }
        // Operator pressed "Run now" while a load is already in flight. Not an
        // error and not a silent no-op: hand back the in-flight run so the UI
        // navigates to it. `record_fire_success` is deliberately skipped — no
        // new run was seeded, and stamping one would misreport the fire count.
        Ok(FireOutcome::SkippedAlreadyRunning(run_id)) => {
            tracing::info!(
                target: "scheduler",
                schedule_id = %s.id,
                run_id = %run_id,
                "run-now collapsed into the in-flight run (single-flight)"
            );
            Ok(run_id)
        }
        Err(e) => {
            set_schedule_last_error(db, &s.id, Some(&e)).await;
            Err(ScheduleError::Invalid(e))
        }
    }
}

/// Seed + enqueue a one-off background run of a custom-app Oxy Function,
/// outside any schedule — the "run this function as a job now" path (manual or
/// API trigger). Mirrors the scheduled `"function"` fire arm but carries no
/// `schedule_id`. Entity/oxy-free: the host resolves the app + its retry policy
/// (from the manifest) and passes plain values here. Returns the seeded
/// `run_id`, which the caller watches over the coordinator SSE like any run.
#[allow(clippy::too_many_arguments)]
pub async fn enqueue_app_function_job(
    db: &DatabaseConnection,
    app_id: &str,
    function_name: &str,
    workspace_id: uuid::Uuid,
    policy: Option<agentic_core::delegation::TaskPolicy>,
    trigger: &str,
    // Optional input params (JSON) handed to the isolate as its request body.
    // `None` → empty body. Carried on the task so the worker replays it.
    input: Option<serde_json::Value>,
    // W3C `traceparent` of the caller's span, when the enqueue happens inside
    // a traced request (a run-now click). The executor links the worker's run
    // to it — a *link*, not a parent, because the run is caused by the
    // request but is not part of its latency. `None` from a cron tick.
    traceparent: Option<String>,
) -> Result<String, ScheduleError> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let mut metadata = serde_json::json!({});
    stamp_trigger_metadata(&mut metadata, &Some(trigger.to_string()), &None, &None);
    agentic_runtime::crud::insert_run(
        db,
        &run_id,
        &format!("fn:{app_id}/{function_name}"),
        None,
        "app_function",
        Some(metadata),
        workspace_id,
    )
    .await
    .map_err(ScheduleError::Db)?;
    let spec = agentic_core::delegation::TaskSpec::Custom {
        kind: "app_function".into(),
        // Carry the trigger so the executor records the invocation `mode` to
        // match (`manual` here) — the invocation history then agrees with the
        // run's stamped `metadata.trigger` — and the input params (if any) so the
        // worker replays them as the function's request body.
        payload: serde_json::json!({
            "app_id": app_id,
            "function_name": function_name,
            "trigger": trigger,
            "input": input,
            "traceparent": traceparent,
        }),
    };
    agentic_runtime::crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &spec,
        policy.as_ref(),
        agentic_runtime::crud::TaskScope::Global,
    )
    .await
    .map_err(ScheduleError::Db)?;
    Ok(run_id)
}

// ── Tick ────────────────────────────────────────────────────────────────────

/// Run one scheduler pass for a single workspace. Returns the number of
/// schedules fired by *this* replica. Never errors out the caller —
/// per-schedule failures are logged and skipped so one bad row can't stall
/// the loop.
///
/// Scoped to `workspace_id` (§12 FU4b): airway targets resolve their
/// pipeline file on the supplied workspace's filesystem, so each
/// workspace's tick must be invoked with its own matching context.
pub async fn tick_schedules(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    workspace: &dyn crate::WorkflowWorkspaceContext,
) -> usize {
    let now = chrono::Utc::now().fixed_offset();
    let due = match schedule::Entity::find()
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .filter(schedule::Column::Enabled.eq(true))
        .filter(schedule::Column::NextRunAt.lte(now))
        // Specialized tick functions own these kinds (`tick_monitor_schedules`,
        // `tick_health_schedules`, `tick_preagg_schedules`). `fire_schedule`
        // can in fact handle `preagg_cycle` — that arm exists so Run now works
        // — but the OWNER is still the specialized tick. Excluding all three
        // here prevents the generic tick from CAS-advancing their `next_run_at`
        // and then failing, or double-firing what the specialized tick is about
        // to claim — either of which starves that tick in the same pass (the
        // row would no longer be due).
        .filter(schedule::Column::TargetKind.is_not_in([
            "monitor_scan",
            "health_eval",
            "preagg_cycle",
        ]))
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(target: "scheduler", error = %e, "tick: query due schedules failed");
            return 0;
        }
    };

    let mut fired = 0;
    for s in due {
        // Strictly-after-now → run-once-then-resume misfire policy.
        let next = match next_occurrence_after(&s.cron_expr, &s.timezone, chrono::Utc::now()) {
            Ok(n) => n.fixed_offset(),
            Err(e) => {
                tracing::warn!(
                    target: "scheduler",
                    schedule_id = %s.id,
                    error = %e,
                    "tick: bad cron/timezone; skipping (fix via CRUD)"
                );
                set_schedule_last_error(db, &s.id, Some(&e)).await;
                continue;
            }
        };

        // Count occurrences between the slot we're about to fire and
        // `now`. Anything in that open range was a slot the tick
        // skipped — the run-once-then-resume policy fires only the
        // due-since slot itself, so each occurrence past `s.next_run_at`
        // is a miss we want to surface (without changing the policy).
        // Capped at 1000 so a misconfigured five-second cron with a
        // multi-year gap doesn't lock the tick.
        let prev_due_utc = s.next_run_at.with_timezone(&chrono::Utc);
        let missed = count_occurrences_between(
            &s.cron_expr,
            &s.timezone,
            prev_due_utc,
            chrono::Utc::now(),
            1000,
        )
        .unwrap_or(0);

        // CAS: advance only if next_run_at still equals what we read. The
        // loser (another replica / concurrent tick) affects 0 rows and
        // skips — exactly-once fire. `missed_runs` and `last_missed_at`
        // are stamped in the same UPDATE so a single replica owns the
        // catch-up accounting too.
        let won = match db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE agentic_schedules \
                 SET next_run_at = $1, \
                     last_fired_at = now(), \
                     missed_runs = missed_runs + $4, \
                     last_missed_at = CASE WHEN $4 > 0 THEN now() ELSE last_missed_at END, \
                     updated_at = now() \
                 WHERE id = $2 AND next_run_at = $3",
                [
                    next.into(),
                    s.id.clone().into(),
                    s.next_run_at.into(),
                    (missed as i32).into(),
                ],
            ))
            .await
        {
            Ok(r) => r.rows_affected() == 1,
            Err(e) => {
                tracing::error!(target: "scheduler", schedule_id = %s.id, error = %e, "tick: CAS failed");
                continue;
            }
        };
        if !won {
            continue;
        }
        if missed > 0 {
            // Warn only — policy is still "fire one, resume". The count
            // tells the user (via the UI) how many slots were skipped.
            tracing::warn!(
                target: "scheduler",
                schedule_id = %s.id,
                missed,
                schedule_name = %s.name,
                "tick: catch-up fire skipped {} occurrences (policy: run-once-then-resume)",
                missed,
            );
        }

        match fire_schedule(db, workspace, &s, "scheduled").await {
            Ok(FireOutcome::Seeded(rid)) => {
                fired += 1;
                tracing::info!(
                    target: "scheduler",
                    schedule_id = %s.id,
                    run_id = %rid,
                    target_kind = %s.target_kind,
                    "tick: fired schedule (Global run seeded)"
                );
                record_fire_success(db, &s.id, &rid).await;
            }
            // The prior load is still running. `next_run_at` has already been
            // advanced, so this slot is simply dropped — same "fire one, resume"
            // policy the catch-up path uses for missed occurrences. Not counted
            // in `fired`, and `last_error` is left alone: a schedule that
            // out-paces its own load is operating normally, not failing.
            Ok(FireOutcome::SkippedAlreadyRunning(rid)) => {
                tracing::info!(
                    target: "scheduler",
                    schedule_id = %s.id,
                    schedule_name = %s.name,
                    in_flight_run_id = %rid,
                    "tick: slot skipped — previous run still in flight (single-flight)"
                );
            }
            Err(e) => {
                // next_run_at was already advanced — the seed failed
                // (e.g. bad ref). The schedule keeps its cadence rather
                // than tight-looping on a bad row.
                tracing::error!(
                    target: "scheduler",
                    schedule_id = %s.id,
                    error = %e,
                    "tick: seed failed; schedule advanced, will retry next slot"
                );
                set_schedule_last_error(db, &s.id, Some(&e)).await;
            }
        }
    }

    fired
}

/// Run one scheduler pass for `monitor_scan` schedules in the given workspace.
/// Creates an `agentic_runs` row per due schedule and spawns the scan in a
/// background task so the tick loop is not blocked. The run is visible in
/// the coordinator immediately as "running".
/// Returns the number of schedules fired. Never errors the caller —
/// per-schedule failures are logged and skipped.
pub async fn tick_monitor_schedules(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    platform: Arc<dyn PlatformContext>,
) -> usize {
    let now = chrono::Utc::now().fixed_offset();
    let due = match schedule::Entity::find()
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .filter(schedule::Column::TargetKind.eq("monitor_scan"))
        .filter(schedule::Column::Enabled.eq(true))
        .filter(schedule::Column::NextRunAt.lte(now))
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(target: "scheduler", error = %e, "monitor tick: query failed");
            return 0;
        }
    };

    let mut fired = 0;
    for s in due {
        // Validate granularity before touching next_run_at — a misconfigured
        // row is a data error, not a transient failure, so we don't advance.
        let granularity = match s
            .variables
            .as_ref()
            .and_then(|v| v.get("granularity"))
            .and_then(|g| g.as_str())
        {
            Some(g) => g.to_string(),
            None => {
                set_schedule_last_error(db, &s.id, Some("missing granularity in variables")).await;
                continue;
            }
        };

        let next = match next_occurrence_after(&s.cron_expr, &s.timezone, chrono::Utc::now()) {
            Ok(n) => n.fixed_offset(),
            Err(e) => {
                tracing::warn!(
                    target: "scheduler",
                    schedule_id = %s.id,
                    error = %e,
                    "monitor tick: bad cron/timezone; skipping"
                );
                set_schedule_last_error(db, &s.id, Some(&e)).await;
                continue;
            }
        };

        let prev_due_utc = s.next_run_at.with_timezone(&chrono::Utc);
        let missed = count_occurrences_between(
            &s.cron_expr,
            &s.timezone,
            prev_due_utc,
            chrono::Utc::now(),
            1000,
        )
        .unwrap_or(0);

        // CAS-advance: exactly-once fire across replicas.
        let won = match db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE agentic_schedules \
                 SET next_run_at = $1, \
                     last_fired_at = now(), \
                     missed_runs = missed_runs + $4, \
                     last_missed_at = CASE WHEN $4 > 0 THEN now() ELSE last_missed_at END, \
                     updated_at = now() \
                 WHERE id = $2 AND next_run_at = $3",
                [
                    next.into(),
                    s.id.clone().into(),
                    s.next_run_at.into(),
                    (missed as i32).into(),
                ],
            ))
            .await
        {
            Ok(r) => r.rows_affected() == 1,
            Err(e) => {
                tracing::error!(target: "scheduler", schedule_id = %s.id, error = %e, "monitor tick: CAS failed");
                continue;
            }
        };
        if !won {
            continue;
        }
        if missed > 0 {
            tracing::warn!(
                target: "scheduler",
                schedule_id = %s.id,
                missed,
                "monitor tick: catch-up fire skipped {} occurrences (policy: run-once-then-resume)",
                missed,
            );
        }

        let run_id = Uuid::new_v4().to_string();
        let mut meta = serde_json::json!({ "granularity": granularity });
        stamp_trigger_metadata(&mut meta, &Some("scheduled".into()), &None, &None);
        if let Err(e) = insert_run_with_schedule(
            db,
            &run_id,
            &format!("Anomaly scan ({granularity})"),
            None,
            "monitor_scan",
            Some(meta),
            &s.id,
            workspace_id,
        )
        .await
        {
            tracing::error!(target: "scheduler", schedule_id = %s.id, error = %e, "monitor tick: failed to create run row; skipping enqueue");
            continue;
        }

        fired += 1;
        record_fire_success(db, &s.id, &run_id).await;

        // Spawn the scan as a background task so the tick loop advances
        // immediately. The run is already visible as "running" in the
        // coordinator; status is updated to done/failed when the scan finishes.
        let db_scan = db.clone();
        let platform_scan = platform.clone();
        let schedule_id_scan = s.id.clone();
        let run_id_scan = run_id.clone();
        let granularity_scan = granularity.clone();
        spawn_with_hub(async move {
            let Some(port) = platform_scan.as_monitor_scan_port() else {
                tracing::error!(target: "monitor_scan", run_id = %run_id_scan, "no MonitorScanPort available");
                let _ = update_run_failed(
                    &db_scan,
                    &run_id_scan,
                    "monitor scan not available in this deployment",
                )
                .await;
                return;
            };
            match port
                .run_monitor_scan(&db_scan, workspace_id, &granularity_scan)
                .await
            {
                Ok(summary) => {
                    tracing::info!(target: "monitor_scan", run_id = %run_id_scan, %summary, "scan complete");
                    let _ = update_run_done(&db_scan, &run_id_scan, &summary, None).await;
                }
                Err(e) => {
                    tracing::warn!(target: "monitor_scan", run_id = %run_id_scan, error = %e, "scan failed");
                    let _ = update_run_failed(&db_scan, &run_id_scan, &e).await;
                    set_schedule_last_error(&db_scan, &schedule_id_scan, Some(&e)).await;
                }
            }
        });
        tracing::info!(target: "monitor_scan", schedule_id = %s.id, run_id = %run_id, %granularity, "scan spawned");
    }

    fired
}

/// Fire due per-workspace `health_eval` schedule rows. Each row's `target_ref`
/// is a workspace id; for each due row this CAS-advances `next_run_at` (via the
/// interval sentinel) and **enqueues** a `TaskScope::Global`
/// `TaskSpec::Custom { kind: "health_eval_workspace", payload: { workspace_id } }`
/// onto the durable queue. The heavy eval runs on the worker/global-run fleet
/// (see `HealthEvalTaskExecutor`), not inline here. Never errors the caller —
/// per-schedule failures are logged and skipped.
pub async fn tick_health_schedules(db: &DatabaseConnection) -> usize {
    let now = chrono::Utc::now().fixed_offset();
    let due = match schedule::Entity::find()
        .filter(schedule::Column::TargetKind.eq("health_eval"))
        .filter(schedule::Column::Enabled.eq(true))
        .filter(schedule::Column::NextRunAt.lte(now))
        // Oldest-due first + a per-tick cap so a post-outage backlog (every
        // workspace elapsed at once) drains over several ticks rather than a
        // single N-wide enqueue spike.
        .order_by_asc(schedule::Column::NextRunAt)
        .limit(MAX_HEALTH_FIRES_PER_TICK)
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(target: "scheduler", error = %e, "health tick: query failed");
            return 0;
        }
    };
    if due.len() as u64 == MAX_HEALTH_FIRES_PER_TICK {
        tracing::info!(
            target: "health_eval",
            cap = MAX_HEALTH_FIRES_PER_TICK,
            "health tick hit per-tick fire cap; remaining due rows drain next tick"
        );
    }

    let mut fired = 0;
    for s in due {
        let next = match next_run_for(&s, chrono::Utc::now()) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(
                    target: "scheduler",
                    schedule_id = %s.id,
                    error = %e,
                    "health tick: bad cadence; skipping"
                );
                set_schedule_last_error(db, &s.id, Some(&e)).await;
                continue;
            }
        };
        if !cas_advance_next_run(db, &s.id, s.next_run_at, next).await {
            // Another replica won the CAS race for this fire slot.
            continue;
        }
        let workspace_id = match s.target_ref.parse::<uuid::Uuid>() {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(
                    target: "scheduler",
                    schedule_id = %s.id,
                    error = %e,
                    "health tick: target_ref is not a workspace uuid; skipping"
                );
                set_schedule_last_error(db, &s.id, Some(&format!("bad target_ref: {e}"))).await;
                continue;
            }
        };
        // A scheduled fire never forces the probes: the smoke cadence is the
        // workspace's own, and overriding it here would bill every workspace the
        // agent-probe token cost on every eval pass.
        match start_health_eval_run(db, workspace_id, Some(&s.id), false).await {
            Ok(run_id) => {
                fired += 1;
                record_fire_success(db, &s.id, &run_id).await;
            }
            Err(e) => {
                tracing::warn!(
                    target: "scheduler",
                    schedule_id = %s.id,
                    error = %e,
                    "health tick: enqueue failed"
                );
                set_schedule_last_error(db, &s.id, Some(&e)).await;
            }
        }
    }
    fired
}

/// CAS-advance `next_run_at` for a schedule. Returns `true` only when this
/// replica's UPDATE affected exactly one row (exactly-once fire). The simple
/// shape (no missed-runs accounting) suits the singleton health row, which
/// catches up on the next tick regardless of how many slots were skipped.
async fn cas_advance_next_run(
    db: &DatabaseConnection,
    id: &str,
    observed: chrono::DateTime<chrono::FixedOffset>,
    next: chrono::DateTime<chrono::FixedOffset>,
) -> bool {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_schedules SET next_run_at = $1, last_fired_at = now(), \
         updated_at = now() WHERE id = $2 AND next_run_at = $3",
        [next.into(), id.to_string().into(), observed.into()],
    ))
    .await
    .map(|r| r.rows_affected() == 1)
    .unwrap_or(false)
}

/// Seed a `TaskScope::Global` run carrying the per-workspace health-eval Custom
/// task. The fleet's `HealthEvalTaskExecutor` (registered via the host's
/// `CustomTaskRegistry`) drains it and runs `run_eval_pass_single`.
///
/// The root task_id **must** equal the run_id, exactly like every other Global
/// seed (`start_automation_run` / `start_agent_run` / `start_airway_run` all do
/// `enqueue_task(db, &run_id, &run_id, …)`). The latency worker's pickup query
/// (`find_pending_global_runs`) and the run-scoped `DurableTransport::with_router`
/// both key the root off `task_id = run_id`; a root task with any other id is
/// invisible to both, so the run sits `running` forever (the original bug here,
/// where the id was `health_eval:{ws}:{fire_slot}`). Per-fire dedup is already
/// guaranteed upstream by `cas_advance_next_run`, and each fire mints a fresh
/// `run_id`, so no deterministic task_id is needed.
///
/// The run row is stamped with `schedule_id` (via `insert_run_with_schedule`,
/// like every other scheduled kind) so the per-job run-history query
/// (`WHERE schedule_id = $1`) surfaces scheduled fires under the job's "Recent
/// runs"; a plain `insert_run` leaves `schedule_id` NULL and the fire is
/// invisible on the job page even though it ran.
async fn start_health_eval_run(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    schedule_id: Option<&str>,
    force_smoke: bool,
) -> Result<String, String> {
    use agentic_core::delegation::TaskSpec;
    use agentic_runtime::crud::{TaskScope, enqueue_task};
    use agentic_runtime::lifecycle::crud::runs::insert_run;

    let run_id = uuid::Uuid::new_v4().to_string();
    // Scheduled fires stamp `schedule_id` so they surface under the job's run
    // history; an on-demand fire before the workspace's health row exists has no
    // schedule to attribute to and uses the plain insert.
    match schedule_id {
        Some(schedule_id) => insert_run_with_schedule(
            db,
            &run_id,
            HEALTH_SCHEDULE_NAME,
            None,
            "health_eval_workspace",
            None,
            schedule_id,
            workspace_id,
        )
        .await
        .map_err(|e| e.to_string())?,
        None => insert_run(
            db,
            &run_id,
            HEALTH_SCHEDULE_NAME,
            None,
            "health_eval_workspace",
            None,
            workspace_id,
        )
        .await
        .map_err(|e| e.to_string())?,
    }

    // `force_smoke` rides in the payload rather than a separate task kind: the
    // work is the same eval pass, and the executor is the only thing that needs
    // to know the probes were asked for out of cadence. Absent → false, so a task
    // enqueued by an older instance mid-deploy still deserializes.
    let spec = TaskSpec::Custom {
        kind: "health_eval_workspace".into(),
        payload: serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "force_smoke": force_smoke,
        }),
    };
    enqueue_task(db, &run_id, &run_id, None, &spec, None, TaskScope::Global)
        .await
        .map_err(|e| e.to_string())?;
    Ok(run_id)
}

/// Enqueue an on-demand (operator-triggered) health eval for one workspace and
/// return its `run_id` for status polling.
///
/// Mirrors the scheduled fire path — seeds a `TaskScope::Global`
/// `health_eval_workspace` Custom task drained by the worker fleet's
/// `HealthEvalTaskExecutor` — so the heavy eval (Postgres signals + live Toast
/// reconciliation) runs off the HTTP handler and survives instance death, rather
/// than blocking the request inline. Stamps the run with the workspace's health
/// `schedule_id` when one exists so manual runs surface under the job's run
/// history alongside scheduled fires; before the workspace's first compile there
/// is no row to attribute to and the run is inserted unattributed.
///
/// `force_smoke` asks the eval to run the workspace's smoke probes even if their
/// (default 6h) cadence has not elapsed — the admin Health tab's "Run smoke test"
/// button. It cannot switch the probes *on*: a workspace with
/// `smoke_test: { enabled: false }` still runs none.
pub async fn enqueue_health_eval(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    force_smoke: bool,
) -> Result<String, String> {
    let schedule_id = schedule::Entity::find()
        .filter(schedule::Column::TargetKind.eq("health_eval"))
        .filter(schedule::Column::TargetRef.eq(workspace_id.to_string()))
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .map(|r| r.id);
    start_health_eval_run(db, workspace_id, schedule_id.as_deref(), force_smoke).await
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
            active.updated_at = Set(agentic_runtime::crud::now());
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

/// Max `preagg_cycle` rows a single [`tick_preagg_schedules`] pass will fire.
const MAX_PREAGG_FIRES_PER_TICK: u64 = 256;

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
            active.updated_at = Set(agentic_runtime::crud::now());
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
async fn start_preagg_cycle_run(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    schedule_id: Option<&str>,
    force: bool,
    target: Option<(String, String)>,
    trigger: &str,
) -> Result<String, String> {
    use agentic_core::delegation::TaskSpec;
    use agentic_runtime::crud::{TaskScope, enqueue_task};
    use agentic_runtime::lifecycle::crud::runs::insert_run;

    let run_id = uuid::Uuid::new_v4().to_string();
    // Stamp the trigger (`scheduled` for a cron fire, `manual` for a run-now or
    // a Rebuild click) so the run history labels a preagg run the same way it
    // labels workflow/airway/agent ones.
    let mut metadata = serde_json::json!({});
    stamp_trigger_metadata(&mut metadata, &Some(trigger.to_string()), &None, &None);
    match schedule_id {
        Some(schedule_id) => insert_run_with_schedule(
            db,
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
            db,
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
    enqueue_task(db, &run_id, &run_id, None, &spec, None, TaskScope::Global)
        .await
        .map_err(|e| e.to_string())?;
    Ok(run_id)
}

/// Enqueue an on-demand (operator-triggered) preagg cycle for one workspace and
/// return its `run_id` for status polling — the IDE's "Rebuild" / "Rebuild all"
/// buttons. Always `force: true`: pressing the button is the operator's answer
/// to "is it stale", so the cycle rebuilds `target` (or everything, if `target`
/// is `None`) regardless of what the refresh key says.
///
/// Mirrors [`enqueue_health_eval`]'s shape — attributes the run to the
/// workspace's `preagg_cycle` schedule row when one exists (so a manual click
/// surfaces under the same run history as a scheduled fire), unattributed
/// otherwise (before the workspace's first compile, or when pre-aggregation is
/// disabled entirely, there is no row to attribute to — an operator can still
/// force one rollup ad hoc).
pub async fn enqueue_preagg_cycle(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    target: Option<(String, String)>,
) -> Result<String, String> {
    let schedule_id = schedule::Entity::find()
        .filter(schedule::Column::TargetKind.eq("preagg_cycle"))
        .filter(schedule::Column::TargetRef.eq(workspace_id.to_string()))
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .map(|r| r.id);
    start_preagg_cycle_run(
        db,
        workspace_id,
        schedule_id.as_deref(),
        true,
        target,
        "manual",
    )
    .await
}

/// Fire due per-workspace `preagg_cycle` schedule rows. Mirrors
/// [`tick_health_schedules`] exactly: CAS-advances each due row's
/// `next_run_at`, then enqueues an unforced (`force: false`, `target: None`)
/// cycle — the executor still honors each rollup's own refresh key, so a
/// scheduled fire only rebuilds what's actually stale. Never errors the
/// caller; per-schedule failures are logged and skipped.
pub async fn tick_preagg_schedules(db: &DatabaseConnection) -> usize {
    let now = chrono::Utc::now().fixed_offset();
    let due = match schedule::Entity::find()
        .filter(schedule::Column::TargetKind.eq("preagg_cycle"))
        .filter(schedule::Column::Enabled.eq(true))
        .filter(schedule::Column::NextRunAt.lte(now))
        .order_by_asc(schedule::Column::NextRunAt)
        .limit(MAX_PREAGG_FIRES_PER_TICK)
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(target: "preagg", error = %e, "preagg tick: query failed");
            return 0;
        }
    };
    if due.len() as u64 == MAX_PREAGG_FIRES_PER_TICK {
        tracing::info!(
            target: "preagg",
            cap = MAX_PREAGG_FIRES_PER_TICK,
            "preagg tick hit per-tick fire cap; remaining due rows drain next tick"
        );
    }

    let mut fired = 0;
    for s in due {
        let next = match next_run_for(&s, chrono::Utc::now()) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(
                    target: "preagg",
                    schedule_id = %s.id,
                    error = %e,
                    "preagg tick: bad cadence; skipping"
                );
                set_schedule_last_error(db, &s.id, Some(&e)).await;
                continue;
            }
        };
        if !cas_advance_next_run(db, &s.id, s.next_run_at, next).await {
            continue;
        }
        let workspace_id = match s.target_ref.parse::<uuid::Uuid>() {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(
                    target: "preagg",
                    schedule_id = %s.id,
                    error = %e,
                    "preagg tick: target_ref is not a workspace uuid; skipping"
                );
                set_schedule_last_error(db, &s.id, Some(&format!("bad target_ref: {e}"))).await;
                continue;
            }
        };
        match start_preagg_cycle_run(db, workspace_id, Some(&s.id), false, None, "scheduled").await
        {
            Ok(run_id) => {
                fired += 1;
                record_fire_success(db, &s.id, &run_id).await;
            }
            Err(e) => {
                tracing::warn!(
                    target: "preagg",
                    schedule_id = %s.id,
                    error = %e,
                    "preagg tick: failed to seed run"
                );
                set_schedule_last_error(db, &s.id, Some(&e)).await;
            }
        }
    }
    fired
}

// ── Shared firing ───────────────────────────────────────────────────────────

/// What a fire attempt actually did.
///
/// "Already running" is deliberately NOT an error: for a periodic ELT it is
/// the expected outcome whenever a load outruns its own cadence, and routing
/// it through the error arm would stamp `last_error` and paint the schedule
/// red for a system behaving exactly as designed. It collapses instead —
/// matching the existing CAS `next_run_at` policy, where missed slots fire
/// once rather than queueing up.
#[derive(Debug, Clone)]
pub enum FireOutcome {
    /// A new run was seeded.
    Seeded(String),
    /// Single-flight rejected this tick; carries the run already in flight.
    SkippedAlreadyRunning(String),
}

/// Seed a `TaskScope::Global` run for `s`. Shared by the tick and run-now;
/// caller picks the trigger label so the run log can distinguish
/// `scheduled` / `manual` / `backfill` at a glance.
async fn fire_schedule(
    db: &DatabaseConnection,
    workspace: &dyn crate::WorkflowWorkspaceContext,
    s: &schedule::Model,
    trigger: &str,
) -> Result<FireOutcome, String> {
    match s.target_kind.as_str() {
        "workflow" => {
            let req = StartAutomationRequest {
                workflow_ref: s.target_ref.clone(),
                variables: s.variables.clone(),
                retry_from_run_id: None,
                cache_enabled: false,
                invalidate_steps: None,
                invalidate_iterations: None,
                thread_id: None,
                schedule_id: Some(s.id.clone()),
                trigger: Some(trigger.to_string()),
                logical_date: None,
                retry_of: None,
            };
            start_automation_run(
                db,
                req,
                agentic_runtime::crud::TaskScope::Global,
                s.workspace_id,
            )
            .await
            .map(FireOutcome::Seeded)
            .map_err(|e| e.to_string())
        }
        "airway" => {
            let req = StartAirwayRequest {
                pipeline_ref: s.target_ref.clone(),
                variables: s.variables.clone(),
                thread_id: None,
                resources: Vec::new(),
                schedule_id: Some(s.id.clone()),
                trigger: Some(trigger.to_string()),
                logical_date: None,
                retry_of: None,
                backfill_from: None,
                backfill_to: None,
            };
            match start_airway_run(
                db,
                workspace,
                req,
                agentic_runtime::crud::TaskScope::Global,
                s.workspace_id,
            )
            .await
            {
                // May be a COALESCED id: since submit stopped refusing
                // contended callers, a tick landing while a run is already
                // queued gets that run's id back rather than an error. The
                // slot still collapses onto one run — the same end state
                // `SkippedAlreadyRunning` produced — but it is now recorded as
                // `Seeded`, so a schedule that used to show "skipped" shows the
                // run it joined instead.
                Ok(rid) => Ok(FireOutcome::Seeded(rid)),
                // Defensive only. `start_airway_run` no longer raises this:
                // contention coalesces at submit and defers at claim. Kept so a
                // future producer of `AlreadyRunning` cannot silently turn a
                // collapsed slot into a scheduler error.
                Err(crate::airway_run::AirwayRunError::AlreadyRunning { run_id, .. }) => {
                    Ok(FireOutcome::SkippedAlreadyRunning(run_id))
                }
                // NOTE: `Unavailable` deliberately has no arm here, and an
                // arm would not help. `tick_schedules` CAS-advances
                // `next_run_at` BEFORE calling this — that CAS is what makes
                // firing exactly-once across replicas — so by the time any
                // outcome is returned the slot is already gone, exactly as the
                // `SkippedAlreadyRunning` comment at the call site says. Making
                // a mid-deploy blip re-firable means re-arming `next_run_at` or
                // reversing the CAS/fire order, either of which risks the
                // double-fire the CAS exists to prevent. Left as a known gap:
                // a scheduled fire landing inside a compile window drops that
                // occurrence, and the next one runs normally.
                Err(e) => Err(e.to_string()),
            }
        }
        "agent" => {
            // validate_input guarantees `question` is present + non-empty
            // for agent schedules, but an old row could legally have NULL
            // — surface a clear error rather than panicking.
            let question = s.question.clone().ok_or_else(|| {
                "agent schedule has no question stored — re-save the schedule".to_string()
            })?;
            let req = StartAgentRequest {
                agent_id: s.target_ref.clone(),
                question,
                thread_id: None,
                schedule_id: Some(s.id.clone()),
                trigger: Some(trigger.to_string()),
                logical_date: None,
                retry_of: None,
            };
            start_agent_run(
                db,
                req,
                agentic_runtime::crud::TaskScope::Global,
                s.workspace_id,
            )
            .await
            .map(FireOutcome::Seeded)
            .map_err(|e| e.to_string())
        }
        "function" => {
            // A scheduled custom-app Oxy Function. `target_ref` encodes
            // "<app_id>/<function_name>"; the host-side `AppFunctionTaskExecutor`
            // (Custom-task registry) runs it under the org-owner identity. This
            // arm stays entity/oxy-free — it only seeds a run + a Custom task
            // whose string payload the host resolves.
            let (app_id, function_name) = s.target_ref.split_once('/').ok_or_else(|| {
                format!(
                    "function schedule target_ref must be '<app_id>/<function_name>', got {:?}",
                    s.target_ref
                )
            })?;
            let run_id = uuid::Uuid::new_v4().to_string();
            // Stamp the trigger (`scheduled` vs `manual` run-now) into the run's
            // metadata so the dashboard can label it, mirroring what the
            // workflow/airway/agent arms get from their `start_*` helpers.
            let mut metadata = serde_json::json!({});
            stamp_trigger_metadata(&mut metadata, &Some(trigger.to_string()), &None, &None);
            insert_run_with_schedule(
                db,
                &run_id,
                &s.name,
                None,
                "app_function",
                Some(metadata),
                &s.id,
                s.workspace_id,
            )
            .await
            .map_err(|e| e.to_string())?;
            let spec = agentic_core::delegation::TaskSpec::Custom {
                kind: "app_function".into(),
                // Carry the trigger (`scheduled` for a cron fire, `manual` for a
                // run-now) so the executor records the invocation `mode` to match.
                payload: serde_json::json!({
                    "app_id": app_id,
                    "function_name": function_name,
                    "trigger": trigger,
                }),
            };
            // The job's retry policy, if any, is stamped onto the schedule's
            // `variables` at publish (`{"task_policy": …}`). Deserialize it here
            // so a scheduled run inherits the durable queue's retry/backoff — this
            // arm stays entity/oxy-free (it only reads a TaskPolicy JSON blob).
            let policy = s
                .variables
                .as_ref()
                .and_then(|v| v.get("task_policy"))
                .and_then(|p| {
                    serde_json::from_value::<agentic_core::delegation::TaskPolicy>(p.clone()).ok()
                });
            agentic_runtime::crud::enqueue_task(
                db,
                &run_id,
                &run_id,
                None,
                &spec,
                policy.as_ref(),
                agentic_runtime::crud::TaskScope::Global,
            )
            .await
            .map_err(|e| e.to_string())?;
            Ok(FireOutcome::Seeded(run_id))
        }
        // The per-workspace pre-aggregation cycle. Unlike `monitor_scan` /
        // `health_eval` — which the host runs inline and therefore special-cases
        // in its run-now handler — a preagg cycle is already a durable Custom
        // task, so it fires here like any other kind and the fleet's
        // `PreaggTaskExecutor` drains it.
        //
        // `force: false` deliberately: a run-now on the *schedule* is a manual
        // fire of the scheduled cadence, so it still honours each rollup's
        // refresh key and rebuilds only what is stale (same policy as the
        // health_eval run-now arm, which doesn't force smoke probes). Forcing a
        // rebuild regardless of staleness is the Semantic tab's Rebuild button,
        // which goes through `enqueue_preagg_cycle`.
        "preagg_cycle" => {
            // `target_ref` is the workspace uuid — same convention
            // `tick_preagg_schedules` reads.
            let workspace_id = s.target_ref.parse::<uuid::Uuid>().map_err(|e| {
                format!(
                    "preagg schedule target_ref must be a workspace uuid, got {:?}: {e}",
                    s.target_ref
                )
            })?;
            start_preagg_cycle_run(db, workspace_id, Some(&s.id), false, None, trigger)
                .await
                .map(FireOutcome::Seeded)
        }
        // `backfill_schedule` rejects the system-managed kinds
        // (`monitor_scan`, `health_eval`, `preagg_cycle`) before it gets here,
        // so what remains really is unrecognised.
        other => Err(format!("unknown target_kind {other:?}")),
    }
}

/// Record a successful fire: link the run and clear any prior error, in
/// one UPDATE. Best-effort — the run is already seeded, so a failure here
/// doesn't lose work.
pub async fn record_fire_success(db: &DatabaseConnection, schedule_id: &str, run_id: &str) {
    if let Err(e) = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_schedules SET last_run_id = $1, last_error = NULL WHERE id = $2",
            [run_id.into(), schedule_id.into()],
        ))
        .await
    {
        tracing::warn!(target: "scheduler", %schedule_id, error = %e, "record fire success failed");
    }
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

// ── Backfill ────────────────────────────────────────────────────────────────
//
// Fill in runs for cron slots the operator wants to replay — typically
// the missing-slot gaps the dashboard surfaces. Each seeded run carries:
//
//   * `schedule_id` — first-class column linking it to the originating job,
//   * `metadata.trigger = "backfill"` — distinguishes it from `scheduled` /
//     `manual` runs in the run log,
//   * `metadata.logical_date` — the cron-scheduled time being replayed,
//     so date-aware downstream logic can use the intended fire time
//     rather than `now()`.

/// Inputs for [`backfill_schedule`]. Body of the backfill HTTP endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct BackfillRequest {
    /// Inclusive lower bound of the range to backfill (UTC).
    pub from: chrono::DateTime<chrono::Utc>,
    /// Inclusive upper bound; must be strictly greater than `from`.
    pub to: chrono::DateTime<chrono::Utc>,
    /// Execution-side throttle hint: `"sequential"` | `"<N>"` | `"all"`.
    /// Currently advisory — recorded on each run's metadata for a future
    /// executor that honors per-schedule throttling. Seeding itself is
    /// sequential.
    #[serde(default)]
    pub concurrency: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackfillResult {
    /// The runs that were successfully seeded into the queue.
    pub run_ids: Vec<String>,
    /// Total cron occurrences enumerated in the requested window. May
    /// exceed `run_ids.len()` if some seed calls failed.
    pub planned: usize,
}

/// Maximum number of slots a single backfill request may seed. Bounds the
/// blast radius — if an operator requested a year of a 1-minute cron we'd
/// otherwise enqueue 525,600 runs in one call.
const BACKFILL_MAX_OCCURRENCES: usize = 500;

/// Seed one run per cron occurrence in `[from, to]`, tagged as backfill.
/// Returns the seeded `run_id`s. Sequential v1 — see [`BackfillRequest`].
pub async fn backfill_schedule(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    workspace: &dyn crate::WorkflowWorkspaceContext,
    schedule_id: &str,
    request: BackfillRequest,
) -> Result<BackfillResult, ScheduleError> {
    let s = get_schedule(db, workspace_id, schedule_id).await?;
    if request.to <= request.from {
        return Err(ScheduleError::Invalid(
            "backfill `to` must be after `from`".into(),
        ));
    }

    // Cron-replay seeds every occurrence at once, which is a request for N
    // concurrent runs of one pipeline — exactly what single-flight forbids, and
    // for the same reason chunked backfill is now pinned to one chunk at a time:
    // every run of a pipeline shares one `<schema>_raw.<table>` staging buffer, and a
    // fold's watermark spans the whole buffer, so concurrent runs consume each
    // other's half-loaded rows.
    //
    // Rejected up front rather than seeded-then-collapsed. Previously
    // occurrence #1 took the lease, #2 returned `AlreadyRunning`, the loop
    // stopped at the first error, and the caller got one run plus a success
    // response claiming N planned — the worst of the options. `oxy airway
    // backfill` is the tool for this: it splits a window into checkpointed
    // chunks and runs them serially, resuming where it left off.
    if s.target_kind == "airway" {
        return Err(ScheduleError::Invalid(
            "cron-replay backfill is not supported for airway schedules: it would \
             start several runs of one pipeline at once, which corrupts the shared \
             staging buffer. Use `oxy airway backfill --from … --to …` (or the \
             Backfill dialog), which runs checkpointed chunks one at a time and \
             resumes on failure. To re-fire a SINGLE missed slot, use Run now — \
             that seeds one run, which single-flight admits."
                .into(),
        ));
    }

    // System-managed kinds: the schedule row exists so the cadence is
    // configurable, but the work behind it has no per-occurrence identity to
    // replay. A monitor scan reads whatever the warehouse holds now; a health
    // evaluation and a pre-aggregation cycle refresh a cache to *current*.
    // Seeding "the run that would have happened at 03:00" is meaningless for
    // all three — it would just run the same present-tense job N times.
    // Rejected up front, with the reason, rather than reaching
    // `seed_backfill_occurrence` and failing there as an unknown kind (a 500
    // that reads like a bug). `Run now` is the operation these kinds do have.
    if matches!(
        s.target_kind.as_str(),
        "monitor_scan" | "health_eval" | "preagg_cycle"
    ) {
        return Err(ScheduleError::Invalid(format!(
            "backfill is not supported for {} schedules: the job refreshes to the \
             present, so replaying a past occurrence would re-run the same work \
             under a misleading logical date. Use Run now instead.",
            s.target_kind
        )));
    }

    // The cron evaluator's range is half-open (after, until], so step
    // back one second on the lower bound so the very first occurrence at
    // `from` is included — operators expect the inclusive range they typed
    // in the dialog.
    let after = request.from - chrono::Duration::seconds(1);
    let occurrences = occurrences_between(
        &s.cron_expr,
        &s.timezone,
        after,
        request.to,
        BACKFILL_MAX_OCCURRENCES,
    )
    .map_err(ScheduleError::Invalid)?;

    let planned = occurrences.len();
    if planned == 0 {
        return Ok(BackfillResult {
            run_ids: Vec::new(),
            planned: 0,
        });
    }

    let mut run_ids = Vec::with_capacity(planned);
    for occurrence in occurrences {
        let run_id =
            match seed_backfill_occurrence(db, workspace, &s, occurrence, &request.concurrency)
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(
                        target: "scheduler",
                        schedule_id = %s.id,
                        logical_date = %occurrence,
                        error = %e,
                        "backfill: seed failed; stopping at first error",
                    );
                    if run_ids.is_empty() {
                        return Err(ScheduleError::Invalid(e));
                    }
                    // Partial success: return what we have so the operator
                    // can act on it rather than losing the seeded runs.
                    return Ok(BackfillResult { run_ids, planned });
                }
            };
        run_ids.push(run_id);
    }

    Ok(BackfillResult { run_ids, planned })
}

async fn seed_backfill_occurrence(
    db: &DatabaseConnection,
    workspace: &dyn crate::WorkflowWorkspaceContext,
    s: &schedule::Model,
    occurrence: chrono::DateTime<chrono::Utc>,
    _concurrency: &Option<String>,
) -> Result<String, String> {
    match s.target_kind.as_str() {
        "workflow" => {
            let req = StartAutomationRequest {
                workflow_ref: s.target_ref.clone(),
                variables: s.variables.clone(),
                retry_from_run_id: None,
                cache_enabled: false,
                invalidate_steps: None,
                invalidate_iterations: None,
                thread_id: None,
                schedule_id: Some(s.id.clone()),
                trigger: Some("backfill".to_string()),
                logical_date: Some(occurrence),
                retry_of: None,
            };
            start_automation_run(
                db,
                req,
                agentic_runtime::crud::TaskScope::Global,
                s.workspace_id,
            )
            .await
            .map_err(|e| e.to_string())
        }
        "airway" => {
            let req = StartAirwayRequest {
                pipeline_ref: s.target_ref.clone(),
                variables: s.variables.clone(),
                thread_id: None,
                resources: Vec::new(),
                schedule_id: Some(s.id.clone()),
                trigger: Some("backfill".to_string()),
                logical_date: Some(occurrence),
                retry_of: None,
                // Schedule "backfill" is cron-replay, not a date-window
                // backfill — it re-fires occurrences, it doesn't pin a window.
                backfill_from: None,
                backfill_to: None,
            };
            start_airway_run(
                db,
                workspace,
                req,
                agentic_runtime::crud::TaskScope::Global,
                s.workspace_id,
            )
            .await
            .map_err(|e| e.to_string())
        }
        "agent" => {
            let question = s.question.clone().ok_or_else(|| {
                "agent schedule has no question stored — re-save the schedule".to_string()
            })?;
            let req = StartAgentRequest {
                agent_id: s.target_ref.clone(),
                question,
                thread_id: None,
                schedule_id: Some(s.id.clone()),
                trigger: Some("backfill".to_string()),
                logical_date: Some(occurrence),
                retry_of: None,
            };
            start_agent_run(
                db,
                req,
                agentic_runtime::crud::TaskScope::Global,
                s.workspace_id,
            )
            .await
            .map_err(|e| e.to_string())
        }
        // `backfill_schedule` rejects the system-managed kinds
        // (`monitor_scan`, `health_eval`, `preagg_cycle`) before it gets here,
        // so what remains really is unrecognised.
        other => Err(format!("unknown target_kind {other:?}")),
    }
}

/// Record the most recent fire/seed/cron failure for UI surfacing.
/// Best-effort observability — never blocks the loop.
pub async fn set_schedule_last_error(
    db: &DatabaseConnection,
    schedule_id: &str,
    msg: Option<&str>,
) {
    let msg: Option<String> = msg.map(str::to_string);
    if let Err(e) = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_schedules SET last_error = $1 WHERE id = $2",
            [msg.into(), schedule_id.into()],
        ))
        .await
    {
        tracing::warn!(target: "scheduler", %schedule_id, error = %e, "set last_error failed");
    }
}

#[cfg(test)]
mod interval_tests {
    use super::*;
    use agentic_runtime::entity::schedule;

    fn row(cron: &str) -> schedule::Model {
        let now = chrono::Utc::now().fixed_offset();
        schedule::Model {
            id: "s1".into(),
            workspace_id: uuid::Uuid::nil(),
            project_id: None,
            branch_id: None,
            name: "h".into(),
            target_kind: "health_eval".into(),
            target_ref: uuid::Uuid::nil().to_string(),
            question: None,
            variables: None,
            cron_expr: cron.into(),
            timezone: "UTC".into(),
            enabled: true,
            next_run_at: now,
            last_fired_at: None,
            last_run_id: None,
            last_error: None,
            missed_runs: 0,
            last_missed_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn cron_string_for_interval() {
        assert_eq!(
            health_interval_cron(std::time::Duration::from_secs(1800)),
            "@interval:1800"
        );
    }

    #[test]
    fn next_run_for_interval_is_now_plus_interval() {
        let now = chrono::Utc::now();
        let next = next_run_for(&row("@interval:1800"), now).unwrap();
        assert_eq!((next.to_utc() - now).num_seconds(), 1800);
    }

    #[test]
    fn next_run_for_plain_cron_uses_cron_engine() {
        // "*/10 * * * *" — next fire is within 10 minutes, never +1800s arithmetic.
        let now = chrono::Utc::now();
        let next = next_run_for(&row("*/10 * * * *"), now).unwrap();
        assert!((next.to_utc() - now).num_seconds() <= 600);
    }

    #[test]
    fn malformed_interval_is_error() {
        assert!(next_run_for(&row("@interval:nope"), chrono::Utc::now()).is_err());
    }
}
