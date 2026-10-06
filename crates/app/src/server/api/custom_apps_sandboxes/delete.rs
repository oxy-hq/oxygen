//! The first half of deleting a sandbox: under its row lock, mark it
//! `deleting`, clear its pointer and queue its teardown — one transaction, so
//! a sandbox is never left serving while "deleted", nor marked with nothing
//! queued to finish the job. The second half is [`super::teardown`]'s.
//!
//! Two rules the lock makes enforceable:
//!
//! * **At most one teardown of a sandbox is on its way at a time.** A delete
//!   of a sandbox whose run is still queued or running answers that run.
//! * **The sweep's deletes look again.** The maintenance loop selected the
//!   sandbox a moment ago for a reason ([`Expect`]); a publish, an
//!   invocation, or a teardown and a re-create may have happened since. A
//!   delete that no longer finds its reason true writes nothing.

use chrono::{DateTime, SubsecRound, Utc};
use entity::{app_environments, apps};
use oxy_app_core::audit;
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect,
    TransactionTrait,
};
use uuid::Uuid;

use super::expiry_audit::expiry_entry;
use super::ops::{require_sandbox, scoped};
use super::teardown::{self, SandboxTeardownTask};
use super::{SandboxError, TeardownReason, activity};
use crate::server::api::custom_apps_environments::{self, EnvAction};

/// Mark the sandbox deleting, clear its pointer and queue its teardown;
/// answers the teardown's run id. See [`delete`].
///
/// `actor` is who asked — the request's user, with the key or token the
/// audit row names — and `None` for an expiry.
pub async fn begin_delete(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    actor: Option<&audit::RequestActor>,
    reason: TeardownReason,
) -> Result<String, SandboxError> {
    Ok(delete(db, app, environment, actor, reason).await?.run_id)
}

/// What a delete did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deletion {
    /// The teardown run to watch: the one this call queued, or the one
    /// already on its way.
    pub run_id: String,
    /// Whether this call queued it.
    pub queued: bool,
    /// Whether this call took the sandbox from active to deleting.
    pub was_active: bool,
}

/// What a delete must still find true once it holds the sandbox's row lock.
///
/// A person names a sandbox and means it, whatever state it is in. The sweep
/// read a list a moment ago, and the sandbox may have moved since: published
/// to, invoked, or torn down and created again under the same name. So the
/// sweep says why it chose the sandbox, and the delete looks again under the
/// lock before it writes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Someone asked for this sandbox by name.
    Any,
    /// The sweep's expiry: still active, and still idle since before the
    /// cutoff — no pointer move and no invocation after it.
    IdleSince(DateTime<Utc>),
    /// The sweep's retry: still the row that was created at `created_at`,
    /// and still marked deleting since before `marked_before` — never a
    /// sandbox created again under the name.
    Stale {
        created_at: DateTime<Utc>,
        marked_before: DateTime<Utc>,
    },
}

/// [`begin_delete`], saying what it did.
///
/// **At most one teardown of a sandbox is on its way at a time.** For a
/// sandbox already being deleted whose run is still queued or running, this
/// writes nothing and answers that run — a second `DELETE`, or a second
/// replica's sweep, never queues a second. Once that run has ended and the
/// row is still there (the teardown failed), this marks the row again and
/// queues a run of its own: how a `DELETE`, and the maintenance loop, retry.
pub async fn delete(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    actor: Option<&audit::RequestActor>,
    reason: TeardownReason,
) -> Result<Deletion, SandboxError> {
    let name = environment.name();
    delete_if(db, app, environment, actor, reason, Expect::Any)
        .await?
        .ok_or(SandboxError::NotFound(name))
}

/// [`delete`], only while `expect` still holds under the sandbox's row lock.
/// `None`: it no longer does, and nothing was written.
pub async fn delete_if(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    actor: Option<&audit::RequestActor>,
    reason: TeardownReason,
    expect: Expect,
) -> Result<Option<Deletion>, SandboxError> {
    require_sandbox(environment)?;
    let txn = db.begin().await.map_err(|e| SandboxError::db("begin", e))?;
    let asked_by = actor.map(|actor| actor.user.id);
    let deletion = mark_and_queue(&txn, app, environment, asked_by, reason, expect).await?;
    txn.commit()
        .await
        .map_err(|e| SandboxError::db("commit", e))?;
    if deletion
        .as_ref()
        .is_some_and(|deletion| deletion.was_active)
    {
        audit_deletion(db, app, environment, actor, reason).await;
    }
    Ok(deletion)
}

/// The `app.environment.deleted` audit row: written when a sandbox went from
/// active to deleting, never for a retry. An expiry's actor is the system's.
async fn audit_deletion(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    actor: Option<&audit::RequestActor>,
    reason: TeardownReason,
) {
    const ACTION: &str = "app.environment.deleted";
    let entry = match actor {
        Some(actor) => audit::AuditEntry::for_request(actor, ACTION),
        None => expiry_entry(ACTION),
    };
    let entry = scoped(entry, app, environment).reason(reason.as_str());
    audit::record_best_effort(db, entry).await;
}

