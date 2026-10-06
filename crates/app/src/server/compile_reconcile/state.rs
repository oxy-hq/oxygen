//! `workspace_compile_checks`: who is due, and the claim that makes a check
//! happen once however many replicas tick.
//!
//! Every statement uses the database's clock, never this process's: the claim
//! compares a timestamp the database wrote with one it is about to write, and
//! two replicas with skewed clocks must still agree on who is due.

use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

/// A check that has come due.
#[derive(Debug, Clone)]
pub(super) struct Due {
    pub workspace_id: Uuid,
    /// The value the claim must still find, or another replica has it.
    pub next_check_at: DateTimeWithTimeZone,
    /// The branch head the last answered check saw.
    pub last_head_sha: Option<String>,
}

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Give every workspace that can be checked a row, due at a random point in
/// the next `interval_secs`. The random phase is the jitter: later checks are
/// one interval after the last, so workspaces stay spread instead of all
/// asking GitHub in the same second.
pub(super) async fn seed(db: &DatabaseConnection, interval_secs: i64) -> Result<u64, DbErr> {
    let seeded = db
        .execute_raw(statement(
            "INSERT INTO workspace_compile_checks (workspace_id, next_check_at) \
             SELECT w.id, now() + random() * $1::bigint * interval '1 second' \
             FROM workspaces w \
             WHERE w.git_remote_url IS NOT NULL \
               AND COALESCE(w.default_branch, '') <> '' \
               AND w.status = 'ready' \
             ON CONFLICT (workspace_id) DO NOTHING",
            vec![interval_secs.into()],
        ))
        .await?;
    Ok(seeded.rows_affected())
}

/// The checks that are due, oldest first, at most `limit`.
pub(super) async fn due(db: &DatabaseConnection, limit: i64) -> Result<Vec<Due>, DbErr> {
    let rows = db
        .query_all_raw(statement(
            "SELECT workspace_id, next_check_at, last_head_sha \
             FROM workspace_compile_checks \
             WHERE next_check_at <= now() \
             ORDER BY next_check_at \
             LIMIT $1",
            vec![limit.into()],
        ))
        .await?;
    rows.iter()
        .map(|row| {
            Ok(Due {
                workspace_id: row.try_get("", "workspace_id")?,
                next_check_at: row.try_get("", "next_check_at")?,
                last_head_sha: row.try_get("", "last_head_sha")?,
            })
        })
        .collect()
}

/// Take `due` for this replica by moving its next check one interval out —
/// only if nobody has moved it since it was read. `true` means this replica
/// owns the check; `false` means another one does, or the write failed.
pub(super) async fn claim(db: &DatabaseConnection, due: &Due, interval_secs: i64) -> bool {
    db.execute_raw(statement(
        "UPDATE workspace_compile_checks \
         SET next_check_at = now() + $1::bigint * interval '1 second' \
         WHERE workspace_id = $2 AND next_check_at = $3",
        vec![
            interval_secs.into(),
            due.workspace_id.into(),
            due.next_check_at.into(),
        ],
    ))
    .await
    .map(|r| r.rows_affected() == 1)
    .unwrap_or(false)
}

/// Write down what a check found. `backoff_secs` pushes the next check out
/// further than the claim already did; it never pulls it in.
pub(super) async fn record(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    outcome: &str,
    head: Option<&str>,
    backoff_secs: i64,
) {
    let written = db
        .execute_raw(statement(
            "UPDATE workspace_compile_checks \
             SET last_checked_at = now(), \
                 last_outcome = $2, \
                 last_head_sha = COALESCE($3, last_head_sha), \
                 next_check_at = GREATEST(next_check_at, now() + $4::bigint * interval '1 second') \
             WHERE workspace_id = $1",
            vec![
                workspace_id.into(),
                outcome.into(),
                head.map(str::to_string).into(),
                backoff_secs.into(),
            ],
        ))
        .await;
    if let Err(e) = written {
        tracing::warn!(%workspace_id, error = %e, "compile reconcile: recording a check failed");
    }
}
