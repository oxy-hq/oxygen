//! `monitor_scan` schedule rows follow `.monitor.yml`'s `schedule:` block.
//!
//! In Cloud mode the block is read from the compile boundary — the
//! `monitor_configs` row of the workspace's promoted revision — never from the
//! workspace filesystem. A worker holds no checkout, and the ide's checkout
//! holds whatever is being edited, so the promoted revision is both the only
//! copy every node can read and the right one. Before this the read went to
//! disk on every node on every tick, and a worker warned once per workspace
//! per tick that it had no file to read: 1,463 warnings in two hours on
//! oxy-dev. Local mode, which has a working copy by definition and no
//! revisions, keeps the file read ([`bootstrap_from_file`]).
//!
//! Both sources feed [`bootstrap_from_config`]: create a row for any
//! granularity the block names that has none yet, and reconcile the
//! `timezone` column on rows already present. Otherwise create-only — never
//! touches cadence, the enabled flag or variables on existing rows, and never
//! deletes rows.

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use oxy_metric_monitoring::MonitorConfig;

/// Read the promoted revision's compiled `.monitor.yml` and reconcile the
/// workspace's `monitor_scan` rows to it. No revision, or a revision with no
/// `.monitor.yml`, means no monitors: nothing to do, and not a warning. A
/// definition that will not parse is a genuine problem and stays loud.
///
/// Returns whether the rows are **settled**: `true` when every row the block
/// names exists and the timezones match, or there was nothing to reconcile
/// from — including a definition that does not parse, or a `schedule:` cron
/// the scheduler rejects, which no retry fixes until the revision changes;
/// `false` when a database read or write failed, so the caller leaves the
/// workspace unstamped and the next claim retries.
pub(super) async fn bootstrap_from_compiled(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    revision_id: Option<Uuid>,
) -> bool {
    use sea_orm::EntityTrait;

    let Some(revision_id) = revision_id else {
        tracing::debug!(
            target: "metric_monitoring",
            %workspace_id,
            "bootstrap: no promoted revision; no monitor schedules to reconcile"
        );
        return true;
    };
    let row = match entity::monitor_configs::Entity::find_by_id(revision_id)
        .one(db)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return true,
        Err(e) => {
            tracing::warn!(
                target: "metric_monitoring",
                %workspace_id,
                %revision_id,
                error = %e,
                "bootstrap: failed to read the compiled .monitor.yml; retried next claim"
            );
            return false;
        }
    };
    match oxy_metric_monitoring::from_definition(row.definition) {
        Ok(cfg) => bootstrap_from_config(db, workspace_id, &cfg).await,
        Err(e) => {
            tracing::warn!(
                target: "metric_monitoring",
                %workspace_id,
                %revision_id,
                error = %e,
                "bootstrap: the compiled .monitor.yml does not parse; skipping"
            );
            true
        }
    }
}

/// Read `.monitor.yml` from a working copy and reconcile the workspace's
/// `monitor_scan` rows to it. Local mode only: in Cloud mode the file is read
/// from the compiled revision ([`bootstrap_from_compiled`]).
pub(crate) async fn bootstrap_from_file(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    workspace_root: &std::path::Path,
) {
    let config_path = oxy_metric_monitoring::default_config_path(workspace_root);
    let cfg = match oxy_metric_monitoring::load_from_file(&config_path) {
        Ok(c) => c,
        // "This node holds no working copy" is an ordinary answer, not a
        // failure. A malformed or unreadable file is a genuine problem and
        // stays loud. `WorkspaceAbsent` only — `load_from_file` returns
        // `Ok(default)` for a merely missing file, so `NotFound` is
        // unconstructible there and an arm for it would read as coverage it
        // does not provide.
        Err(e @ oxy_metric_monitoring::ConfigLoadError::WorkspaceAbsent(_)) => {
            tracing::debug!(
                target: "metric_monitoring",
                %workspace_id,
                error = %e,
                "bootstrap: no readable .monitor.yml on this node; skipping"
            );
            return;
        }
        Err(e) => {
            tracing::warn!(
                target: "metric_monitoring",
                %workspace_id,
                error = %e,
                "bootstrap: failed to read .monitor.yml; skipping"
            );
            return;
        }
    };
    bootstrap_from_config(db, workspace_id, &cfg).await;
}

