//! **The one function that moves production's pointer** (environments design
//! §5.5a, rulings 3 and 4). Every writer goes through it: `oxyc promote` and
//! `oxyc rollback`, the console's *Make Live* (`admin::apps::ops::publish_one`,
//! which the partner console and batch publish call too), the console's
//! rollback (`rollback_app`), *promote latest*, and a promoting publish
//! (`custom_apps_publish::set_pointers`, through [`move_locked`] after it has
//! recorded its own staging event in the same transaction).
//!
//! In order:
//!
//! 1. **Admit, unlocked** — so nothing is applied for a build that cannot go
//!    live: a build a sandbox agent token published is `409
//!    draft_published_by_agent`; one staging never served is
//!    `staging_never_served` (invariant 1: *production only ever serves a
//!    build staging has served*, its evidence any `staging` event for the
//!    build); one whose validation is not `passed` is
//!    `validation_not_passed`; a promote by the build's own publisher, while
//!    the app requires a second person, is `same_person_promote` — and so is
//!    their rollback to a build production never served (`second_person`).
//! 2. **Migrations, before the pointer** — the build's pending OLTP files,
//!    then its Airhouse files, re-read from the stored bundle and applied
//!    through `apply_on_promote` / `apply_airhouse_on_promote` unchanged. A
//!    failure leaves the pointer where it was and is answered with the
//!    migration's own status. A file already applied is a no-op, so a
//!    rollback to a build whose files all ran applies nothing; nothing is ever
//!    reversed.
//! 3. **Move, under the app's row lock** — in one transaction holding
//!    `apps` `FOR UPDATE`, everything in 1 is asked again of the database as
//!    it is now — and, for a promote, the app's four-eyes rule
//!    (`second_person`), read from the locked row — then production's pointer
//!    moves (`custom_apps_environments::record_move`, the one writer of it)
//!    with the app's `published_at` and promotion stamps.

use chrono::Utc;
use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait, QuerySelect,
    TransactionTrait,
};
use serde::Serialize;
use uuid::Uuid;

use super::audit::{MoveAudit, MoveRow, promoted_entry, published_entry};
use super::blockers::{Act, staging_never_served, validation_not_passed};
use super::error::PromoteError;
use super::second_person;
use super::target::BuildRef;
use crate::custom_apps_agent_built::AgentBuilt;
use crate::custom_apps_build_state::{BuildState, PromoteRefusal};
use crate::custom_apps_environments::{self, EnvAction};
use crate::{
    custom_apps_build_history as history, custom_apps_cache, custom_apps_migrations as migrations,
};

/// One move of production's pointer.
#[derive(Debug, Clone)]
pub struct ProductionMove {
    pub app_id: Uuid,
    /// The build production moves to: an `app_builds.id` (`.into()`), or the
    /// draft / newest build, resolved under the row lock (`target`).
    pub build: BuildRef,
    /// `Promote` or `Rollback`: what the `production` event records.
    pub action: EnvAction,
    pub actor: Option<Uuid>,
    /// The machine identity moving production, beside `actor`: a trusted
    /// publisher's workflow or a CI token's name, as `app_builds.published_via`
    /// records one. With `actor`, what the four-eyes rule compares against
    /// the build's publisher (`second_person`).
    pub actor_via: Option<String>,
    /// `Some(b)`: refuse `409 target_moved` unless production still serves
    /// `b` (`None` inside: serves nothing) when the row is locked.
    pub expect_current: Option<Option<Uuid>>,
    /// Refuse `409 already_serving` for the build production already serves.
    /// Off for the console's *Make Live*, whose "Re-publish" re-stamps the
    /// live build.
    pub refuse_same: bool,
}

/// The files a production move applied, per store.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct MigrationsApplied {
    pub oltp: Vec<String>,
    pub airhouse: Vec<String>,
}

/// A move that happened: the app row as it now is, and what it applied.
#[derive(Debug)]
pub struct Moved {
    pub app: apps::Model,
    pub applied: MigrationsApplied,
}

/// How many times a move starts over for a build its name came to mean
/// something else while it admitted and migrated the previous one.
const MAX_RESOLVES: usize = 3;

/// Admit, migrate, then move under the row lock (module docs). Drops the
/// serve path's resolution cache once committed.
///
/// A move aimed at the draft or the newest build ([`BuildRef`]) names it again
/// under the lock. When that is not the build it admitted — a draft publish
/// landed meanwhile — the transaction is dropped and the move starts over, so
/// the newer build is admitted and migrated like any other and wins.
pub async fn move_production(
    db: &DatabaseConnection,
    mv: &ProductionMove,
) -> Result<Moved, PromoteError> {
    move_production_audited(db, mv, None).await
}

