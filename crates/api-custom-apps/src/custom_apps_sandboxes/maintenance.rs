//! The sandbox maintenance loop: a boot-time loop, shaped like
//! `previews::maintenance`, that expires idle sandboxes and retries teardowns
//! that did not finish. Apart from step 5 it only writes rows and queues tasks
//! — the teardown itself is a `custom_app_sandbox_teardown` task
//! ([`super::teardown`]) on the worker fleet — so it runs on any node, with or
//! without workers.
//!
//! Each pass ([`sweep`]):
//! 1. **Expire** up to [`SANDBOXES_PER_PASS`] active sandboxes whose last
//!    activity (`super::activity`) is more than [`super::idle_ttl`] ago: each
//!    is deleted exactly as a `DELETE` would (`ops::begin_delete`), with no
//!    actor and the reason `expired`.
//! 2. **End** the sandboxes of sandbox agent tokens that were revoked or
//!    expired more than a day ago ([`super::token_ended`]), reason
//!    `token_ended`.
//! 3. **Retry** up to [`SANDBOXES_PER_PASS`] sandboxes marked deleting more
//!    than [`stale_teardown`] ago: their teardown failed, or never got a
//!    worker. Deleting one again marks it now and queues a new run, so it is
//!    next retried a further [`stale_teardown`] later.
//! 4. **Expire past the fork** up to [`SANDBOXES_PER_PASS`] active sandboxes
//!    whose fork point (`fork_at`, else when they were created) is more than
//!    [`oxy_oltp::env_branches::EXPIRY_DAYS`] ago **and** that hold a copy of
//!    production rows ([`expires_from_fork`]), or whose Airhouse overlay still
//!    lists a table it copied ([`super::copies::holding_copies`]), reason `oltp_branch_expired`.
//!    Activity does not count: a copy of production data must not outlive its
//!    window because someone keeps using it, or because its plane was moved
//!    off the mode that copied it. Its teardown deletes the branch and the
//!    overlay.
//! 5. **Delete orphaned OLTP branches** — environment branches whose sandbox
//!    row is gone without a teardown, by a cascade from its app or owner
//!    ([`super::orphan_branches`]). Inline, not queued: no sandbox is left.
//! 6. **Drop OLTP branches no sandbox asks for** — a sandbox moved off
//!    `branch`/`empty` whose own `Drop` failed ([`super::unwanted_branches`]).
//!
//! **The list is a moment old when each sandbox's turn comes.** A publish
//! may have landed on an idle sandbox, or a stale teardown finished and its
//! name been created again. So each delete says why the sandbox was selected
//! (`ops::Expect`) and looks again under the sandbox's row lock: an expiry
//! only of one still idle, a retry only of the very row still stuck.
//!
//! **Safe across replicas.** A delete queues nothing for a sandbox whose
//! teardown is still queued or running (`ops::delete`), so two nodes sweeping
//! at once queue one teardown between them; and a teardown holds the
//! sandbox's lock for its whole run (`super::lock`).

use std::time::Duration;

use chrono::{DateTime, Utc};
use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use oxy_custom_apps::planes::{AirhouseMode, OltpMode, ResolvedPlanes};
use oxy_oltp::env_branches::EXPIRY_DAYS;
use sea_orm::sea_query::{Expr, Func};
use sea_orm::{
    ColumnTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Statement,
};
use uuid::Uuid;

use super::copies::{expires_row, holding_copies, overlay_key_of};
use super::delete::forked_at;
use super::ops::Expect;
use super::{SandboxError, TeardownReason, activity, ops};

pub const INTERVAL_ENV: &str = "OXY_APP_SANDBOX_MAINTENANCE_INTERVAL_SECS";
const DEFAULT_INTERVAL_SECS: u64 = 900;
const MIN_INTERVAL_SECS: u64 = 60;
/// Sandboxes expired, and teardowns retried, per pass; the next pass continues.
pub const SANDBOXES_PER_PASS: i64 = 50;
/// Rows the fork pass reads at a time while it looks for sandboxes that qualify.
const FORK_PAGE: u64 = 200;

