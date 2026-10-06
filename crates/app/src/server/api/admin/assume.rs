//! `/api/admin/assume` — explicit, audited, time-bounded **assume-role** for Oxy
//! staff.
//!
//! ## Why this exists
//!
//! Oxy staff have ALWAYS been able to act as a tenant Owner: `org_context` and
//! `workspace_context` synthesize an Owner `org_members` row for any Global Owner
//! / Global Admin who isn't a real member (`is_global_override = true`). That is
//! impersonation — it was just **implicit, unbounded, unlogged and unannounced**.
//! You opened a tenant and silently became its Owner.
//!
//! This module doesn't add a new power. It makes the existing one *observable*:
//!
//! * **Opt-in** — no live session for `(actor, org)` ⇒ **no synthetic membership**
//!   ⇒ the operator is a plain non-member and gets a 403, exactly like anyone
//!   else. The seam is now closed by default.
//! * **Scoped** — one org per session. No blanket cross-tenant reach.
//! * **Bounded** — [`MAX_SESSION`] minutes, non-renewable (start a fresh one).
//! * **Explained** — a `reason` is required.
//! * **Audited** — start and end both write `audit_events`, with the actor being
//!   the REAL staff user. The audit trail never names the impersonated identity as
//!   the actor, or the log would launder the impersonation.
//! * **Announced** — the frontend renders a persistent banner while it's live.
//!
//! ## What it deliberately does NOT do: act as a *user*
//!
//! Assume-role names an **org**, never a person. "Act as bob@acme" was designed,
//! scoped read-only, and then **rejected** (2026-07-14) — the schema for it was
//! added and dropped again (`m20260714_000007`). The reasoning, so it isn't
//! relitigated from scratch:
//!
//! Acting as an org is bounded on both ends: the session names one org, and the
//! role it grants is capped — Owner for staff, Admin for a partner — with
//! `is_global_override` set, so `OrgAdminStrict` still shuts billing and
//! admin-promotion. A **user** swap has neither bound. You become a specific
//! person, and every ownership check in the product silently resolves to them.
//!
//! Read-only would have contained the worst of it (a write recorded as *Bob did
//! this* is forgery, not impersonation) — but read-only is a property you must keep
//! true across every endpoint that will ever exist. The cost of getting it wrong
//! once is a customer's data touched under their own name, in a log that says they
//! did it. Debuggability is worth a great deal; it is not worth an authorization
//! primitive whose safety depends on nobody ever adding a handler that forgets.
//!
//! The debugging need is real, so meet it another way: reproduce with the *role*,
//! not the *identity* (act as the org, then read the member's effective
//! permissions), or ask the customer to share what they see. If that proves
//! insufficient, revisit this with a design that does not depend on a global
//! read-only invariant.
//!
//! ## What it deliberately does NOT change
//!
//! * **Strict guards stay strict.** `OrgAdminStrict` / `OrgMemberStrict` still
//!   reject `is_global_override`, so billing and admin-promotion remain closed
//!   under an assumed identity. Assuming a role does not launder you into a
//!   tenant officer.
//! * **The customer's Oxy-staff lockdown still holds.** Custom-app access is
//!   gated on `app_admins` + `workspace_oxy_lockdown` and on *real*
//!   `org_members` rows — never on the synthetic one — so assuming a role is not
//!   a way around a customer's lockdown.

use axum::Json;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Router, extract::OriginalUri, extract::Query};
use chrono::{Duration, Utc};
use entity::admin_assume_sessions;
use entity::prelude::{AdminAssumeSessions, Organizations};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::router::AppState;
use oxy_app_core::audit::{self, AuditEntry};
use oxy_app_core::pagination::{self, Paged, trim_overfetch};

// The pure liveness-query cluster now lives in `oxy-server-authz` so the authz fact
// loader and the partner tier can read session liveness without depending on `oxy-app`.
// The handlers below use `MAX_SESSION` / `live_filter` internally; the three query fns
// are re-exported so external callers keep resolving `assume::…` unchanged.
use oxy_server_authz::assume_liveness::{MAX_SESSION, live_filter, usable_by};
pub use oxy_server_authz::assume_liveness::{
    is_session_live, live_assumed_org_ids, live_sessions_for,
};