/// The timezone a workspace's `monitor_scan` schedule rows should fire in.
///
/// Only the FILE-level `.monitor.yml` timezone can apply: one schedule row
/// drives every monitor at a given granularity, so a per-monitor override has
/// nothing to attach to. Absent = UTC, preserving existing behavior.
fn desired_schedule_timezone(cfg: &MonitorConfig) -> String {
    cfg.timezone.clone().unwrap_or_else(|| "UTC".to_string())
}

/// Reconcile the timezone of the rows that exist and create a row for every
/// granularity the `schedule:` block names that has none. Returns whether
/// every write that was needed landed.
async fn bootstrap_from_config(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    cfg: &MonitorConfig,
) -> bool {
    use agentic_runtime::entity::schedule;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let timezone = desired_schedule_timezone(cfg);
    let Some(sched) = cfg.schedule.clone() else {
        return true;
    };

    // Fetch existing monitor_scan rows once to avoid N+1 queries.
    let existing = match schedule::Entity::find()
        .filter(schedule::Column::WorkspaceId.eq(workspace_id))
        .filter(schedule::Column::TargetKind.eq("monitor_scan"))
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(
                target: "metric_monitoring",
                %workspace_id,
                error = %e,
                "bootstrap: failed to query existing monitor schedules"
            );
            return false;
        }
    };

    let mut settled = reconcile_schedule_timezones(db, workspace_id, &existing, &timezone).await;

    let has_granularity = |gran: &str| {
        existing.iter().any(|s| {
            s.variables
                .as_ref()
                .and_then(|v| v.get("granularity"))
                .and_then(|g| g.as_str())
                == Some(gran)
        })
    };
    let entries = [
        (sched.daily.as_deref(), "day", "Metric monitoring (daily)"),
        (
            sched.weekly.as_deref(),
            "week",
            "Metric monitoring (weekly)",
        ),
        (
            sched.monthly.as_deref(),
            "month",
            "Metric monitoring (monthly)",
        ),
    ];
    for (maybe_cron, gran, name) in entries {
        let Some(cron_expr) = maybe_cron else {
            continue;
        };
        if has_granularity(gran) {
            continue;
        }
        settled &= create_schedule(db, workspace_id, name, gran, cron_expr, &timezone).await;
    }
    settled
}

/// Create one `monitor_scan` row. Returns whether the row is **settled**:
/// it landed, or it never can from this revision. A failure is logged either
/// way; [`create_error_settled`] decides which failures retry.
async fn create_schedule(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    gran: &str,
    cron_expr: &str,
    timezone: &str,
) -> bool {
    use agentic_pipeline::scheduler::{ScheduleInput, create_schedule};

    let input = ScheduleInput {
        name: name.to_string(),
        target_kind: "monitor_scan".to_string(),
        target_ref: ".monitor.yml".to_string(),
        question: None,
        variables: Some(serde_json::json!({ "granularity": gran })),
        cron_expr: cron_expr.to_string(),
        timezone: timezone.to_string(),
        enabled: true,
    };
    match create_schedule(db, workspace_id, input).await {
        Ok(_) => {
            tracing::info!(
                target: "metric_monitoring",
                %workspace_id,
                granularity = %gran,
                "bootstrapped monitor_scan schedule"
            );
            true
        }
        Err(e) if create_error_settled(&e) => {
            tracing::warn!(
                target: "metric_monitoring",
                %workspace_id,
                granularity = %gran,
                cron = %cron_expr,
                error = %e,
                "bootstrap: the compiled .monitor.yml schedule is not a schedule the \
                 scheduler accepts; skipping until the revision changes"
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                target: "metric_monitoring",
                %workspace_id,
                granularity = %gran,
                error = %e,
                "bootstrap: failed to create monitor_scan schedule; retried next claim"
            );
            false
        }
    }
}

/// Whether a failed `monitor_scan` insert counts as settled.
///
/// The `schedule:` block's cron strings are plain `Option<String>` in
/// `MonitorScheduleConfig`, and `from_definition` validates only timezones and
/// the notify channel, so a promoted `.monitor.yml` with `daily: "every
/// morning"` compiles and is rejected here, by the scheduler's `validate_cron`
/// / `next_after`, as [`ScheduleError::Invalid`]. That is a property of the
/// revision, not of the moment: retrying the insert every claim would re-read
/// three configs and log two lines per 30 s per workspace — the flood the
/// claim-gated reconcile exists to remove — until someone fixes the file, and
/// the fix is a new revision, which reopens the gate on its own. So an
/// `Invalid` settles, like an unparseable definition does. Only a database
/// error retries. `NotFound` is unconstructible from an insert; were it ever
/// returned, no retry would change it either.
fn create_error_settled(e: &agentic_pipeline::scheduler::ScheduleError) -> bool {
    use agentic_pipeline::scheduler::ScheduleError;

    match e {
        ScheduleError::Invalid(_) | ScheduleError::NotFound => true,
        ScheduleError::Db(_) => false,
    }
}

