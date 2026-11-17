//! Scope for a row that belongs to a **person**: the user directory.
//!
//! [`scope`] fences a row by its org. A user has none — they are a member of zero
//! or more orgs — so whether a bounded grant reaches one is a question about those
//! memberships, and it has three answers:
//!
//! * **seeing** a user (the directory, the detail pane) needs *one* org of theirs in
//!   reach, and of that user shows only the rows those orgs own;
//! * **adding them to an org** needs what seeing them needs, so a grant cannot add
//!   someone to an org and so make them its own;
//! * **changing their status** needs *every* org of theirs in reach: `deleted` refuses
//!   the person on every protected route and at every login, in all of them at once.
//!
//! A user in no org is platform-level — an all-orgs grant and the Global Owner only —
//! the answer [`scope::deny_out_of_scope_opt`] gives any org-less row. Out of
//! reach is the same 404 a missing user gets.
//!
//! A status change has a second fence past scope ([`may_change_status`]): the Global
//! Owner cannot be locked out from the console, and a target who holds staff standing is
//! behind the delegation bound of their grant row. Suspending a peer Global Admin is
//! demoting them by other means, which is exactly what that bound refuses.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use entity::org_members;
use entity::users::{self, UserStatus};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{DelegationDenial, PrincipalFacts};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use uuid::Uuid;

use crate::server::authz::{Caller, globals};
use oxy_server_authz::staff::stored_grant::{StoredGrant, delegatable, stored_grant_of};
use oxy_server_authz::staff::{delegation, scope};

/// Does a caller whose scope is `scope` (`None` = unbounded) reach **every** org of a
/// user who is a member of `member_orgs`? What changing that user's status takes. A user
/// in no org is in no bounded scope.
pub(super) fn reaches_all_of(scope: Option<&[Uuid]>, member_orgs: &[Uuid]) -> bool {
    scope.is_none_or(|orgs| {
        !member_orgs.is_empty() && member_orgs.iter().all(|org| orgs.contains(org))
    })
}

/// Does a caller whose scope is `scope` reach **one** org of a user who is a member of
/// `member_orgs`? What seeing that user takes.
pub(super) fn reaches_one_of(scope: Option<&[Uuid]>, member_orgs: &[Uuid]) -> bool {
    scope.is_none_or(|orgs| member_orgs.iter().any(|org| orgs.contains(org)))
}

/// May a caller whose scope is `scope` see a row owned by `org`? An org-less row is
/// platform-level, so only an unbounded caller does.
pub(super) fn row_in_scope(scope: Option<&[Uuid]>, org: Option<Uuid>) -> bool {
    scope.is_none_or(|orgs| org.is_some_and(|o| orgs.contains(&o)))
}

/// The caller's reach over the user directory: `None` unbounded, `Some(orgs)` the orgs
/// whose members it may see.
///
/// The Global Owner passes on the `OXY_OWNER` allow-list before any grant is read, as
/// every by-id fence in [`scope`] does; everyone else is [`scope::list_scope`],
/// so an unreadable grant is a 500 and no standing reaches nobody.
pub(super) async fn user_scope(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
) -> Result<Option<Vec<Uuid>>, StatusCode> {
    if globals::is_global_owner(&Caller::from_user(actor)) {
        return Ok(None);
    }
    scope::list_scope(db, actor).await
}

/// The orgs `user_id` is a member of, in id order. `db` is a transaction when a write
/// decides again under its lock.
pub(super) async fn member_orgs_of<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<Vec<Uuid>, DbErr> {
    org_members::Entity::find()
        .select_only()
        .column(org_members::Column::OrgId)
        .filter(org_members::Column::UserId.eq(user_id))
        .order_by_asc(org_members::Column::OrgId)
        .into_tuple()
        .all(db)
        .await
}

