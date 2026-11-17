//! The four-eyes rule (`apps.require_second_person_to_promote`): with it on,
//! production refuses a **promote** by the identity that published the build.
//!
//! An identity is what a build records about its publisher and what a move
//! knows about its mover: a user (`app_builds.published_by`, the move's
//! `actor`) and a machine identity (`app_builds.published_via`, the move's
//! `actor_via`) — a trusted publisher's workflow, or the name of the CI token
//! a service account acted through. Two identities are the same when both
//! name a user and it is the same user, or, when either names none, both name
//! the same machine identity. So a person promotes nothing they published, a
//! CI token's service account promotes nothing it published (Make Live, which
//! a publish token may call), and a promoting publish — whose mover is, by
//! construction, its build's publisher — never moves production while the rule
//! is on.
//!
//! **A rollback is exempt only to a build production served before**: that is
//! the emergency path, and nothing production has not already run reaches it.
//! The console's rollback (`rollback_app`) takes any build staging served, so
//! a rollback to a build production never served is a promote by another
//! name, and its publisher is refused it like any promote. (`oxyc rollback`
//! names only builds production served, ruling 6, so it is always exempt.) A
//! move that re-stamps the build production already serves (the console's
//! Re-publish) is exempt too: nothing new reaches production.
//!
//! Checked twice, like invariant 1: by the plan (a dry run lists
//! `same_person_promote` beside the other blockers, and a promoting publish's
//! gate refuses with it), and again by the one production function — before
//! it applies any migration, and under the app's row lock — where every writer
//! of production's pointer passes.

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::token::StoredKind;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::blockers::{same_person_promote, same_person_rollback};
use super::error::PromoteError;
use super::production::ProductionMove;
use crate::custom_apps_build_history as history;
use crate::custom_apps_environments::EnvAction;

/// Who published a build, or who moves production.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub user: Option<Uuid>,
    /// The machine identity, in `app_builds.published_via`'s shape.
    pub via: Option<String>,
}

impl Identity {
    /// The publisher a build recorded.
    pub fn published(build: &app_builds::Model) -> Self {
        Self::new(build.published_by, build.published_via.clone())
    }

    /// A request's user, and the machine identity its credential carries.
    pub fn of_user(user: &AuthenticatedUser) -> Self {
        Self::new(Some(user.id), via_of(user))
    }

    /// The mover a production move names.
    pub fn of_move(mv: &ProductionMove) -> Self {
        Self::new(mv.actor, mv.actor_via.clone())
    }

    /// The nil user is the machine principal's placeholder, never a person.
    pub fn new(user: Option<Uuid>, via: Option<String>) -> Self {
        Self {
            user: user.filter(|id| !id.is_nil()),
            via: via.filter(|v| !v.is_empty()),
        }
    }

    /// The same identity (module docs): the same user when both name one,
    /// else the same machine identity. Nobody is the same as nobody.
    pub fn is(&self, other: &Identity) -> bool {
        match (self.user, other.user) {
            (Some(a), Some(b)) => a == b,
            _ => matches!((&self.via, &other.via), (Some(a), Some(b)) if a == b),
        }
    }
}

/// The machine identity a request's credential carries, as a publish records
/// it: a trusted-access (`ci`) token's name. `None` for a person's session,
/// key or token.
pub fn via_of(user: &AuthenticatedUser) -> Option<String> {
    user.credential
        .as_ref()
        .filter(|c| c.kind == StoredKind::Ci)
        .map(|c| c.name.clone())
}

/// What the rule asks of `mv` before anything is read: `None` when it has
/// nothing to say — the locked `row` does not ask for a second person, the
/// mover did not publish `build`, or the move re-stamps the build production
/// serves (`previous`, what it served before the move) — else the move's
/// action, which decides.
pub fn asks(
    row: &apps::Model,
    build: &app_builds::Model,
    mv: &ProductionMove,
    previous: Option<Uuid>,
) -> Option<EnvAction> {
    let restamp = previous == Some(build.id);
    let same = Identity::published(build).is(&Identity::of_move(mv));
    (row.require_second_person_to_promote && same && !restamp).then_some(mv.action)
}

/// Refuse `mv` when the rule asks a second person for it ([`asks`]): a
/// promote always, a rollback when production never served `build`.
pub async fn refuse_same_person<C: ConnectionTrait>(
    conn: &C,
    row: &apps::Model,
    build: &app_builds::Model,
    mv: &ProductionMove,
    previous: Option<Uuid>,
) -> Result<(), PromoteError> {
    let refused = match asks(row, build, mv, previous) {
        Some(EnvAction::Promote) => same_person_promote(&build.build_id),
        Some(EnvAction::Rollback) if !served_production(conn, build).await? => {
            same_person_rollback(&build.build_id)
        }
        _ => return Ok(()),
    };
    Err(PromoteError::Blocked(vec![refused]))
}

/// Production has served `build` at some point: a `production` event names it.
async fn served_production<C: ConnectionTrait>(
    conn: &C,
    build: &app_builds::Model,
) -> Result<bool, PromoteError> {
    history::has_served(conn, build.app_id, &AppEnvironment::Production, build.id)
        .await
        .map_err(|e| PromoteError::internal("read production's history", e))
}

#[cfg(test)]
#[path = "second_person_tests.rs"]
mod tests;