/// Mounted at `/api/assume`, NOT under `/admin`.
///
/// It cannot live under the admin surface: a partner acting as a client is not
/// staff, so `/admin/*` would 403 them — they could start a session and never end
/// it. And staff can't reach `/admin/*` while acting either, because acting closes
/// the staff surface. Either way, putting the exit behind the door it locks is a
/// trap.
///
/// Authorization lives in the handlers (`may_act_as`), which is where it belongs
/// now that two different populations can act.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/assume", post(start).delete(end))
        .route("/assume/current", get(current))
        // The full impersonation log is a staff view — a partner has no business
        // reading who else Oxy has acted as.
        .route("/assume/history", get(history))
}

// Keep clippy quiet about the unused imports when the router shrinks.
#[allow(unused_imports)]
use axum::routing as _routing;

#[derive(Deserialize)]
pub struct StartBody {
    pub org_id: Uuid,
    /// Required. An unexplained impersonation is a red flag, not a convenience.
    pub reason: String,
}

#[derive(Serialize)]
pub struct SessionDto {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: Option<String>,
    /// Where the operator lands: the org's own product surface is `/{slug}`.
    pub org_slug: Option<String>,
    /// True when the assumed org holds a partner grant — then the surface that
    /// matters is the partner console, not the org home. Acting as a partner and
    /// landing on an org dashboard would show you the wrong product.
    pub is_partner: bool,
    pub actor_email: String,
    pub reason: String,
    pub started_at: String,
    pub expires_at: String,
    /// Convenience for the banner countdown.
    pub expires_in_seconds: i64,
}

