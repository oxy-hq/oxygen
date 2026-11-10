//! Failing streaks: the failed runs of one automation or pipeline that belong
//! together, so a source that fails every hour is told once, then daily.
//!
//! A **source** is one automation or pipeline file in one workspace:
//! (workspace, source type, `workflow_ref` / `pipeline_ref`). Its **history**
//! is its root runs that are the customer's own work (`customer_run_sql`: no
//! preview's run) and not cancels. `done` ends a streak; `failed` and
//! `timed_out` extend it; a cancel is neither, so it cannot start or break
//! one. A run's **streak** is every failure of its source created after the
//! latest success created before it.
//!
//! Derived from `agentic_runs` and `notifications` when asked. No table
//! records a streak, and the previous deploy reads every notice as it always
//! did. The one schema change is additive: `idx_agentic_runs_root_source_file`
//! (agentic-runtime's `AddRunSourceFileIndex`), so reading a source's history
//! costs its own runs, not every finished run in the workspace.

use std::collections::HashMap;

use agentic_runtime::entity::run;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use entity::notifications;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseTransaction, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, QuerySelect, Statement,
};
use uuid::Uuid;

use super::kinds::PlatformNotice;
use super::subjects::run_record;

/// How long a streak stays quiet after a person was told about it. A source
/// still failing after that reminds them, at most once per this long.
pub const REMINDER_AFTER: Duration = Duration::hours(24);

/// One automation or pipeline file in one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub workspace_id: Uuid,
    pub source_type: String,
    /// The file it runs: `pipeline_ref` or `workflow_ref`.
    pub file: String,
}

impl Source {
    /// The source of an automation or Airway run. `None` for a run that names
    /// no file: with nothing to group it by, each of its failures is told.
    pub fn of(row: &run::Model) -> Option<Self> {
        Some(Self {
            workspace_id: row.workspace_id,
            source_type: row.source_type.clone()?,
            file: run_record(row)?.file?,
        })
    }

    /// Serialize every failure decision about this source until the
    /// transaction ends, so two pods finishing two of its runs at once cannot
    /// both decide theirs is the first. Keyed on a 64-bit hash: a collision
    /// only makes two unrelated decisions wait for each other.
    pub async fn lock(&self, txn: &DatabaseTransaction) -> Result<(), DbErr> {
        txn.execute_raw(Statement::from_sql_and_values(
            txn.get_database_backend(),
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            [self.lock_key().into()],
        ))
        .await?;
        Ok(())
    }

    fn lock_key(&self) -> String {
        format!(
            "run-streak|{}|{}|{}",
            self.workspace_id, self.source_type, self.file
        )
    }
}

/// How long a run's source had been failing when the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Streak {
    /// Its failures since it last succeeded, through this run.
    pub failures: u32,
    /// When the first of them started.
    pub since: DateTime<Utc>,
}

/// The failed runs of `run_id`'s streak, including any created after it: a
/// run that failed alongside it is the same streak, whichever pod decides
/// first.
pub async fn streak_of(conn: &impl ConnectionTrait, run_id: &str) -> Result<Vec<String>, DbErr> {
    Ok(failures(conn, vec![run_id.to_string()], false)
        .await?
        .into_iter()
        .map(|f| f.id)
        .collect())
}

/// Which of `people` were told about a run in `streak` after `after`.
pub async fn told_after(
    conn: &impl ConnectionTrait,
    notice: &PlatformNotice,
    streak: Vec<String>,
    people: &[Uuid],
    after: DateTime<Utc>,
) -> Result<Vec<Uuid>, DbErr> {
    if streak.is_empty() || people.is_empty() {
        return Ok(Vec::new());
    }
    notifications::Entity::find()
        .select_only()
        .column(notifications::Column::UserId)
        .distinct()
        .filter(notifications::Column::UserId.is_in(people.iter().copied()))
        .filter(notifications::Column::OrgId.eq(notice.org_id()))
        .filter(notifications::Column::Kind.eq(notice.kind()))
        .filter(notifications::Column::SubjectKind.eq(notice.subject().kind))
        .filter(notifications::Column::SubjectId.is_in(streak))
        .filter(notifications::Column::CreatedAt.gt(after.fixed_offset()))
        .into_tuple::<Uuid>()
        .all(conn)
        .await
}

/// Each of `run_ids`' streak through that run, for the words. A run whose
/// source names no file, or that no longer exists, is absent.
pub async fn through(
    conn: &impl ConnectionTrait,
    run_ids: &[String],
) -> Result<HashMap<String, Streak>, DbErr> {
    if run_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut out: HashMap<String, Streak> = HashMap::new();
    for f in failures(conn, run_ids.to_vec(), true).await? {
        let at = f.created_at.with_timezone(&Utc);
        let streak = out.entry(f.target).or_insert(Streak {
            failures: 0,
            since: at,
        });
        streak.failures += 1;
        streak.since = streak.since.min(at);
    }
    Ok(out)
}

/// A failure in the streak of `target`.
#[derive(Debug, FromQueryResult)]
struct Failure {
    target: String,
    id: String,
    created_at: DateTime<FixedOffset>,
}

/// The failures in the streak of each of `targets`: through the target, or
/// after it too.
async fn failures(
    conn: &impl ConnectionTrait,
    targets: Vec<String>,
    through_target: bool,
) -> Result<Vec<Failure>, DbErr> {
    Failure::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        failures_sql(through_target),
        [targets.into()],
    ))
    .all(conn)
    .await
}