/// Under a lock on the sandbox's row: leave it alone when `expect` no longer
/// holds (`None`), answer the run already on its way, or mark the row and
/// queue one.
async fn mark_and_queue<C: ConnectionTrait>(
    txn: &C,
    app: &apps::Model,
    environment: &AppEnvironment,
    actor: Option<Uuid>,
    reason: TeardownReason,
    expect: Expect,
) -> Result<Option<Deletion>, SandboxError> {
    let row = lock_sandbox(txn, app.id, &environment.name()).await?;
    if !still_expected(txn, &row, expect).await? {
        return Ok(None);
    }
    if let Some(run_id) = run_on_its_way(txn, &row).await? {
        return Ok(Some(Deletion {
            run_id,
            queued: false,
            was_active: false,
        }));
    }
    let was_active = row.deleting_at.is_none();
    if was_active {
        custom_apps_environments::record_move(
            txn,
            app.id,
            environment,
            None,
            EnvAction::Unpublish,
            actor,
        )
        .await
        .map_err(|e| SandboxError::db("clear the sandbox's pointer", e))?;
    }
    let run_id = mark_deleting_and_queue(txn, app, row.name, reason).await?;
    Ok(Some(Deletion {
        run_id,
        queued: true,
        was_active,
    }))
}

/// The sandbox's row, locked until the transaction ends.
async fn lock_sandbox<C: ConnectionTrait>(
    txn: &C,
    app_id: Uuid,
    name: &str,
) -> Result<app_environments::Model, SandboxError> {
    app_environments::Entity::find_by_id((app_id, name.to_string()))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .lock_exclusive()
        .one(txn)
        .await
        .map_err(|e| SandboxError::db("lock the sandbox", e))?
        .ok_or_else(|| SandboxError::NotFound(name.to_string()))
}

/// Whether `expect` still holds of the locked `row`.
async fn still_expected<C: ConnectionTrait>(
    txn: &C,
    row: &app_environments::Model,
    expect: Expect,
) -> Result<bool, SandboxError> {
    match expect {
        Expect::Any => Ok(true),
        Expect::IdleSince(cutoff) => {
            if row.deleting_at.is_some() {
                return Ok(false);
            }
            let invoked = activity::last_invocation(txn, row.app_id, &row.name)
                .await
                .map_err(|e| SandboxError::db("read the sandbox's last invocation", e))?;
            let updated_at = row.updated_at.with_timezone(&Utc);
            Ok(activity::last_activity(updated_at, invoked) < cutoff)
        }
        Expect::Stale {
            created_at,
            marked_before,
        } => Ok(row.created_at.with_timezone(&Utc) == created_at
            && row
                .deleting_at
                .is_some_and(|marked| marked.with_timezone(&Utc) < marked_before)),
    }
}

/// The teardown run queued when `row` was last marked, while the queue still
/// holds it waiting or running.
async fn run_on_its_way<C: ConnectionTrait>(
    txn: &C,
    row: &app_environments::Model,
) -> Result<Option<String>, SandboxError> {
    let Some(marked) = row.deleting_at else {
        return Ok(None);
    };
    let run_id = teardown::run_id_of(row.app_id, &row.name, marked.timestamp_micros());
    let pending = teardown::is_pending(txn, &run_id)
        .await
        .map_err(|e| SandboxError::db("read the teardown already queued", e))?;
    Ok(pending.then_some(run_id))
}

/// Mark the row deleting now, and queue the teardown that names that moment.
async fn mark_deleting_and_queue<C: ConnectionTrait>(
    txn: &C,
    app: &apps::Model,
    name: String,
    reason: TeardownReason,
) -> Result<String, SandboxError> {
    // Microseconds: what Postgres keeps, so the run id names the stored value.
    let marked_at = Utc::now().trunc_subsecs(6);
    app_environments::Entity::update_many()
        .col_expr(
            app_environments::Column::DeletingAt,
            Expr::value(marked_at.fixed_offset()),
        )
        .filter(app_environments::Column::AppId.eq(app.id))
        .filter(app_environments::Column::Name.eq(name.clone()))
        .exec(txn)
        .await
        .map_err(|e| SandboxError::db("mark the sandbox deleting", e))?;
    let task = SandboxTeardownTask {
        app_id: app.id,
        app_slug: app.slug.clone(),
        org_id: app.org_id,
        workspace_id: app.project_id,
        environment: name,
        reason: reason.as_str().to_string(),
        marked_at_micros: marked_at.timestamp_micros(),
    };
    teardown::enqueue(txn, &task)
        .await
        .map_err(|e| SandboxError::db("queue the teardown", e))
}