/// Build the `ActiveModel` for a timezone reconcile: sets only `id` (primary
/// key — never emitted in the `SET` clause), `timezone` and `next_run_at`,
/// leaving every other column `NotSet`.
///
/// `agentic_schedules` has exactly one true writer today —
/// `tick_monitor_schedules`'s CAS `UPDATE ... WHERE id = $2 AND
/// next_run_at = $3` — and several replicas run the tick loop concurrently
/// with no leader election precisely because that CAS makes it safe. Building
/// this `ActiveModel` from `row.clone().into()` would set every column from a
/// snapshot taken earlier in the tick and emit a full-row, unguarded `UPDATE`,
/// making this the first non-CAS writer on the table: a concurrent CAS fire
/// on the same row between the snapshot read and this write would have its
/// bookkeeping (`last_fired_at`, `last_run_id`, `missed_runs`,
/// `last_missed_at`) silently reverted. A targeted two-column update can
/// never race the CAS fire on any column it doesn't touch.
fn timezone_reconcile_active_model(
    row: &agentic_runtime::entity::schedule::Model,
    timezone: &str,
    next: chrono::DateTime<chrono::FixedOffset>,
) -> agentic_runtime::entity::schedule::ActiveModel {
    use agentic_runtime::entity::schedule;

    schedule::ActiveModel {
        id: sea_orm::ActiveValue::Set(row.id.clone()),
        timezone: sea_orm::ActiveValue::Set(timezone.to_string()),
        next_run_at: sea_orm::ActiveValue::Set(next),
        ..Default::default()
    }
}