/// [`move_production`] that also writes `audit`'s row (`audit`) in the move's
/// transaction: a refused move leaves no row, and a row that cannot be
/// written leaves no move.
pub async fn move_production_audited(
    db: &DatabaseConnection,
    mv: &ProductionMove,
    audit: Option<&MoveAudit<'_>>,
) -> Result<Moved, PromoteError> {
    let mut last = None;
    for _ in 0..MAX_RESOLVES {
        let app = apps::Entity::find_by_id(mv.app_id)
            .one(db)
            .await
            .map_err(|e| PromoteError::internal("load the app", e))?
            .ok_or(PromoteError::AppNotFound)?;
        let build = mv.build.resolve(db, &app).await?;
        let previous = check_pointer(db, &app, &build, mv).await?;
        admissible(db, &build).await?;
        second_person::refuse_same_person(db, &app, &build, mv, previous).await?;
        let applied = apply_migrations(db, &app, &build).await?;
        let txn = db
            .begin()
            .await
            .map_err(|e| PromoteError::internal("begin the move", e))?;
        let row = lock_app(&txn, mv.app_id).await?;
        let named = mv.build.resolve(&txn, &row).await?;
        if named.id != build.id {
            last = Some((build.id, named.id));
            continue;
        }
        let app = move_resolved(&txn, row, mv, named, audit).await?;
        txn.commit()
            .await
            .map_err(|e| PromoteError::internal("commit the move", e))?;
        custom_apps_cache::invalidate_app_resolution_cache();
        return Ok(Moved { app, applied });
    }
    let (expected, actual) = last.unwrap_or_default();
    Err(mv.build.moved(expected, actual))
}

/// The app's row, held `FOR UPDATE` until the caller's transaction ends.
pub async fn lock_app<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
) -> Result<apps::Model, PromoteError> {
    apps::Entity::find_by_id(app_id)
        .lock_exclusive()
        .one(conn)
        .await
        .map_err(|e| PromoteError::internal("lock the app", e))?
        .ok_or(PromoteError::AppNotFound)
}

/// Step 3 alone, in the caller's transaction with `row` already locked
/// ([`lock_app`]): ask everything again, then move. A promoting publish calls
/// this after recording its own staging event in the same transaction, which
/// is the evidence invariant 1 then finds.
pub async fn move_locked<C: ConnectionTrait>(
    conn: &C,
    row: apps::Model,
    mv: &ProductionMove,
) -> Result<apps::Model, PromoteError> {
    let build = mv.build.resolve(conn, &row).await?;
    move_resolved(conn, row, mv, build, None).await
}

/// [`move_locked`] once the locked `row` has named its `build`.
async fn move_resolved<C: ConnectionTrait>(
    conn: &C,
    row: apps::Model,
    mv: &ProductionMove,
    build: app_builds::Model,
    audit: Option<&MoveAudit<'_>>,
) -> Result<apps::Model, PromoteError> {
    let previous = check_pointer(conn, &row, &build, mv).await?;
    admissible(conn, &build).await?;
    second_person::refuse_same_person(conn, &row, &build, mv, previous).await?;
    let now = Utc::now().fixed_offset();
    let mut active: apps::ActiveModel = row.into();
    active.published_at = ActiveValue::Set(Some(now));
    active.last_promoted_by = ActiveValue::Set(mv.actor);
    active.last_promoted_at = ActiveValue::Set(Some(now));
    active.updated_at = ActiveValue::Set(now);
    let updated = active
        .update(conn)
        .await
        .map_err(|e| PromoteError::internal("move production's pointer", e))?;
    custom_apps_environments::record_move(
        conn,
        mv.app_id,
        &AppEnvironment::Production,
        Some(build.id),
        mv.action,
        mv.actor,
    )
    .await
    .map_err(|e| PromoteError::internal("record production's event", e))?;
    if let Some(audit) = audit {
        let entry = match audit.row {
            MoveRow::Published => published_entry(audit.actor, &updated),
            MoveRow::Promoted => promoted_entry(
                audit.actor,
                Act::Promote,
                (&updated, &AppEnvironment::Production),
                None,
                (build.id, previous),
            ),
        };
        oxy_app_core::audit::record_in_txn(conn, entry)
            .await
            .map_err(|e| PromoteError::internal("record the audit row", e))?;
    }
    Ok(updated)
}

/// The build `build_pk` of `app_id`; another app's build is no build of this one.
pub async fn load_build<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
    build_pk: Uuid,
) -> Result<app_builds::Model, PromoteError> {
    app_builds::Entity::find_by_id(build_pk)
        .one(conn)
        .await
        .map_err(|e| PromoteError::internal("load the build", e))?
        .filter(|build| build.app_id == app_id)
        .ok_or_else(|| PromoteError::BuildNotFound(build_pk.to_string()))
}