fn unreadable(what: &'static str) -> impl FnOnce(DbErr) -> StatusCode {
    move |e| {
        tracing::error!(target: "authz", error = %e, "{what} unreadable fencing a user");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// 404 when the caller's grant does not reach `user_id` by `reaches` (`reaches_*_of`);
/// otherwise the caller's scope, for narrowing the rows returned about that user.
pub(super) async fn deny_user_out_of_scope(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
    user_id: Uuid,
    reaches: fn(Option<&[Uuid]>, &[Uuid]) -> bool,
) -> Result<Option<Vec<Uuid>>, StatusCode> {
    let scope = user_scope(db, actor).await?;
    if let Some(orgs) = scope.as_deref() {
        let member_orgs = member_orgs_of(db, user_id)
            .await
            .map_err(unreadable("memberships"))?;
        if !reaches(Some(orgs), &member_orgs) {
            return Err(StatusCode::NOT_FOUND);
        }
    }
    Ok(scope)
}

/// 404 when a bounded caller would put `target` into an org without already reaching
/// them. Otherwise a grant could make its own reach: add a user who is in no org to one
/// of its orgs, and that user is now theirs to suspend ([`reaches_all_of`]).
///
/// So a bounded grant adds only someone it can already see: a member of one of its
/// orgs ([`reaches_one_of`]). A pending invitation is not reach, because the grant can
/// write one itself (an assume session into an org in scope is that org's Owner, and an
/// Owner may invite any address). Someone invited joins by accepting. A frontline worker
/// or a service account holds no `org_members` row, so neither qualifies. An unbounded
/// caller and the Global Owner pass, as before.
pub(super) async fn deny_addition_out_of_scope(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
    target: &users::Model,
) -> Result<(), StatusCode> {
    deny_user_out_of_scope(db, actor, target.id, reaches_one_of)
        .await
        .map(|_| ())
}

/// The target of a status change, as [`may_change_status`] needs it.
pub(super) struct StatusTarget {
    pub is_global_owner: bool,
    /// Their platform grant as stored, if they hold one.
    pub grant: Option<StoredGrant>,
}

/// Why a status change was refused, past scope.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum StatusRefusal {
    /// The target is the Global Owner and the change would lock them out.
    GlobalOwner,
    /// The target's grant is not one the actor may act on.
    Delegation(DelegationDenial),
}

/// May `actor` set `target`'s status to `to`? Scope is decided first, by
/// [`deny_user_out_of_scope`] with [`reaches_all_of`].
///
/// * **Root is never locked out.** Its standing is the `OXY_OWNER` allow-list, which a
///   console write cannot change, so suspending the account only strands the address
///   the allow-list names. Reactivating it is allowed: that restores what the
///   allow-list already grants, and is the way back for an account suspended before
///   this fence existed.
/// * **A staff target is behind their grant row's delegation bound**, both ways —
///   reactivating a Global Admin the Owner suspended undoes the Owner's call. It goes
///   through `delegatable`, the one decision over a stored grant, so a role this build
///   cannot name is undelegatable for everyone but the owner here as well.
pub(super) fn may_change_status(
    actor: &PrincipalFacts,
    target: &StatusTarget,
    to: &UserStatus,
) -> Result<(), StatusRefusal> {
    if target.is_global_owner {
        return match to {
            UserStatus::Active => Ok(()),
            UserStatus::Deleted => Err(StatusRefusal::GlobalOwner),
        };
    }
    match &target.grant {
        Some(g) => delegatable(actor, &g.role, g.scope_all, &g.scope_org_ids)
            .map_err(StatusRefusal::Delegation),
        None => Ok(()),
    }
}

/// What a status change is decided on about the **caller**: their scope over the user
/// directory and their standing. Loaded once; [`Self::decide`] is then asked twice, once
/// before the write and once inside it after the row is locked.
pub(super) struct StatusDecision {
    scope: Option<Vec<Uuid>>,
    facts: PrincipalFacts,
    actor_email: String,
}

impl StatusDecision {
    pub(super) async fn load(
        db: &DatabaseConnection,
        actor: &AuthenticatedUser,
    ) -> Result<Self, Response> {
        let scope = user_scope(db, actor)
            .await
            .map_err(IntoResponse::into_response)?;
        let facts = delegation::actor_facts(db, actor)
            .await
            .map_err(IntoResponse::into_response)?;
        Ok(Self {
            scope,
            facts,
            actor_email: actor.email.clone().unwrap_or_default(),
        })
    }

    /// May the caller set `target`'s status to `to`, as `db` reads the target now? Every
    /// org of theirs must be in scope ([`reaches_all_of`], else the 404 a missing user
    /// gets), then [`may_change_status`] over their stored grant. Answers their orgs.
    ///
    /// Asked on the write's transaction after `FOR UPDATE`, it reads memberships and the
    /// grant as they stand when the row is written, so one that changed after the first
    /// answer is what gets decided on. A delegation refusal is the console's
    /// `delegation::Refusal` (403 with `message` and `denial`); a root refusal says so
    /// with `code: "global_owner"`.
    pub(super) async fn decide<C: ConnectionTrait>(
        &self,
        db: &C,
        target: &users::Model,
        to: &UserStatus,
    ) -> Result<Vec<Uuid>, Response> {
        let member_orgs = member_orgs_of(db, target.id)
            .await
            .map_err(|e| unreadable("memberships")(e).into_response())?;
        if !reaches_all_of(self.scope.as_deref(), &member_orgs) {
            return Err(StatusCode::NOT_FOUND.into_response());
        }
        let email = target.email.as_deref().unwrap_or_default();
        let grant = stored_grant_of(db, email)
            .await
            .map_err(|e| unreadable("target grant")(e).into_response())?;
        let target = StatusTarget {
            is_global_owner: globals::is_global_owner(&Caller::without_credential(
                target.id, email,
            )),
            grant,
        };
        self.refuse(may_change_status(&self.facts, &target, to))?;
        Ok(member_orgs)
    }

    fn refuse(&self, decision: Result<(), StatusRefusal>) -> Result<(), Response> {
        match decision {
            Ok(()) => Ok(()),
            Err(StatusRefusal::Delegation(denial)) => {
                delegation::refuse(Err(denial), &self.actor_email)
                    .map_err(IntoResponse::into_response)
            }
            Err(StatusRefusal::GlobalOwner) => Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "code": "global_owner",
                    "message": "The Global Owner's account cannot be suspended from the console. \
                                Root standing is the OXY_OWNER allow-list; change it there.",
                })),
            )
                .into_response()),
        }
    }
}

#[cfg(test)]
#[path = "user_scope_tests.rs"]
mod tests;
