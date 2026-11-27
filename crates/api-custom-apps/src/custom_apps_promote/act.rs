//! A promote or a rollback, from resolved request to answered plan
//! (environments design §5.5a, rulings 7, 9, 10 and 11).
//!
//! 1. Moving an environment to the build it serves is `409 already_serving`,
//!    and to a build a sandbox agent token published `409
//!    draft_published_by_agent` — a dry run included: there is nothing to plan.
//! 2. The plan is read. With `dry_run` it is the answer.
//! 3. Otherwise the act refuses `409 target_moved` when the moving
//!    environment no longer serves what the plan saw — `expect_current` when
//!    the request sends it back, else what this request read — and `409` with
//!    the first blocker's code when the plan lists any.
//! 4. The pointer moves: production through the one production function
//!    (`production`, migrations first), staging through `staging` (its
//!    migrations queued after). Production's schedules are then reconciled to
//!    the build's manifest.
//! 5. One audit row: `app.promoted` / `app.rolled_back`, the moved
//!    environment, metadata `{from, build, previous}`.
//!
//! A promote to production may carry `override_checks` (`checks_override`):
//! decided before the plan is read, shown in the plan's `checks`, and carried
//! by the move, which audits it.
//!
//! **One rollback has no request behind it**: the promote checks' automatic
//! rollback of production ([`rollback_unattended`]). It is this same path,
//! from the plan to the move, with the platform as the mover ([`Mover`]): no
//! user on the move, and an audit row of its own (`unattended_audit`).

use oxy_app_core::audit::{self, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_server_authz::Caller;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::audit::promoted_entry;
use super::blockers::Act;
use super::checks_override::{self, GateAsk};
use super::error::PromoteError;
use super::plan::{self, Plan};
use super::production::{self, ProductionMove, describe};
use super::resolve::{self, PromoteRequest, Resolved, RollbackRequest};
use super::schedules;
use super::second_person::{self, Identity};
use super::staging::{self, StagingMove};
use super::unattended_audit::{self, Unattended};
use crate::app_events::{gate, subscriptions};
use crate::custom_apps_auth;
use crate::custom_apps_environments::EnvAction;

/// Who moves an environment.
enum Mover<'a> {
    /// A request's actor: a person, a key or a token.
    Request(&'a RequestActor),
    /// The platform itself ([`rollback_unattended`]).
    Platform(&'a Unattended),
}

impl Mover<'_> {
    /// The identity the four-eyes rule compares with a build's publisher. The
    /// platform is nobody: it published nothing.
    fn identity(&self) -> Identity {
        match self {
            Self::Request(actor) => Identity::of_user(&actor.user),
            Self::Platform(_) => Identity::default(),
        }
    }

    /// The user a pointer move records; none for the platform.
    fn user_id(&self) -> Option<Uuid> {
        match self {
            Self::Request(actor) => Some(actor.user.id),
            Self::Platform(_) => None,
        }
    }

    fn via(&self) -> Option<String> {
        match self {
            Self::Request(actor) => second_person::via_of(&actor.user),
            Self::Platform(_) => None,
        }
    }
}

/// What the request asks of the act, beside what it names.
struct Options<'a> {
    dry_run: bool,
    confirmed: &'a [String],
    expect_current: Option<Option<Uuid>>,
    break_listeners: bool,
    /// What the request asks of the promote checks gate.
    checks: GateAsk,
}

pub async fn promote(
    db: &DatabaseConnection,
    actor: &RequestActor,
    app_id: Uuid,
    to: &str,
    req: &PromoteRequest,
) -> Result<Plan, PromoteError> {
    let caller = Caller::from_user(&actor.user);
    let resolved = resolve::resolve_promote(db, &caller, app_id, to, req).await?;
    // Only a promote to production meets the checks: staging's move never blocks.
    let checks = match resolved.to {
        AppEnvironment::Production => {
            // The route refuses a publish token before it gets here.
            let asked = req.override_checks.as_ref();
            checks_override::ask(db, actor, false, &resolved.app, asked).await?
        }
        _ => GateAsk::default(),
    };
    let opts = Options {
        dry_run: req.dry_run,
        confirmed: &req.confirm_schedule_removals,
        expect_current: req.expect_current,
        break_listeners: req.break_listeners,
        checks,
    };
    execute(db, &Mover::Request(actor), resolved, opts).await
}

