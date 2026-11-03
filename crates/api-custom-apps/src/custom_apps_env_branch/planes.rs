//! What a change of an environment's `planes` does to its OLTP branch: the
//! task it queues, in the same transaction as the write that asked for it
//! (`POST …/environments`, `PUT …/environments/{name}/planes`).
//!
//! - **into `branch` or `empty`** from anything else: a `Cut` at the
//!   sandbox's fork point — or a `Reset` from the parent as it is now when a row
//!   already exists (a drop that has not run yet, or the other branch mode),
//!   which re-cuts in the new mode (`home_for` refuses a branch cut in the mode the planes no longer
//!   ask for);
//! - **out of them**: a `Drop`, so the provider branch and its row go now and
//!   do not wait for the provider's expiry;
//! - **no change of the OLTP plane**: nothing.

use chrono::{DateTime, Utc};
use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::entity::env_branches::EnvBranchMode;
use oxy_oltp::env_branches::{check_fork_at, find_for_app};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;

use super::queue::{BranchOp, EnvBranchTask, enqueue_in};
use super::{wants_branch, wants_branch_in};

/// The op a change of the OLTP plane from `before` to `after` queues, given
/// whether the environment has a branch row now and the fork point `at` to
/// cut or re-cut at. `None`: nothing to do.
pub(crate) fn op_for_change(
    before: Option<EnvBranchMode>,
    after: Option<EnvBranchMode>,
    has_row: bool,
    at: Option<DateTime<Utc>>,
) -> Option<BranchOp> {
    match (before == after, after, has_row) {
        (true, _, _) | (false, None, false) => None,
        (false, None, true) => Some(BranchOp::Drop),
        (false, Some(_), true) => Some(BranchOp::Reset { at }),
        (false, Some(_), false) => Some(BranchOp::Cut { at }),
    }
}

/// Queue the task `row`'s environment needs now that its planes were
/// `before_planes` and are `row.planes`: the run's id, or `None` when the OLTP
/// plane did not change in a way that needs one. Call it with the write's
/// transaction as `conn`.
///
/// **Only a sandbox has an OLTP branch**: a change of the OLTP plane of any
/// other environment is refused here, not left to the caller's validation.
/// A cut or re-cut also writes the sandbox's `fork_at` — the point it cuts at,
/// else now — in the same transaction, so the expiry pass (`maintenance`)
/// counts the branch from when it was asked for, whether or not the task ever
/// gets to write it.
pub(crate) async fn queue_for_change<C: ConnectionTrait>(
    conn: &C,
    app: &apps::Model,
    row: &app_environments::Model,
    before_planes: &Value,
) -> Result<Option<String>, DbErr> {
    let (before, after) = (wants_branch_in(before_planes), wants_branch(row));
    if before == after {
        return Ok(None);
    }
    if !matches!(
        AppEnvironment::parse(&row.name),
        Some(AppEnvironment::Dev { .. })
    ) {
        return Err(DbErr::Custom(format!(
            "{} is not a sandbox: only a sandbox has an OLTP branch",
            row.name
        )));
    }
    let has_row = find_for_app(conn, app.id, &row.name).await?.is_some();
    // A cut forks at the sandbox's recorded point. A re-cut of a branch that
    // exists is a fresh copy: the recorded point is the old cut's, not the
    // instant being asked for.
    let at = match (after, has_row) {
        (Some(_), false) => fork_point(conn, row).await?,
        _ => None,
    };
    let Some(op) = op_for_change(before, after, has_row, at) else {
        return Ok(None);
    };
    if matches!(op, BranchOp::Cut { .. } | BranchOp::Reset { .. }) {
        record_fork_at(conn, app.id, &row.name, at.unwrap_or_else(Utc::now)).await?;
    }
    let task = EnvBranchTask {
        app_id: app.id,
        app_slug: app.slug.clone(),
        org_id: app.org_id,
        workspace_id: app.project_id,
        environment: row.name.clone(),
        op,
    };
    enqueue_in(conn, &task).await.map(Some)
}

/// The point to cut `row`'s environment at: its recorded `fork_at` while the
/// database still keeps that far back. A sandbox whose planes change days after
/// it was created with an explicit `--at` cannot be cut there any more; it is
/// cut now, and its `fork_at` says so — the expiry clock and `env show` must
/// describe the branch that exists.
pub(crate) async fn fork_point<C: ConnectionTrait>(
    conn: &C,
    row: &app_environments::Model,
) -> Result<Option<DateTime<Utc>>, DbErr> {
    let Some(at) = row.fork_at.map(|at| at.with_timezone(&Utc)) else {
        return Ok(None);
    };
    let now = Utc::now();
    if check_fork_at(at, now).is_ok() {
        return Ok(Some(at));
    }
    record_fork_at(conn, row.app_id, &row.name, now).await?;
    Ok(None)
}

/// Write `at` as the environment's fork point: what the expiry pass
/// (`maintenance`) and `env show` read. A reset moves it, so a sandbox that is
/// reset is not torn down for the age of its first cut.
pub(crate) async fn record_fork_at<C: ConnectionTrait>(
    conn: &C,
    app_id: uuid::Uuid,
    name: &str,
    at: DateTime<Utc>,
) -> Result<(), DbErr> {
    app_environments::Entity::update_many()
        .col_expr(
            app_environments::Column::ForkAt,
            Expr::value(at.fixed_offset()),
        )
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(name))
        .exec(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    const B: Option<EnvBranchMode> = Some(EnvBranchMode::Branch);
    const E: Option<EnvBranchMode> = Some(EnvBranchMode::Empty);

    fn at() -> Option<DateTime<Utc>> {
        Some(Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap())
    }

    #[test]
    fn becoming_a_branch_cuts_or_recuts_at_the_fork_point() {
        for mode in [B, E] {
            assert_eq!(
                op_for_change(None, mode, false, at()),
                Some(BranchOp::Cut { at: at() })
            );
            assert_eq!(
                op_for_change(None, mode, true, at()),
                Some(BranchOp::Reset { at: at() }),
                "a leftover row is re-cut, not trusted"
            );
        }
    }

    #[test]
    fn switching_between_branch_and_empty_resets_or_cuts() {
        assert_eq!(
            op_for_change(B, E, true, None),
            Some(BranchOp::Reset { at: None })
        );
        assert_eq!(
            op_for_change(E, B, true, None),
            Some(BranchOp::Reset { at: None })
        );
        assert_eq!(
            op_for_change(B, E, false, at()),
            Some(BranchOp::Cut { at: at() }),
            "no row to reset"
        );
    }

    #[test]
    fn leaving_the_branch_modes_drops_the_branch_when_there_is_one() {
        for mode in [B, E] {
            assert_eq!(op_for_change(mode, None, true, None), Some(BranchOp::Drop));
            assert_eq!(op_for_change(mode, None, false, None), None);
        }
    }

    #[test]
    fn an_unchanged_oltp_plane_queues_nothing() {
        for mode in [None, B, E] {
            for has_row in [false, true] {
                assert_eq!(op_for_change(mode, mode, has_row, at()), None);
            }
        }
    }
}
