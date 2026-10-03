//! Grant scope for `/admin/internal-jobs/*`.
//!
//! A queue row has no org column. Its tenant is reached through the run it belongs
//! to — `agentic_task_queue.run_id → agentic_runs.workspace_id → workspaces.org_id` —
//! and the task's payload, question and error are that tenant's. `operate_platform`
//! is held by every Global Admin whatever their bound and the capability gate cannot
//! see scope, so unfenced a grant bounded to one org read (and could re-enqueue or
//! delete) every other tenant's tasks.
//!
//! The rule, the same one `admin::scope` states for every staff surface:
//!
//! * a **list or a count** is narrowed in the query to tasks whose run's workspace
//!   belongs to an org the grant names;
//! * a task named **by id** outside that set answers the same `task_not_found` a
//!   missing one does;
//! * a task with **no org** — a system job, or a run with no workspace — is
//!   platform-level: unbounded grants and the Global Owner only;
//! * a **fleet-wide action** (the reaper, the retention sweep) acts on every tenant
//!   at once, so it is refused for a bounded grant.
//!
//! Kept out of `internal_jobs.rs` so that file does not grow, and so the whole rule
//! for this surface reads in one place.

use axum::http::StatusCode;
use axum::response::Response;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement, Value};
use uuid::Uuid;

use super::internal_jobs::{db_err, error_body, not_found_row};
use super::scope;

/// The caller's listing scope, as this surface's error type.
pub(super) async fn listing_scope(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
) -> Result<Option<Vec<Uuid>>, Response> {
    scope::list_scope(db, actor).await.map_err(unreadable)
}

/// ` AND <run_col> IN (<runs of the grant's orgs>)` for a bounded grant; empty for an
/// unbounded one. For the queue queries that do not already join `workspaces` — the
/// enriched feeds do, and take `scope::org_scope_clause("w.org_id", ..)` directly.
///
/// An inner join on purpose: a run with no workspace, or a workspace with no org, is
/// not in the subquery, so its task is invisible to a bounded grant.
pub(super) fn run_scope_clause(
    run_col: &str,
    scope: Option<&[Uuid]>,
    values: &mut Vec<Value>,
) -> String {
    let in_scope = scope::org_scope_clause("sw.org_id", scope, values);
    if in_scope.is_empty() {
        return String::new();
    }
    format!(
        " AND {run_col} IN (SELECT sr.id FROM agentic_runs sr \
           JOIN workspaces sw ON sw.id = sr.workspace_id WHERE TRUE{in_scope})"
    )
}

#[derive(Debug, FromQueryResult)]
struct TaskOrgRow {
    org_id: Option<Uuid>,
}

/// Refuse when the caller's grant does not reach the task named by `task_id`.
///
/// Out of scope answers exactly what a missing task does (`404 task_not_found`), so a
/// bounded grant cannot probe the queue for another tenant's task ids.
pub(super) async fn deny_out_of_scope_task(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
    task_id: &str,
) -> Result<(), Response> {
    let task = TaskOrgRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT w.org_id AS org_id \
         FROM agentic_task_queue q \
         LEFT JOIN agentic_runs r ON q.run_id = r.id \
         LEFT JOIN workspaces w ON r.workspace_id = w.id \
         WHERE q.task_id = $1",
        [task_id.into()],
    ))
    .one(db)
    .await
    .map_err(db_err)?
    .ok_or_else(not_found_row)?;
    scope::deny_out_of_scope_opt(db, actor, task.org_id)
        .await
        .map_err(|status| match status {
            StatusCode::NOT_FOUND => not_found_row(),
            _ => unreadable(status),
        })
}

/// Refuse a fleet-wide action for a bounded grant.
pub(super) async fn deny_out_of_scope_fleet(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
) -> Result<(), Response> {
    scope::deny_out_of_scope_platform(db, actor)
        .await
        .map_err(|status| match status {
            StatusCode::NOT_FOUND => error_body(status, "not_found", None),
            _ => unreadable(status),
        })
}

fn unreadable(status: StatusCode) -> Response {
    error_body(
        status,
        "scope_unreadable",
        Some("platform grant could not be read".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unbounded_grant_adds_no_predicate_and_binds_nothing() {
        let mut values: Vec<Value> = vec![1i64.into()];
        assert_eq!(run_scope_clause("q.run_id", None, &mut values), "");
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn a_bounded_grant_binds_its_orgs_after_the_fixed_parameters() {
        let org = Uuid::new_v4();
        let mut values: Vec<Value> = vec![1i64.into(), 2i64.into()];
        let clause = run_scope_clause("q.run_id", Some(&[org]), &mut values);
        assert!(clause.starts_with(" AND q.run_id IN (SELECT sr.id FROM agentic_runs sr"));
        assert!(
            clause.contains("sw.org_id = ANY($3)"),
            "the org array is the third bound value: {clause}"
        );
        assert_eq!(values.len(), 3);
    }

    /// An empty scope is a real answer — a grant bounded to nothing — and must still
    /// produce the predicate, or "reaches nothing" would list everything.
    #[test]
    fn an_empty_scope_still_narrows() {
        let mut values: Vec<Value> = Vec::new();
        let clause = run_scope_clause("run_id", Some(&[]), &mut values);
        assert!(clause.contains("ANY($1)"));
        assert_eq!(values.len(), 1);
    }
}