pub async fn rollback(
    db: &DatabaseConnection,
    actor: &RequestActor,
    app_id: Uuid,
    name: &str,
    req: &RollbackRequest,
) -> Result<Plan, PromoteError> {
    let resolved = resolve::resolve_rollback(db, app_id, name, req).await?;
    let opts = Options {
        dry_run: req.dry_run,
        confirmed: &req.confirm_schedule_removals,
        expect_current: req.expect_current,
        // A rollback is never refused: nothing to override.
        break_listeners: false,
        // Nor held by the checks: the emergency path.
        checks: GateAsk::default(),
    };
    execute(db, &Mover::Request(actor), resolved, opts).await
}

/// Roll production back from `serving` to the build it served before, with no
/// request behind the move: the promote checks' automatic rollback
/// (`custom_apps_build_checks::production`). The same rollback `oxyc rollback
/// production` makes, with three differences:
///
/// - it moves only while production still serves `serving` (`409
///   target_moved` otherwise), so it never undoes a move someone made since;
/// - the schedules it retires are confirmed by the move itself: they are the
///   ones the build being rolled away from registered, and nobody is there to
///   name them;
/// - the mover is the platform, and the audit row is `why`'s.
///
/// Every other blocker and refusal of a rollback stands.
pub async fn rollback_unattended(
    db: &DatabaseConnection,
    app_id: Uuid,
    serving: Uuid,
    why: &Unattended,
) -> Result<Plan, PromoteError> {
    let production = AppEnvironment::Production.name();
    let req = RollbackRequest {
        expect_current: Some(Some(serving)),
        ..RollbackRequest::default()
    };
    let resolved = resolve::resolve_rollback(db, app_id, &production, &req).await?;
    let retired = schedules::diff(db, &resolved.app, &resolved.build)
        .await?
        .removed_functions();
    let opts = Options {
        dry_run: false,
        confirmed: &retired,
        expect_current: req.expect_current,
        break_listeners: false,
        checks: GateAsk::default(),
    };
    execute(db, &Mover::Platform(why), resolved, opts).await
}

async fn execute(
    db: &DatabaseConnection,
    mover: &Mover<'_>,
    r: Resolved,
    opts: Options<'_>,
) -> Result<Plan, PromoteError> {
    if r.current == Some(r.build.id) {
        return Err(PromoteError::AlreadyServing {
            environment: r.to.name(),
            build: r.build.build_id.clone(),
        });
    }
    // A build a sandbox agent token published is refused before it is
    // planned, as every promote path refuses it — and again by the move.
    production::refuse_agent_built(db, &r.build).await?;
    let identity = mover.identity();
    let asked = plan::Asked {
        confirmed: opts.confirmed,
        mover: &identity,
        // A dry run previews the event gate; a real act leaves it to the move.
        break_listeners: opts.dry_run.then_some(opts.break_listeners),
        checks: &opts.checks,
    };
    let mut plan = plan::build(db, &r, asked).await?;
    if opts.dry_run {
        return Ok(plan);
    }
    let expected = opts.expect_current.unwrap_or(r.current);
    if expected != r.current {
        return Err(PromoteError::TargetMoved {
            environment: r.to.name(),
            expected: describe(expected),
            actual: describe(r.current),
        });
    }
    if !plan.blockers.is_empty() {
        return Err(PromoteError::Blocked(plan.blockers));
    }
    let warnings = move_pointer(db, mover, &r, (expected, &opts), &mut plan).await?;
    plan.warnings.extend(warnings);
    let rolled_over: &[String] = match r.act {
        Act::Rollback => &plan.broken_listeners,
        Act::Promote => &[],
    };
    record(db, mover, &r, rolled_over).await;
    // Only a request promotes; the platform's one move is a rollback.
    if let Mover::Request(actor) = mover
        && r.act == Act::Promote
        && !plan.broken_listeners.is_empty()
    {
        gate::audit_override(db, actor, &r.app, &r.build.build_id, &plan.broken_listeners).await;
    }
    plan.dry_run = false;
    plan.moved = true;
    Ok(plan)
}