/// Update the `timezone` (and recompute `next_run_at`) on any existing
/// `monitor_scan` row whose timezone differs from `timezone`.
///
/// Bootstrap is create-only, so without this reconcile every existing row
/// would be stranded on its original timezone and a `timezone:` edit in
/// `.monitor.yml` would silently never take effect. Cadence, the enabled
/// flag and variables are left exactly as they are — see
/// [`timezone_reconcile_active_model`] for why the update must stay
/// column-targeted. A row whose next fire time can't be recomputed for the
/// new timezone is logged and left alone, never half-updated — and counts as
/// settled, since no retry changes a cron the new timezone cannot schedule.
/// Returns `false` only when a write failed.
async fn reconcile_schedule_timezones(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    existing: &[agentic_runtime::entity::schedule::Model],
    timezone: &str,
) -> bool {
    let mut settled = true;
    for row in existing.iter().filter(|r| r.timezone != timezone) {
        let next = match agentic_pipeline::scheduler::next_after(
            &row.cron_expr,
            timezone,
            chrono::Utc::now(),
        ) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(
                    target: "metric_monitoring",
                    %workspace_id,
                    schedule_id = %row.id,
                    error = %e,
                    "reconcile: could not recompute next_run_at for the new timezone; leaving the row alone"
                );
                continue;
            }
        };
        let active = timezone_reconcile_active_model(row, timezone, next);
        match sea_orm::ActiveModelTrait::update(active, db).await {
            Ok(_) => tracing::info!(
                target: "metric_monitoring",
                %workspace_id,
                schedule_id = %row.id,
                from = %row.timezone,
                to = %timezone,
                "reconciled monitor_scan schedule timezone"
            ),
            Err(e) => {
                tracing::warn!(
                    target: "metric_monitoring",
                    %workspace_id,
                    schedule_id = %row.id,
                    error = %e,
                    "reconcile: failed to update monitor_scan schedule timezone"
                );
                settled = false;
            }
        }
    }
    settled
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A malformed cron in a promoted `.monitor.yml` reaches the insert as
    /// `ScheduleError::Invalid` (nothing upstream rejects it — this is the
    /// rejection the scheduler itself produces for the string) and must count
    /// as settled, or the workspace is never stamped and every 30 s claim
    /// re-reads and re-warns until the revision changes. A database error is
    /// the one failure a retry can fix.
    #[test]
    fn an_invalid_cron_settles_and_a_db_error_retries() {
        use agentic_pipeline::scheduler::ScheduleError;

        let rejected = agentic_runtime::cron::validate_cron("every morning", "UTC")
            .expect_err("a prose cadence is not a cron expression");
        assert!(
            create_error_settled(&ScheduleError::Invalid(rejected)),
            "an invalid cron is a property of the revision; retrying cannot fix it"
        );
        assert!(
            !create_error_settled(&ScheduleError::Db(sea_orm::DbErr::Custom(
                "connection reset".into()
            ))),
            "a database error retries on the next claim"
        );
    }

    #[test]
    fn schedule_timezone_comes_from_the_monitor_config() {
        let cfg: MonitorConfig = serde_yaml::from_str(
            "timezone: America/Los_Angeles\nmonitors:\n  - measure: a.b\n    time_dimension: a.t\n",
        )
        .unwrap();
        assert_eq!(desired_schedule_timezone(&cfg), "America/Los_Angeles");
    }

    #[test]
    fn schedule_timezone_defaults_to_utc() {
        let cfg: MonitorConfig =
            serde_yaml::from_str("monitors:\n  - measure: a.b\n    time_dimension: a.t\n").unwrap();
        assert_eq!(
            desired_schedule_timezone(&cfg),
            "UTC",
            "an existing config with no timezone must keep firing on the UTC cron"
        );
    }

    #[test]
    fn per_monitor_overrides_do_not_change_the_schedule_timezone() {
        // One schedule row drives every monitor at a granularity, so only the
        // file-level timezone can determine when it fires.
        let cfg: MonitorConfig = serde_yaml::from_str(
            "timezone: America/Los_Angeles\nmonitors:\n  - measure: a.b\n    time_dimension: a.t\n    timezone: Europe/Berlin\n",
        )
        .unwrap();
        assert_eq!(desired_schedule_timezone(&cfg), "America/Los_Angeles");
    }

    /// `agentic_schedules` has exactly one true writer today — the tick's CAS
    /// `UPDATE ... WHERE id = $2 AND next_run_at = $3` — and several replicas
    /// run the tick loop concurrently with no leader election *because* that
    /// CAS makes it safe. A reconcile built from `row.clone().into()` would
    /// set every column and emit a full-row, unguarded `UPDATE`, becoming a
    /// second, non-CAS writer that can silently revert a concurrent tick's
    /// fire bookkeeping. Pin the reconcile to a true two-column update: every
    /// field except `id` (the primary key, never emitted in `SET`),
    /// `timezone` and `next_run_at` must stay `NotSet`.
    #[test]
    fn timezone_reconcile_active_model_only_touches_timezone_and_next_run_at() {
        use agentic_runtime::entity::schedule;

        let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().fixed_offset();
        let row = schedule::Model {
            id: "sched-1".to_string(),
            workspace_id: uuid::Uuid::new_v4(),
            project_id: None,
            branch_id: None,
            name: "Metric monitoring (daily)".to_string(),
            target_kind: "monitor_scan".to_string(),
            target_ref: ".monitor.yml".to_string(),
            question: None,
            variables: Some(serde_json::json!({ "granularity": "day" })),
            cron_expr: "0 6 * * *".to_string(),
            timezone: "UTC".to_string(),
            enabled: true,
            next_run_at: now,
            // Bookkeeping a concurrent CAS fire on another replica could have
            // just written — must survive an unrelated timezone reconcile.
            last_fired_at: Some(now),
            last_run_id: Some("run-42".to_string()),
            last_error: None,
            missed_runs: 3,
            last_missed_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        let next = now + chrono::Duration::hours(1);

        let active = timezone_reconcile_active_model(&row, "America/Los_Angeles", next);

        assert!(
            active.id.is_set(),
            "primary key must be set to target the row"
        );
        assert!(active.timezone.is_set());
        assert!(active.next_run_at.is_set());

        assert!(active.workspace_id.is_not_set());
        assert!(active.project_id.is_not_set());
        assert!(active.branch_id.is_not_set());
        assert!(active.name.is_not_set());
        assert!(active.target_kind.is_not_set());
        assert!(active.target_ref.is_not_set());
        assert!(active.question.is_not_set());
        assert!(active.variables.is_not_set());
        assert!(active.cron_expr.is_not_set());
        assert!(active.enabled.is_not_set());
        assert!(
            active.last_fired_at.is_not_set(),
            "must not revert a concurrent tick's fire bookkeeping"
        );
        assert!(active.last_run_id.is_not_set());
        assert!(active.last_error.is_not_set());
        assert!(active.missed_runs.is_not_set());
        assert!(active.last_missed_at.is_not_set());
        assert!(active.created_at.is_not_set());
        assert!(active.updated_at.is_not_set());
    }
}