/// `target_moved` and `already_serving`, against the pointer production's row
/// holds as `conn` sees it: for a move's final answer, `conn` is the
/// transaction holding `row` locked. Answers that pointer: the build
/// production serves before the move.
async fn check_pointer<C: ConnectionTrait>(
    conn: &C,
    row: &apps::Model,
    build: &app_builds::Model,
    mv: &ProductionMove,
) -> Result<Option<Uuid>, PromoteError> {
    let current = custom_apps_environments::pointer(conn, row.id, &AppEnvironment::Production)
        .await
        .map_err(|e| PromoteError::internal("read production's pointer", e))?;
    if let Some(expected) = mv.expect_current
        && expected != current
    {
        return Err(PromoteError::TargetMoved {
            environment: AppEnvironment::Production.name(),
            expected: describe(expected),
            actual: describe(current),
        });
    }
    if mv.refuse_same && current == Some(build.id) {
        return Err(PromoteError::AlreadyServing {
            environment: AppEnvironment::Production.name(),
            build: build.build_id.clone(),
        });
    }
    Ok(current)
}

/// A pointer, for a sentence: its build id, or `nothing`.
pub fn describe(pointer: Option<Uuid>) -> String {
    pointer.map_or_else(|| "nothing".to_string(), |b| b.to_string())
}

/// May production serve `build` at all: not a build a sandbox agent token
/// published, and — every blocker it fails, listed — one staging has served
/// whose validation passed.
pub async fn admissible<C: ConnectionTrait>(
    conn: &C,
    build: &app_builds::Model,
) -> Result<(), PromoteError> {
    let unvalidated = match BuildState::of(build).promotable() {
        Ok(()) => None,
        Err(PromoteRefusal::AgentBuilt { token_id }) => {
            return Err(agent_built_refusal(conn, build, token_id).await);
        }
        Err(PromoteRefusal::ValidationNotPassed { status }) => Some(status),
    };
    let mut blockers = Vec::new();
    let staging = AppEnvironment::Staging;
    let served = history::has_served(conn, build.app_id, &staging, build.id)
        .await
        .map_err(|e| PromoteError::internal("read staging's history", e))?;
    if !served {
        blockers.push(staging_never_served(&build.build_id));
    }
    if let Some(status) = unvalidated {
        blockers.push(validation_not_passed(&build.build_id, &status));
    }
    if blockers.is_empty() {
        Ok(())
    } else {
        Err(PromoteError::Blocked(blockers))
    }
}

/// `409 draft_published_by_agent` when a sandbox agent token published
/// `build` — the refusal every promote path makes before it plans or moves.
pub async fn refuse_agent_built<C: ConnectionTrait>(
    conn: &C,
    build: &app_builds::Model,
) -> Result<(), PromoteError> {
    match BuildState::of(build).provenance.check() {
        Err(PromoteRefusal::AgentBuilt { token_id }) => {
            Err(agent_built_refusal(conn, build, token_id).await)
        }
        _ => Ok(()),
    }
}

/// The refusal for a build `token_id` published, with the token's name, its
/// minter and where the build went.
async fn agent_built_refusal<C: ConnectionTrait>(
    conn: &C,
    build: &app_builds::Model,
    token_id: Uuid,
) -> PromoteError {
    match AgentBuilt::describe(conn, build, token_id).await {
        Ok(built) => built.into(),
        Err(e) => PromoteError::internal("read the build's publisher", e),
    }
}

/// Step 2: the build's pending files, OLTP then Airhouse, applied to
/// production before the pointer moves.
async fn apply_migrations(
    db: &DatabaseConnection,
    app: &apps::Model,
    build: &app_builds::Model,
) -> Result<MigrationsApplied, PromoteError> {
    let declared = migrations::declared_in_build(build, &app.slug).await?;
    let oltp =
        migrations::apply_on_promote(db, app.id, &app.slug, app.org_id, build.id, &declared.oltp)
            .await?;
    let airhouse = migrations::apply_airhouse_on_promote(
        db,
        app.id,
        &app.slug,
        app.project_id,
        build.id,
        &declared.airhouse,
    )
    .await?;
    for summary in [oltp.summary(), airhouse.summary()] {
        if !summary.is_empty() {
            tracing::info!(app_id = %app.id, build = %build.build_id, "promote: {summary}");
        }
    }
    Ok(MigrationsApplied {
        oltp: oltp.applied,
        airhouse: airhouse.applied,
    })
}
