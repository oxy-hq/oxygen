//! The staff console's lenient org filter: which orgs a **bounded** platform grant
//! reaches, for the registry listings.
//!
//! Not in [`super::scope`]: that module is the one reader of a grant's scope
//! (`caller_scope`, and `app_scope_boundary` holds it to that). This is the
//! app registry's older, separate read, kept as it was when it lived in
//! `admin::apps::handlers`; the registry and the airhouse, airway-config, OLTP and
//! Oxy-access listings share it.

use uuid::Uuid;

use crate::{Caller, globals};

/// The org ids a **bounded** platform grant reaches — `None` for unbounded.
///
/// **Fallible on purpose.** An unreadable grant is not "unbounded"; it is *unknown*, and
/// the two callers of this need opposite things from that:
///
/// * a READ (`list_apps`, `oxy-access`) prefers to show rows — the capability gate
///   already admitted this caller, and a blip shouldn't present an empty console as
///   though it were the truth. [`scope_org_filter`] collapses `Err` that way.
/// * a WRITE (`create_app`, the `batch/*` endpoints) must refuse. Treating unknown as
///   unbounded there means one transient `DbErr` turns `batch/delete` into a
///   cross-tenant delete — which the module docs correctly call the worst leak this
///   model could have.
///
/// `app_scope_guard` already fails closed on `Err`. Having the two halves of one system
/// disagree about what an unreadable grant means is how this drifts, so the difference is
/// stated here once rather than re-decided at each call site.
pub async fn scope_org_filter_checked(
    db: &sea_orm::DatabaseConnection,
    user: &oxy_auth::types::AuthenticatedUser,
) -> Result<Option<Vec<Uuid>>, sea_orm::DbErr> {
    use oxy_authz::Scope;
    // A Global Owner is unbounded by definition and holds no grant row — short-circuit
    // rather than reading one, so an owner who ALSO carries a bounded row (possible when
    // OXY_OWNER and OXY_GLOBAL_ADMINS overlap) isn't narrowed here while every other
    // path says they reach everything. Mirrors `platform_reaches` / `platform_holds`.
    if globals::is_global_owner(&Caller::from_user(user)) {
        return Ok(None);
    }
    match globals::platform_grant_checked(db, &Caller::from_user(user)).await? {
        Some(grant) => Ok(match &grant.scope {
            Scope::All => None,
            Scope::Orgs(orgs) => Some(orgs.clone()),
        }),
        // No grant row and not an owner: nothing to narrow by. The capability gate
        // decides whether they belong here at all.
        None => Ok(None),
    }
}

/// The lenient read-path filter — see [`scope_org_filter_checked`] for why `Err`
/// collapses to "don't filter" here and nowhere else.
///
/// `Some(vec![])` is a real answer — a grant bounded to nothing — and correctly yields
/// an empty list.
pub async fn scope_org_filter(
    db: &sea_orm::DatabaseConnection,
    user: &oxy_auth::types::AuthenticatedUser,
) -> Option<Vec<Uuid>> {
    scope_org_filter_checked(db, user).await.unwrap_or_else(|e| {
        tracing::warn!(
            target: "authz",
            error = %e,
            "platform grant unreadable — listing unfiltered rather than showing an empty registry"
        );
        None
    })
}