/// How long a sandbox may stay `deleting` before its teardown is queued again.
pub fn stale_teardown() -> chrono::Duration {
    chrono::Duration::hours(6)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaintenanceConfig {
    pub interval: Duration,
}

impl MaintenanceConfig {
    /// `OXY_APP_SANDBOX_MAINTENANCE_INTERVAL_SECS`, default 900, at least 60.
    pub fn from_env() -> Self {
        let secs = std::env::var(INTERVAL_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_INTERVAL_SECS)
            .max(MIN_INTERVAL_SECS);
        Self {
            interval: Duration::from_secs(secs),
        }
    }
}

/// Spawn the detached loop. Pure DB work, so it runs on any node regardless
/// of `--no-workers`; the queued teardowns run wherever the worker fleet does.
pub fn spawn(config: MaintenanceConfig) {
    tokio::spawn(async move {
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(
                    ?e,
                    "sandbox_maintenance: DB connect failed; loop not started"
                );
                return;
            }
        };
        tracing::info!(
            interval_secs = config.interval.as_secs(),
            "sandbox_maintenance: started"
        );
        let mut tick = tokio::time::interval(config.interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await; // skip the startup storm, as previews::maintenance does
        loop {
            tick.tick().await;
            match sweep(&db, Utc::now()).await {
                Ok(0) => {}
                Ok(queued) => tracing::info!(queued, "sandbox_maintenance: teardowns queued"),
                Err(e) => tracing::warn!(error = %e, "sandbox_maintenance: sweep failed"),
            }
        }
    });
}

/// Whether a sandbox with `planes` holds a copy of production's rows, and so
/// is deleted [`EXPIRY_DAYS`] after its fork point.
///
/// **The one place this is decided.** An exhaustive match on the planes that
/// can copy rows, so a new mode has to choose. `empty` copies nothing (its
/// branch is cut without a provider expiry); OLTP `schema` is a schema of
/// the org's shared staging branch, which this pass does not own. Storage
/// never counts: an environment's silo starts empty and its objects expire
/// on their own.
pub fn expires_from_fork(planes: &ResolvedPlanes) -> bool {
    let oltp = match planes.oltp {
        OltpMode::Branch => true,
        OltpMode::Empty | OltpMode::Schema => false,
    };
    let airhouse = match planes.airhouse {
        // The overlay copies production's rows into the sandbox on write.
        AirhouseMode::Branch | AirhouseMode::Snapshot => true,
        AirhouseMode::WriteOnly => false,
    };
    oltp || airhouse
}

