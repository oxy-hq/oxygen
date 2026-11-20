//! A platform grant **as stored** — the `app_admins` row and its
//! `app_admin_scope_orgs` set, role unexpanded — and the one decision over it,
//! [`delegatable`]: the delegation bound ([`oxy_authz::may_delegate`]) applied to
//! a row as it exists in the table.
//!
//! The grant-table handlers (`oxy-api-admin`'s `app_admins`, out of `oxy-app`'s
//! `admin::app_admins` since 2026-11-20) and the user directory's status fence
//! (`admin::user_scope`) both decide on this shape.
//! Down from `admin::app_admins` in the admin split's PR 3, so the user
//! directory can leave `oxy-app` without taking the grant table's routes.

use entity::prelude::AppAdmins;
use entity::{app_admin_scope_orgs, app_admins};
use oxy_authz::{DelegationDenial, PlatformRole, PrincipalFacts, Scope, may_delegate};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

/// A stored grant's `(role, scope)` — what the delegation bound is asked about.
///
/// `PlatformRole::from_str` returning `None` means this build cannot expand the stored
/// role. The loader already drops such a grant rather than guessing, and the same
/// reading applies here: an unnameable role is not something to compare ranks against.
/// See [`delegatable`] for who can still act on those rows.
fn stored_standing(
    role: &str,
    scope_all: bool,
    scope_org_ids: &[Uuid],
) -> Option<(PlatformRole, Scope)> {
    let role = PlatformRole::from_str(role)?;
    let scope = if scope_all {
        Scope::All
    } else {
        Scope::Orgs(scope_org_ids.to_vec())
    };
    Some((role, scope))
}

/// May `facts` write this stored row? The delegation bound, applied to a grant as it
/// exists in the table.
///
/// **The Global Owner is checked first, before the role is expanded.** The obvious
/// spelling — `match stored_standing(..) { Some(..) => may_delegate(..), None => Err(..) }`
/// — returns on the `None` arm without ever reaching the owner short-circuit inside
/// `may_delegate`, which locked root out of exactly the rows a rollback creates. The
/// comment above `stored_standing` claimed those rows froze "for everyone but the owner";
/// they froze for the owner too, and the only remedy was hand-written SQL against
/// `app_admins`. Root has no rank to compare, which is the whole reason that
/// short-circuit exists — so it has to come first here as well.
///
/// For everyone else an unnameable role is undelegatable: it cannot be ranked, and
/// guessing a rank for it is how a rollback turns into an escalation.
pub fn delegatable(
    facts: &PrincipalFacts,
    role: &str,
    scope_all: bool,
    scope_org_ids: &[Uuid],
) -> Result<(), DelegationDenial> {
    match stored_standing(role, scope_all, scope_org_ids) {
        Some((r, sc)) => may_delegate(facts, r, &sc),
        None if facts.is_global_owner => Ok(()),
        None => Err(DelegationDenial::RoleNotBelow),
    }
}

/// A grant row as stored, role unexpanded — what [`delegatable`] decides on.
pub struct StoredGrant {
    pub role: String,
    pub scope_all: bool,
    pub scope_org_ids: Vec<Uuid>,
}

/// The grant stored for `email`, if any, for a write that acts on a **person** who holds
/// standing rather than on their grant row (`users_admin::set_user_status`).
///
/// Read raw rather than through `globals::grant_of_email`, which drops a role this build
/// cannot name: here that would read as "not staff" and skip the bound, where
/// [`delegatable`] keeps such a row the owner's alone.
pub async fn stored_grant_of<C: sea_orm::ConnectionTrait>(
    db: &C,
    email: &str,
) -> Result<Option<StoredGrant>, sea_orm::DbErr> {
    let key = email.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Ok(None);
    }
    let Some(row) = AppAdmins::find()
        .filter(app_admins::Column::Email.eq(key))
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let scope_org_ids = if row.scope_all {
        Vec::new()
    } else {
        app_admin_scope_orgs::Entity::find()
            .filter(app_admin_scope_orgs::Column::AppAdminId.eq(row.id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| r.org_id)
            .collect()
    };
    Ok(Some(StoredGrant {
        role: row.role,
        scope_all: row.scope_all,
        scope_org_ids,
    }))
}