/// Step 4; the warnings are the reconcile's or the queue's.
async fn move_pointer(
    db: &DatabaseConnection,
    mover: &Mover<'_>,
    r: &Resolved,
    (expected, opts): (Option<Uuid>, &Options<'_>),
    plan: &mut Plan,
) -> Result<Vec<String>, PromoteError> {
    let action = match r.act {
        Act::Promote => EnvAction::Promote,
        Act::Rollback => EnvAction::Rollback,
    };
    if r.to != AppEnvironment::Production {
        let mv = StagingMove {
            app_id: r.app.id,
            build: r.build.id,
            action,
            actor: mover.user_id(),
            expect_current: Some(expected),
        };
        staging::move_staging(db, &mv).await?;
        let mut warnings = staging::queue_staging_migrations(db, &r.app, &r.build).await;
        warnings.extend(staging::reconcile_staging_schedules(db, &r.app).await);
        warnings.extend(staging::reconcile_staging_subscriptions(db, &r.app).await);
        // Staging serves the build now: its checks are queued (once per build).
        warnings.extend(
            crate::custom_apps_build_checks::queue::after_staging_move_of(db, &r.app, &r.build)
                .await,
        );
        return Ok(warnings);
    }
    let mv = ProductionMove {
        app_id: r.app.id,
        build: r.build.id.into(),
        action,
        actor: mover.user_id(),
        actor_via: mover.via(),
        expect_current: Some(expected),
        refuse_same: true,
        // The request's flag: the move is the one place the event gate runs.
        break_listeners: opts.break_listeners,
        // The request's override, if it carried one that was accepted.
        checks: opts.checks.ask.clone(),
    };
    let moved = production::move_production(db, &mv).await?;
    // `published_at` is part of who may open the app.
    custom_apps_auth::invalidate_access_cache();
    plan.migrations_applied = Some(moved.applied);
    // What the move found: a promote's overridden refusals, a rollback's
    // warnings. Both are recorded (`execute`).
    plan.warnings.extend(moved.warnings.iter().cloned());
    plan.broken_listeners = match r.act {
        Act::Promote => moved.broken_listeners,
        Act::Rollback => moved.warnings,
    };
    let mut warnings = schedules::reconcile(db, &moved.app, &r.build).await;
    warnings.extend(subscriptions::reconcile_production(db, &moved.app, &r.build).await);
    // Production's own check of the build it now serves, when it was not queued.
    warnings.extend(moved.after);
    Ok(warnings)
}

/// Step 5, best effort: the move stands either way.
async fn record(db: &DatabaseConnection, mover: &Mover<'_>, r: &Resolved, broken: &[String]) {
    let (moved, builds) = ((&r.app, &r.to), (r.build.id, r.current));
    let actor = match mover {
        Mover::Request(actor) => actor,
        Mover::Platform(why) => {
            let entry = unattended_audit::entry(why, moved, builds, broken);
            return audit::record_best_effort(db, entry).await;
        }
    };
    let from = r.from.as_ref().map(AppEnvironment::name);
    record_move(db, actor, r.act, moved, from, builds, broken).await;
}

/// The audit row of one pointer move by promote or rollback — also the
/// console rollback's (`rollback_app`). `moved` is `(app, environment)`,
/// `builds` is `(build, previous)`; `broken` is what a rollback broke, which
/// its row records (spec §6.8) — empty for a promote, whose override has a row
/// of its own.
pub async fn record_move(
    db: &DatabaseConnection,
    actor: &RequestActor,
    act: Act,
    moved: (&entity::apps::Model, &AppEnvironment),
    from: Option<String>,
    builds: (Uuid, Option<Uuid>),
    broken: &[String],
) {
    let entry = promoted_entry(actor, act, moved, from, builds, broken);
    audit::record_best_effort(db, entry).await;
}