/// The file a run of `alias` ran, as [`run_record`] reads it. Written out
/// with literal source types, exactly as the key of
/// `idx_agentic_runs_root_source_file` (`RUN_SOURCE_FILE_INDEX_SQL`): with a
/// bound parameter in its place the planner could not match the index.
fn file_sql(alias: &str) -> String {
    use agentic_pipeline::{AIRWAY_SOURCE_TYPE, WORKFLOW_SOURCE_TYPE};
    format!(
        "CASE {alias}source_type WHEN '{AIRWAY_SOURCE_TYPE}' THEN {alias}metadata->>'pipeline_ref' \
         WHEN '{WORKFLOW_SOURCE_TYPE}' THEN {alias}metadata->>'workflow_ref' END"
    )
}

/// The index's predicate, restated so the planner can prove the history read
/// implies it: the join's `r.source_type = s.source_type` does not.
fn root_source_sql(alias: &str) -> String {
    use agentic_pipeline::{AIRWAY_SOURCE_TYPE, WORKFLOW_SOURCE_TYPE};
    format!(
        "{alias}parent_run_id IS NULL \
         AND {alias}source_type IN ('{AIRWAY_SOURCE_TYPE}', '{WORKFLOW_SOURCE_TYPE}')"
    )
}

/// `$1` the target run ids. Each distinct source's history is read once,
/// through `idx_agentic_runs_root_source_file` (its runs only, in time
/// order), however many of its runs a page of the inbox names; each target
/// then picks its streak out of it.
fn failures_sql(through_target: bool) -> String {
    format!(
        "WITH t AS ( \
           SELECT id, workspace_id, source_type, created_at, {t_file} AS file \
           FROM agentic_runs WHERE id = ANY($1) \
         ), sources AS ( \
           SELECT DISTINCT workspace_id, source_type, file FROM t WHERE file IS NOT NULL \
         ), history AS ( \
           SELECT s.workspace_id, s.source_type, s.file, r.id, r.created_at, r.task_status \
           FROM sources s JOIN agentic_runs r \
             ON r.workspace_id = s.workspace_id AND r.source_type = s.source_type \
            AND {r_file} = s.file \
           WHERE {root_source} \
             AND r.task_status IN ('done', 'failed', 'timed_out') \
             AND r.cancel_requested_at IS NULL \
             AND r.error_message IS DISTINCT FROM 'cancelled by user' \
             AND {customer} \
         ), last_success AS ( \
           SELECT t.id AS target, max(h.created_at) AS at \
           FROM t JOIN history h USING (workspace_id, source_type, file) \
           WHERE h.task_status = 'done' AND h.created_at < t.created_at \
           GROUP BY t.id \
         ) \
         SELECT t.id AS target, h.id, h.created_at \
         FROM t JOIN history h USING (workspace_id, source_type, file) \
         LEFT JOIN last_success s ON s.target = t.id \
         WHERE h.task_status <> 'done' AND (s.at IS NULL OR h.created_at > s.at){bound}",
        t_file = file_sql(""),
        r_file = file_sql("r."),
        root_source = root_source_sql("r."),
        customer = agentic_runtime::crud::customer_run_sql("r."),
        bound = if through_target {
            " AND h.created_at <= t.created_at"
        } else {
            ""
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(source: &str, metadata: serde_json::Value) -> run::Model {
        let now = Utc::now().fixed_offset();
        run::Model {
            id: "r-1".into(),
            question: String::new(),
            answer: None,
            error_message: None,
            thread_id: None,
            source_type: Some(source.into()),
            metadata: Some(metadata),
            parent_run_id: None,
            schedule_id: None,
            task_status: Some("failed".into()),
            task_metadata: None,
            attempt: 0,
            recovery_requested_at: None,
            driver_id: None,
            driver_heartbeat_at: None,
            cancel_requested_at: None,
            workspace_id: Uuid::nil(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn a_source_is_the_file_its_kind_of_run_names() {
        let airway = Source::of(&row(
            agentic_pipeline::AIRWAY_SOURCE_TYPE,
            serde_json::json!({ "pipeline_ref": "p.airway.yml", "workflow_ref": "x" }),
        ))
        .unwrap();
        assert_eq!(airway.file, "p.airway.yml");
        let automation = Source::of(&row(
            agentic_pipeline::WORKFLOW_SOURCE_TYPE,
            serde_json::json!({ "workflow_ref": "a.automation.yml" }),
        ))
        .unwrap();
        assert_eq!(automation.file, "a.automation.yml");
        assert_ne!(
            airway.lock_key(),
            automation.lock_key(),
            "two sources never share a lock"
        );
    }

    #[test]
    fn a_run_naming_no_file_has_no_source() {
        let no_ref = row(agentic_pipeline::AIRWAY_SOURCE_TYPE, serde_json::json!({}));
        assert_eq!(Source::of(&no_ref), None);
        let analytics = row("analytics", serde_json::json!({ "workflow_ref": "a" }));
        assert_eq!(Source::of(&analytics), None);
    }

    /// Postgres uses `idx_agentic_runs_root_source_file` only when the history
    /// read writes its key expression and implies its predicate. A change on
    /// either side silently returns the read to a scan of every finished run
    /// in the workspace; the fix is a new migration, not an edit to the
    /// shipped one.
    #[test]
    fn the_history_read_matches_its_index() {
        let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let index = squash(agentic_runtime::migration::RUN_SOURCE_FILE_INDEX_SQL);
        assert!(
            index.contains(&format!("({})", squash(&file_sql("")))),
            "key expression drifted: {index}"
        );
        assert!(
            index.ends_with(&format!("WHERE {}", squash(&root_source_sql("")))),
            "predicate drifted: {index}"
        );
        let sql = squash(&failures_sql(false));
        assert!(sql.contains(&squash(&file_sql("r."))));
        assert!(sql.contains(&squash(&root_source_sql("r."))));
    }
}