/// One pass at `now`: expire idle sandboxes, end those whose token ended,
/// retry stale teardowns, expire those past their fork, then delete the OLTP
/// branches no sandbox holds.
/// Answers how many teardowns it queued. A sandbox that cannot be deleted is
/// logged and left for the next pass; only a failed read fails the pass.
pub async fn sweep(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<usize, DbErr> {
    sweep_with(db, now, expires_from_fork).await
}

/// [`sweep`], with the rule for which planes expire from the fork passed in:
/// the seam the tests use to force it `true`.
pub async fn sweep_with(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
    expires: fn(&ResolvedPlanes) -> bool,
) -> Result<usize, DbErr> {
    let ttl = super::idle_ttl();
    let idle_since = Expect::IdleSince(now - ttl);
    let idle = activity::idle_sandboxes(db, now, ttl, SANDBOXES_PER_PASS)
        .await?
        .into_iter()
        .map(|(app_id, name)| (app_id, name, idle_since))
        .collect();
    let mut queued = delete_each(db, idle, TeardownReason::Expired).await;
    queued += super::token_ended::sweep(db, now).await?;
    let stale = stale_teardowns(db, now - stale_teardown(), SANDBOXES_PER_PASS).await?;
    queued += delete_each(db, stale, TeardownReason::Retried).await;
    let cutoff = now - chrono::Duration::days(EXPIRY_DAYS);
    let forked = forked_sandboxes(db, cutoff, SANDBOXES_PER_PASS as usize, expires).await?;
    queued += delete_each(db, forked, TeardownReason::BranchExpired).await;
    super::orphan_branches::sweep(db).await;
    queued += super::unwanted_branches::sweep(db, now).await;
    Ok(queued)
}

/// One sandbox the pass selected, and what must still be true of it when the
/// pass reaches it — the list is a moment old by then.
type Selected = (Uuid, String, Expect);

/// Up to `limit` sandboxes marked deleting before `marked_before`,
/// longest-waiting first — each with the row it was, so a sandbox created
/// again under the name is not taken for it.
async fn stale_teardowns(
    db: &DatabaseConnection,
    marked_before: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Selected>, DbErr> {
    use sea_orm::ConnectionTrait;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT app_id, name, created_at FROM app_environments \
              WHERE kind = 'dev' AND deleting_at < $1 \
              ORDER BY deleting_at LIMIT $2",
            [marked_before.into(), limit.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| {
            let created_at: DateTime<Utc> = row.try_get("", "created_at")?;
            let expect = Expect::Stale {
                created_at,
                marked_before,
            };
            Ok((row.try_get("", "app_id")?, row.try_get("", "name")?, expect))
        })
        .collect()
}

/// Up to `limit` active sandboxes forked before `cutoff` whose planes `expires`,
/// oldest fork first, each to be deleted only while still forked before it.
/// Read a page at a time: most old sandboxes copy nothing, and must not crowd
/// out those that do. A row whose planes do not parse is warned about and
/// skipped — never torn down on a choice nobody can read.
async fn forked_sandboxes(
    db: &DatabaseConnection,
    cutoff: DateTime<Utc>,
    limit: usize,
    expires: fn(&ResolvedPlanes) -> bool,
) -> Result<Vec<Selected>, DbErr> {
    use app_environments::Column;
    use sea_orm::sea_query::ExprTrait;
    let fork = || -> Expr {
        Func::coalesce([Expr::col(Column::ForkAt), Expr::col(Column::CreatedAt)]).into()
    };
    let mut selected = Vec::new();
    let mut after: Option<(DateTime<Utc>, Uuid, String)> = None;
    while selected.len() < limit {
        let mut query = app_environments::Entity::find()
            .filter(Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
            .filter(Column::DeletingAt.is_null())
            .filter(fork().lt(cutoff.fixed_offset()))
            .order_by_asc(fork())
            .order_by_asc(Column::AppId)
            .order_by_asc(Column::Name)
            .limit(FORK_PAGE);
        if let Some((forked, app_id, name)) = after.take() {
            let here = Expr::tuple([fork(), Expr::col(Column::AppId), Expr::col(Column::Name)]);
            let last = Expr::tuple([
                Expr::value(forked.fixed_offset()),
                Expr::value(app_id),
                Expr::value(name),
            ]);
            query = query.filter(here.gt(last));
        }
        let page = query.all(db).await?;
        let Some(last) = page.last() else { break };
        after = Some((forked_at(last), last.app_id, last.name.clone()));
        let full = page.len() as u64 == FORK_PAGE;
        let by_planes: Vec<bool> = page.iter().map(|row| expires_row(row, expires)).collect();
        let unexpired: Vec<String> = page
            .iter()
            .zip(&by_planes)
            .filter(|(_, by_planes)| !**by_planes)
            .filter_map(|(row, _)| overlay_key_of(row))
            .collect();
        let held = holding_copies(db, &unexpired).await?;
        selected.extend(
            page.iter()
                .zip(by_planes)
                .filter(|(row, by_planes)| {
                    *by_planes || overlay_key_of(row).is_some_and(|key| held.contains(&key))
                })
                .map(|(row, _)| row)
                .map(|row| (row.app_id, row.name.clone(), Expect::ForkedBefore(cutoff)))
                .take(limit - selected.len()),
        );
        if !full {
            break;
        }
    }
    Ok(selected)
}

/// `ops::delete_if` for each selected sandbox; how many teardowns were queued.
async fn delete_each(
    db: &DatabaseConnection,
    selected: Vec<Selected>,
    reason: TeardownReason,
) -> usize {
    let mut queued = 0;
    for (app_id, name, expect) in selected {
        match delete_one(db, app_id, &name, reason, expect).await {
            Ok(Some(deletion)) if deletion.queued => {
                tracing::info!(%app_id, environment = %name, run_id = %deletion.run_id,
                    reason = reason.as_str(), "sandbox_maintenance: teardown queued");
                queued += 1;
            }
            // Its teardown is already on its way — another replica's sweep, a
            // `DELETE`, or a run still waiting for a worker.
            Ok(Some(_)) => {}
            // It moved on since the pass selected it — published to, invoked,
            // or created again under the name: not this pass's to delete.
            Ok(None) => {
                tracing::debug!(%app_id, environment = %name, reason = reason.as_str(),
                    "sandbox_maintenance: no longer what the pass selected; left alone");
            }
            // Deleted, and torn down, between the read and now.
            Err(SandboxError::NotFound(_) | SandboxError::AppNotFound) => {}
            Err(e) => {
                tracing::warn!(%app_id, environment = %name, reason = reason.as_str(), error = %e,
                    "sandbox_maintenance: could not queue the teardown");
            }
        }
    }
    queued
}

async fn delete_one(
    db: &DatabaseConnection,
    app_id: Uuid,
    name: &str,
    reason: TeardownReason,
    expect: Expect,
) -> Result<Option<ops::Deletion>, SandboxError> {
    let environment =
        AppEnvironment::parse(name).ok_or_else(|| SandboxError::InvalidName(name.to_string()))?;
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|e| SandboxError::db("load the app", e))?
        .ok_or(SandboxError::AppNotFound)?;
    ops::delete_if(db, &app, &environment, None, reason, expect).await
}

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod tests;