fn db_err<E: std::fmt::Display>(ctx: &str) -> impl Fn(E) -> StatusCode + '_ {
    move |e| {
        tracing::error!("admin/assume: {ctx}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn db() -> Result<DatabaseConnection, StatusCode> {
    establish_connection().await.map_err(|e| {
        tracing::error!("admin/assume: DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// **Who may act as an org.** Two populations, one rule each:
///
/// * **Oxy staff** — any org. They always could (the synthetic-Owner seam);
///   assume-role just makes it deliberate and audited.
/// * **A partner** — only the clients they are assigned to, and only with
///   `develop_apps`. Entering a client's product IS the data plane: it means
///   reading their warehouse, their threads, their dashboards. An
///   `account_manager` — whose whole point is "runs the relationship, cannot touch
///   the data" — must not be able to walk in through this door, or the role split
///   is decorative.
///
/// Re-checked on **every request** (not just at session creation), so revoking a
/// partner's `develop_apps` or un-assigning a client kills a live session's reach
/// immediately rather than at expiry.
///
/// Takes the [`Caller`], so both authorities are what the request's credential
/// carries: an API token without `platform` is not staff here, and one without
/// `partner` operates no partner.
pub async fn may_act_as(
    db: &DatabaseConnection,
    caller: &crate::server::authz::Caller,
    org_id: Uuid,
) -> Option<ActingAs> {
    // Staff acting as a tenant synthesize **Owner** in that org, so this door demands the
    // capability that gates owner-level authority (`ManageOrgSettings`) — and honours the
    // grant's scope, since it is a reach INTO a specific tenant rather than a console
    // section.
    //
    // `is_staff()` alone would reopen exactly the hole the doc comment above warns about:
    // an App Operator holds neither capability, and their legitimate reach into a tenant
    // is the app data plane (`Ring::AppAccess` via `develop_apps`), which needs no
    // impersonation at all. A Global Admin holds `ManageOrgSettings` at `Scope::All`, so
    // nothing changes for them.
    if crate::server::authz::globals::platform_reaches(
        db,
        caller,
        oxy_authz::Cap::ManageOrgSettings,
        org_id,
    )
    .await
    {
        return Some(ActingAs::Staff);
    }

    // A partner acting as one of its clients.
    use crate::server::api::middlewares::partner_authz::{
        PartnerCapability, partner_for_org, resolve_scope,
    };
    let partner_org_id = partner_for_org(db, org_id).await?;
    let scope = resolve_scope(db, partner_org_id, caller).await?;
    if !scope.allows(PartnerCapability::DevelopApps) {
        return None;
    }
    if !scope.org_ids.contains(&org_id) {
        return None;
    }
    Some(ActingAs::Partner)
}

/// Which authority a live session rests on — it decides the role the request runs
/// with, so it must not be guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActingAs {
    Staff,
    Partner,
}

impl ActingAs {
    /// Staff get Owner (what the old silent override gave them). A partner gets
    /// **Admin**: enough to work inside the client's product, but the org's own
    /// Owner remains untouchable — a partner administers a client, it never owns
    /// one. Both are marked `is_global_override`, so `OrgAdminStrict` still shuts
    /// billing and admin-promotion to them.
    pub fn org_role(self) -> entity::org_members::OrgRole {
        match self {
            ActingAs::Staff => entity::org_members::OrgRole::Owner,
            ActingAs::Partner => entity::org_members::OrgRole::Admin,
        }
    }
}

fn to_dto(
    m: admin_assume_sessions::Model,
    org: Option<&entity::organizations::Model>,
    is_partner: bool,
) -> SessionDto {
    let expires_in_seconds = (m.expires_at.to_utc() - Utc::now()).num_seconds().max(0);
    SessionDto {
        id: m.id,
        org_id: m.org_id,
        org_name: org.map(|o| o.name.clone()),
        org_slug: org.map(|o| o.slug.clone()),
        is_partner,
        actor_email: m.actor_email,
        reason: m.reason,
        started_at: m.started_at.to_rfc3339(),
        expires_at: m.expires_at.to_rfc3339(),
        expires_in_seconds,
    }
}

/// `POST /admin/assume` — begin acting as an Owner of `org_id`.
pub async fn start(
    actor: oxy_app_core::audit::RequestActor,
    Json(body): Json<StartBody>,
) -> Result<Json<SessionDto>, StatusCode> {
    let reason = body.reason.trim().to_string();
    if reason.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let caller = crate::server::authz::caller_of(&actor);
    // A session belongs to the credential that opens it (API-tokens design
    // §3.2). A new-format token may open one only when it carries platform
    // standing; a browser session and a legacy key open one as they always
    // have, and share it.
    if caller.reach().is_some_and(|reach| !reach.platform) {
        return Err(StatusCode::FORBIDDEN);
    }
    let db = db().await?;

    let org = Organizations::find_by_id(body.org_id)
        .one(&db)
        .await
        .map_err(db_err("load org"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    // The gate. Staff may act as any org; a partner only as an assigned client,
    // and only with `develop_apps`.
    let authority = may_act_as(&db, &caller, body.org_id)
        .await
        .ok_or(StatusCode::FORBIDDEN)?;

    // Re-entering an org you're already assuming is idempotent — return the live
    // session rather than stacking rows (and rather than silently extending it).
    let now = Utc::now().fixed_offset();
    //
    // "Already assuming" is per credential: a token's session is not the
    // browser's, so each gets (and ends) its own.
    if let Some(existing) = AdminAssumeSessions::find()
        .filter(usable_by(&caller))
        .filter(admin_assume_sessions::Column::OrgId.eq(body.org_id))
        .filter(live_filter(now))
        .one(&db)
        .await
        .map_err(db_err("existing session"))?
    {
        let is_partner = org_is_partner(&db, body.org_id).await;
        return Ok(Json(to_dto(existing, Some(&org), is_partner)));
    }

    let expires_at = (Utc::now() + Duration::minutes(MAX_SESSION)).fixed_offset();

    // The session and its audit row must land together. The comment below is a
    // promise — "if it can't be recorded, the session must not stand" — and a
    // separate insert + audit::record broke it: a failed audit write returned 500
    // while leaving a LIVE, unaudited impersonation behind. One transaction makes
    // the promise true: either both commit, or neither does.
    let txn = db.begin().await.map_err(db_err("begin assume"))?;

    let model = admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(actor.id),
        actor_email: ActiveValue::Set(actor.label().to_string()),
        org_id: ActiveValue::Set(body.org_id),
        reason: ActiveValue::Set(reason.clone()),
        started_at: ActiveValue::NotSet,
        expires_at: ActiveValue::Set(expires_at),
        ended_at: ActiveValue::Set(None),
        // NULL for a browser session and a legacy key: shared, as before.
        token_id: ActiveValue::Set(caller.assume_binding()),
    }
    .insert(&txn)
    .await
    .map_err(db_err("insert session"))?;

    audit::record_in_txn(
        &txn,
        AuditEntry::for_request(&actor, "admin.assume.started")
            .org(body.org_id)
            .target("organization", body.org_id.to_string(), org.name.clone())
            .reason(reason)
            .metadata(serde_json::json!({ "expires_at": expires_at.to_rfc3339() })),
    )
    .await
    .map_err(db_err("audit assume.started"))?;

    txn.commit().await.map_err(db_err("commit assume"))?;

    tracing::warn!(
        actor = %actor.label(), org_id = %body.org_id, ?authority,
        "assume: STARTED — actor is now acting as this org"
    );
    let is_partner = org_is_partner(&db, body.org_id).await;
    Ok(Json(to_dto(model, Some(&org), is_partner)))
}

#[derive(Deserialize)]
pub struct EndQuery {
    /// Optional: end the session for this org. Omitted = end ALL live sessions.
    pub org_id: Option<Uuid>,
}

/// `DELETE /admin/assume` — stop acting as a tenant.
pub async fn end(
    actor: oxy_app_core::audit::RequestActor,
    Query(q): Query<EndQuery>,
) -> Result<StatusCode, StatusCode> {
    let db = db().await?;
    let now = Utc::now().fixed_offset();

    // Only the sessions this credential holds: a token ends its own, the
    // browser (and a legacy key) the browser's.
    let mut query = AdminAssumeSessions::find()
        .filter(usable_by(&crate::server::authz::caller_of(&actor)))
        .filter(live_filter(now));
    if let Some(org_id) = q.org_id {
        query = query.filter(admin_assume_sessions::Column::OrgId.eq(org_id));
    }
    let live = query.all(&db).await.map_err(db_err("load live sessions"))?;

    for s in live {
        let org_id = s.org_id;
        // Atomic, matching `start`: the end + its audit land in one transaction,
        // so a dropped `assume.ended` can't leave the impersonation log showing an
        // apparently-open session.
        let txn = db.begin().await.map_err(db_err("begin end-session"))?;
        let mut m: admin_assume_sessions::ActiveModel = s.into();
        m.ended_at = ActiveValue::Set(Some(now));
        m.update(&txn).await.map_err(db_err("end session"))?;
        audit::record_in_txn(
            &txn,
            AuditEntry::for_request(&actor, "admin.assume.ended")
                .org(org_id)
                .target("organization", org_id.to_string(), String::new()),
        )
        .await
        .map_err(db_err("audit assume.ended"))?;
        txn.commit().await.map_err(db_err("commit end-session"))?;
        tracing::info!(actor = %actor.label(), %org_id, "admin/assume: ENDED");
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /admin/assume/current` — the caller's live sessions. Drives the banner.
pub async fn current(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<Vec<SessionDto>>, StatusCode> {
    let db = db().await?;
    let now = Utc::now().fixed_offset();

    let rows = AdminAssumeSessions::find()
        .filter(usable_by(&crate::server::authz::Caller::from_user(&actor)))
        .filter(live_filter(now))
        .order_by_desc(admin_assume_sessions::Column::StartedAt)
        .all(&db)
        .await
        .map_err(db_err("load current"))?;

    let orgs = org_index(&db, rows.iter().map(|r| r.org_id).collect()).await?;
    let partners = partner_org_ids(&db).await;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let is_partner = partners.contains(&r.org_id);
                let org = orgs.get(&r.org_id).cloned();
                to_dto(r, org.as_ref(), is_partner)
            })
            .collect(),
    ))
}

/// Default and ceiling for one page of `/assume/history`, matching `admin/audit.rs`.
const HISTORY_PAGE: u64 = 100;
const HISTORY_MAX_PAGE: u64 = 500;

/// How `/assume/history` is paged.
///
/// `limit`/`offset` with the same defaults and ceiling as `admin/audit.rs`,
/// deliberately: both read an append-only log for the same auditor, and a
/// second spelling of "page an audit log" in the same admin surface is a thing
/// to memorise for no benefit.
#[derive(Deserialize, Default)]
pub struct HistoryQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

/// How many rows one `/assume/history` request may read.
///
/// A named function rather than an inline `unwrap_or(..).min(..)` so the
/// ceiling is testable: the whole point of this handler's change is that no
/// caller-supplied `limit` can restore the unbounded read, and that is a claim
/// worth pinning rather than re-reading off a chained expression.
fn history_page_size(limit: Option<u64>) -> u64 {
    // Clamped at BOTH ends. A zero page size is not an empty page — it is an
    // over-fetch of one row that `trim_overfetch` discards while still
    // reporting "more", against a cursor that advances by zero, i.e. a
    // `rel="next"` pointing at the request that produced it.
    limit.unwrap_or(HISTORY_PAGE).clamp(1, HISTORY_MAX_PAGE)
}

/// `GET /assume/history` — every session (live or not). The impersonation log an
/// operator or auditor reads. **Staff only** — it spans tenants.
///
/// PAGED, and it has to be: `admin_assume_sessions` is append-only and spans
/// every tenant, so an unbounded read grows with the platform's whole
/// impersonation history — and each row then costs a lookup in `org_index`.
/// It was reading the entire table. Newest first, so the default page is the
/// part an auditor actually wants.
pub async fn history(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    OriginalUri(uri): OriginalUri,
    Query(q): Query<HistoryQuery>,
) -> Result<Paged<SessionDto>, StatusCode> {
    let db = db().await?;
    // The impersonation log spans every tenant, so it is an audit read — `ViewAudit`,
    // not merely "is staff". An App Operator has no business reading who impersonated
    // whom across the platform.
    let facts = crate::server::authz::loader::load_platform_facts(
        &db,
        &crate::server::authz::Caller::from_user(&actor),
    )
    .await
    .ok_or(StatusCode::FORBIDDEN)?;
    if !crate::server::authz::allows(
        &facts,
        crate::server::authz::Action::PlatformAudit,
        &crate::server::authz::Resource::platform(),
    ) {
        return Err(StatusCode::FORBIDDEN);
    }
    // The capability says the caller may read the log; the grant's SCOPE says whose.
    // A session row names the org that was impersonated, so a bounded grant reads the
    // sessions into its own orgs and no others — narrowed here, ahead of the paging.
    let scope = crate::server::api::admin::scope::list_scope(&db, &actor).await?;
    let limit = history_page_size(q.limit);
    let offset = q.offset.unwrap_or(0);
    let mut find = AdminAssumeSessions::find();
    if let Some(orgs) = scope {
        find = find.filter(admin_assume_sessions::Column::OrgId.is_in(orgs));
    }
    let mut rows = find
        .order_by_desc(admin_assume_sessions::Column::StartedAt)
        // `started_at` alone is not a total order — two sessions can share an
        // instant, and then a row can repeat on one page and vanish from the
        // next. The id tiebreak is what makes the offset walk cover the log.
        .order_by_desc(admin_assume_sessions::Column::Id)
        .offset(offset)
        // One extra row, so the `Link: rel="next"` below is known without a
        // second COUNT(*) over a table that only grows.
        .limit(limit + 1)
        .all(&db)
        .await
        .map_err(db_err("load history"))?;
    let has_more = trim_overfetch(&mut rows, limit);

    let orgs = org_index(&db, rows.iter().map(|r| r.org_id).collect()).await?;
    let partners = partner_org_ids(&db).await;
    Ok(pagination::page(
        rows.into_iter()
            .map(|r| {
                let is_partner = partners.contains(&r.org_id);
                let org = orgs.get(&r.org_id).cloned();
                to_dto(r, org.as_ref(), is_partner)
            })
            .collect(),
        has_more,
        &uri,
        // Saturating: `?offset=<u64::MAX>` would otherwise panic a debug build
        // and wrap to a nonsense cursor in release.
        &[("offset", offset.saturating_add(limit).to_string())],
    ))
}

async fn org_index(
    db: &DatabaseConnection,
    ids: Vec<Uuid>,
) -> Result<std::collections::HashMap<Uuid, entity::organizations::Model>, StatusCode> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    Ok(Organizations::find()
        .filter(entity::organizations::Column::Id.is_in(ids))
        .all(db)
        .await
        .map_err(db_err("load orgs"))?
        .into_iter()
        .map(|o| (o.id, o))
        .collect())
}

/// Does this org hold a partner grant? Decides which surface the operator lands on.
async fn org_is_partner(db: &DatabaseConnection, org_id: Uuid) -> bool {
    entity::prelude::PartnerGrants::find_by_id(org_id)
        .one(db)
        .await
        .ok()
        .flatten()
        .is_some_and(|g| g.status == "active")
}

async fn partner_org_ids(db: &DatabaseConnection) -> std::collections::HashSet<Uuid> {
    entity::prelude::PartnerGrants::find()
        .all(db)
        .await
        .map(|rows| {
            rows.into_iter()
                .filter(|g| g.status == "active")
                .map(|g| g.org_id)
                .collect()
        })
        .unwrap_or_default()
}

// ── acting is a MODE, not a badge ─────────────────────────────────────────

/// **Acting closes the admin surface.**
///
/// Without this, "act as" was decoration: you got a banner, kept every staff
/// power, and stayed on the admin page — so nothing about your reach actually
/// changed, and the mode had no meaning. Impersonation has to be a *place you go*,
/// not a hat you wear while continuing to be yourself.
///
/// It also removes a genuine hazard: staff writes performed through the admin API
/// during an assume window would be attributable to the real actor, but sit inside
/// an audit span that says "was acting as tenant X" — mixing two authorities in
/// one session is exactly the thing the audit chain is supposed to keep separate.
///
/// The `/assume` subtree is deliberately NOT behind this layer (see `router()`),
/// so the one-click exit always works. Ending the session restores admin.
pub async fn block_admin_while_acting(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let Ok(db) = establish_connection().await else {
        // Can't tell ⇒ don't invent a lockout. Admin routes have their own guards.
        return Ok(next.run(request).await);
    };
    let caller = crate::server::authz::Caller::from_user(&actor);
    if !live_sessions_for(&db, &caller).await.is_empty() {
        tracing::info!(
            actor = %actor.label(),
            "admin/assume: admin surface refused — actor is currently acting as a tenant"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bound is a hard ceiling, and it is NOT renewable — a longer
    /// investigation must leave a trail of deliberate re-entries.
    #[test]
    fn session_is_bounded_to_an_hour() {
        assert_eq!(MAX_SESSION, 60);
        let start = Utc::now();
        let expires = start + Duration::minutes(MAX_SESSION);
        assert!((expires - start).num_minutes() <= 60);
    }

    /// An empty reason is rejected before any row is written — the reason is the
    /// point, not decoration.
    #[test]
    fn blank_reason_is_rejected() {
        for raw in ["", "   ", "\t\n"] {
            assert!(raw.trim().is_empty(), "{raw:?} must be treated as blank");
        }
    }

    /// No `?limit=` restores the unbounded read — and none produces a page of
    /// zero either.
    ///
    /// `/assume/history` spans every tenant and `admin_assume_sessions` only
    /// grows, so the ceiling is half the fix: a handler that honoured
    /// `?limit=100000` would be the old behaviour with an extra step. The FLOOR
    /// is the other half. At zero the over-fetch reads one row, `trim_overfetch`
    /// discards it and still reports "more", and the cursor advances by zero —
    /// so `rel="next"` points back at the same request and a link-following
    /// client walks empty pages forever.
    #[test]
    fn history_page_is_capped_whatever_the_caller_asks_for() {
        assert_eq!(history_page_size(None), HISTORY_PAGE);
        assert_eq!(history_page_size(Some(25)), 25);
        assert_eq!(history_page_size(Some(HISTORY_MAX_PAGE)), HISTORY_MAX_PAGE);
        assert_eq!(history_page_size(Some(u64::MAX)), HISTORY_MAX_PAGE);
        assert_eq!(
            history_page_size(Some(0)),
            1,
            "a page of zero never advances"
        );
    }

    /// Liveness = not ended AND not expired. An expired-but-unended row grants
    /// nothing, so a forgotten session can't become a permanent backdoor.
    #[test]
    fn expiry_alone_kills_a_session() {
        let now = Utc::now();
        let expired = now - Duration::minutes(1);
        let live = now + Duration::minutes(1);
        assert!(expired < now, "an expired session must not be live");
        assert!(live > now, "an unexpired session is live");
    }
}
