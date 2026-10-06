//! `/api/admin/app-admins` — the **platform-grant control plane**.
//!
//! Reachable by anyone holding `Cap::ManagePlatformGrants` (the Global Owner, and
//! `global_admin` via its preset), then fenced **per row** by `admin::delegation`.
//!
//! This was owner-only for one release, and the reason it no longer is matters: the
//! original rationale — "a capability that could edit the grant table would let its
//! holder widen their own ceiling" — argues against an unbounded capability, which is
//! not the only kind available. `oxy_authz::may_delegate` bounds every write to a grant
//! strictly weaker than the writer's own, so a holder's own row is structurally
//! unreachable and the ceiling holds. What the owner keeps is the top tier: only they
//! can create, re-role or revoke a `global_admin`.
//!
//! **The capability gate alone is not the control.** It answers "may you administer
//! grants at all"; every handler below must additionally decide per row:
//! `delegation::actor_facts` once, then `delegation::refuse(may_delegate(..))` for each
//! grant the request reads or writes. A route added here without that fence re-opens the
//! escalation the owner-only guard existed to prevent — which is why
//! `app_scope_boundary` asserts it structurally rather than trusting review.
//!
//! A grant is `(role × scope)`:
//! * **role** — a `PlatformRole` preset, expanded to capabilities in `oxy-authz`.
//!   `global_admin` is the historical meaning of a row here; `app_operator` ships and
//!   develops custom apps and holds nothing else.
//! * **scope** — every org, or an explicit list. Scope narrows *tenant* reach; it does
//!   not hide console sections (see `platform_cap_guard`).
//!
//! Replaces the legacy `OXY_APP_ADMINS` env var, which can only ever seed
//! `global_admin` at unbounded scope.

use std::collections::HashMap;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use entity::prelude::AppAdmins;
use entity::{app_admin_scope_orgs, app_admins};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_authz::{DelegationDenial, PlatformRole, PrincipalFacts, Scope, may_delegate};
use sea_orm::{
    ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::api::admin::delegation;
use crate::server::authz::globals::invalidate_admin_cache;
use crate::server::router::AppState;
use oxy_app_core::audit;

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
fn delegatable(
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

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/app-admins", get(list_app_admins).post(create_app_admin))
        .route("/app-admins/{id}", delete(delete_app_admin))
}

#[derive(Serialize)]
pub struct AppAdminResponse {
    pub id: Uuid,
    pub email: String,
    pub granted_by: Option<Uuid>,
    pub created_at: String,
    /// When the grant last changed. Equal to `created_at` for one that never has.
    pub updated_at: String,
    /// `PlatformRole::as_str` — `global_admin` or `app_operator`.
    pub role: String,
    /// `true` = every org. `false` = the orgs in `scope_org_ids`.
    pub scope_all: bool,
    /// Empty when `scope_all` is true. The list endpoint fills this for every row from
    /// one grouped query — not a query per row, and not left empty.
    pub scope_org_ids: Vec<Uuid>,
    /// The capabilities `role` expands to, so the console can render what a grant
    /// actually buys without duplicating the expansion in TypeScript. Derived, never
    /// stored — the model stays the only definition.
    pub capabilities: Vec<String>,
    /// **May the caller write this row?** Server-computed per row, so the console can
    /// disable what it cannot change instead of offering a control that 403s.
    ///
    /// Every grant is listed to every operator who may open this surface — knowing who
    /// holds staff standing is the point of the page, and a filtered list would let a
    /// bounded operator conclude a colleague does not exist. Authority to *see* and
    /// authority to *change* are different questions, and this is the second one.
    ///
    /// UX only. `create`/`delete` re-decide server-side; a client that ignores this
    /// gets a 403, not a write.
    pub can_manage: bool,
}

impl From<app_admins::Model> for AppAdminResponse {
    fn from(m: app_admins::Model) -> Self {
        let capabilities = PlatformRole::from_str(&m.role)
            .map(|r| r.caps().iter().map(|c| c.as_str().to_string()).collect())
            .unwrap_or_default();
        Self {
            id: m.id,
            email: m.email,
            granted_by: m.granted_by,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
            role: m.role,
            scope_all: m.scope_all,
            scope_org_ids: Vec::new(),
            capabilities,
            // Fail-closed default. `From<Model>` has no caller to decide against, so
            // every construction site must set this deliberately — a forgotten one
            // renders a disabled control, never a permitted one.
            can_manage: false,
        }
    }
}

pub async fn list_app_admins(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<Vec<AppAdminResponse>>, delegation::Refusal> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("list_app_admins DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let rows = AppAdmins::find()
        .order_by_asc(app_admins::Column::Email)
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("list_app_admins query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Scope in ONE query for the whole page, grouped in memory. The console renders a
    // grant's reach inline, so an N+1 here would be a query per staff member on every
    // page load. Only bounded grants have rows, so this is usually empty.
    let mut scopes: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    if rows.iter().any(|r| !r.scope_all) {
        let bounded: Vec<Uuid> = rows.iter().filter(|r| !r.scope_all).map(|r| r.id).collect();
        for s in app_admin_scope_orgs::Entity::find()
            .filter(app_admin_scope_orgs::Column::AppAdminId.is_in(bounded))
            .all(&db)
            .await
            .map_err(|e| {
                tracing::error!("list_app_admins scope query failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
        {
            scopes.entry(s.app_admin_id).or_default().push(s.org_id);
        }
    }

    // One grant read for the caller, then a pure decision per row. `may_delegate` does
    // no IO precisely so this cannot become a query per staff member.
    let facts = delegation::actor_facts(&db, &actor).await?;

    Ok(Json(
        rows.into_iter()
            .map(|m| {
                let id = m.id;
                let role = m.role.clone();
                let scope_all = m.scope_all;
                let scope_org_ids = scopes.remove(&id).unwrap_or_default();
                // Same helper the writes use, so a row can never render writable and
                // then 403 — or render locked to the owner, who can in fact write it.
                let can_manage = delegatable(&facts, &role, scope_all, &scope_org_ids).is_ok();
                let mut r: AppAdminResponse = m.into();
                r.scope_org_ids = scope_org_ids;
                r.can_manage = can_manage;
                r
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct CreateAppAdminBody {
    pub email: String,
    /// Omitted = `global_admin`, which is what every row meant before roles existed.
    /// Defaulting to the *broad* role is the wrong direction for least privilege, but
    /// it is the right direction for compatibility: an existing client that posts only
    /// an email must keep creating the grant it always did.
    #[serde(default)]
    pub role: Option<String>,
    /// Omitted = unbounded. Supplying an empty list bounds the grant to nothing, which
    /// is a valid (if useless) grant and deliberately not an error — the alternative is
    /// silently promoting it to unbounded.
    #[serde(default)]
    pub scope_org_ids: Option<Vec<Uuid>>,
}

pub async fn create_app_admin(
    State(_): State<AppState>,
    actor: oxy_app_core::audit::RequestActor,
    Json(body): Json<CreateAppAdminBody>,
) -> Result<Json<AppAdminResponse>, delegation::Refusal> {
    let email = body.email.trim().to_ascii_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(StatusCode::BAD_REQUEST.into());
    }

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("create_app_admin DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Reject a role this build cannot expand, rather than storing a string the loader
    // will later drop. The write side and `PlatformRole::from_str` must agree, or a
    // typo becomes a grant that silently authorizes nothing.
    let role = match body.role.as_deref() {
        None => PlatformRole::GlobalAdmin,
        Some(r) => PlatformRole::from_str(r).ok_or(StatusCode::BAD_REQUEST)?,
    };
    let scope_all = body.scope_org_ids.is_none();
    let scope_org_ids = body.scope_org_ids.unwrap_or_default();
    let scope = if scope_all {
        Scope::All
    } else {
        Scope::Orgs(scope_org_ids.clone())
    };

    // ── The delegation bound, both halves ────────────────────────────────────────
    //
    // This endpoint is an upsert, so it is two operations wearing one name: it issues a
    // grant, and — when the email already holds one — it destroys the grant that was
    // there. Each needs its own authorization, against a different pair of values.
    //
    // Checking only the incoming values is the mistake worth naming: a Global Admin
    // POSTing `{email: <a peer global_admin>, role: "app_operator"}` writes something
    // admissible (app_operator is below them) on top of something that is not (a peer's
    // global_admin row). One check, and demoting every peer is a single request.
    let facts = delegation::actor_facts(&db, &actor).await?;
    delegation::refuse(
        may_delegate(&facts, role, &scope),
        actor.email.as_deref().unwrap_or(""),
    )?;

    // **Upsert, not create-or-ignore.** This used to return the existing row untouched,
    // which made a grant permanently unchangeable: with no PATCH on the router, there
    // was no way to downgrade a Global Admin to App Operator or bound an unbounded
    // grant — and the call answered 200 with the OLD role, so the caller was told a
    // change landed that never did. Enforcement with no usable write path is the exact
    // failure this whole model exists to correct; a half-present one is worse, because
    // it lies.
    //
    // `ON CONFLICT (email) DO UPDATE` rather than read-then-branch: the check-then-write
    // shape let two concurrent POSTs for a new email both see "absent", both insert, and
    // one die on the unique index. Rare and owner-only, but the atomic form is no harder
    // to read and has no such window.
    //
    // `granted_by` follows the most recent decision — the audit question an owner asks
    // is "who set it to THIS", not "who first granted anything" — and `updated_at`
    // records when, which `created_at` can no longer answer now that rows mutate.
    //
    // Both timestamps come from the SAME instant. Leaving `created_at` NotSet lets it
    // fall to the column default `now()` — the DATABASE clock, evaluated when the
    // statement runs — while `updated_at` carries the app clock, read before the
    // statement was even sent. The two never match, so "never changed" (rendered as an
    // em-dash) became unreachable for every console-issued grant, and a brand-new one
    // typically showed Changed *earlier* than Added.
    //
    // The other two write paths hid it: the migration backfills the two equal, and the
    // env bootstrap leaves both NotSet so they share one transaction timestamp. Only
    // this path — the one the column was added for — got it wrong, which is why
    // verifying the migration against a real database said nothing about it.
    let txn = db.begin().await.map_err(|e| {
        tracing::error!("create_app_admin begin failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // What is there now — the row this upsert would destroy, read INSIDE the transaction
    // and locked (`SELECT … FOR UPDATE`), so the authorization decision below and the
    // write that acts on it see the same row.
    //
    // Read on `&db` before the transaction opened, this was a decision made against a
    // snapshot the write did not hold: a concurrent promotion between the two turned an
    // authorized overwrite of an `app_operator` row into an unauthorized overwrite of a
    // `global_admin` one. Narrow — it requires racing another grant write on the same
    // address — but it is the whole check, so a narrow window is still the wrong shape.
    //
    // What this does NOT close, stated rather than implied: an email with no row yet has
    // nothing to lock, so two concurrent creates still resolve by `ON CONFLICT` with only
    // the first one's authorization considered. And the actor's own standing comes from
    // the TTL-cached `platform_grant_checked`, so a demotion elsewhere lags by up to that
    // TTL — a wider window than this one, shared with every capability decision in the
    // console, and not this endpoint's to fix.
    let existing = AppAdmins::find()
        .filter(app_admins::Column::Email.eq(email.clone()))
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(|e| {
            tracing::error!("create_app_admin existing-grant read failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    // Bound outside the `if let` because the audit row below needs it too: recording a
    // `before` of `{role, scope_all}` alone makes a re-scope illegible — Orgs([acme]) →
    // Orgs([globex]) writes an identical before/after shape, and re-scoping is the most
    // likely edit this console sees.
    let mut prev_scope_orgs: Vec<Uuid> = Vec::new();
    if let Some(prev) = &existing {
        if !prev.scope_all {
            prev_scope_orgs = app_admin_scope_orgs::Entity::find()
                .filter(app_admin_scope_orgs::Column::AppAdminId.eq(prev.id))
                .all(&txn)
                .await
                .map_err(|e| {
                    tracing::error!("create_app_admin existing-scope read failed: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
                .into_iter()
                .map(|r| r.org_id)
                .collect();
        }
        delegation::refuse(
            delegatable(&facts, &prev.role, prev.scope_all, &prev_scope_orgs),
            actor.email.as_deref().unwrap_or(""),
        )?;
    }

    let now = chrono::Utc::now().fixed_offset();
    app_admins::Entity::insert(app_admins::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(email.clone()),
        granted_by: ActiveValue::Set(Some(actor.id)),
        created_at: ActiveValue::Set(now),
        role: ActiveValue::Set(role.as_str().to_string()),
        scope_all: ActiveValue::Set(scope_all),
        updated_at: ActiveValue::Set(now),
    })
    .on_conflict(
        sea_orm::sea_query::OnConflict::column(app_admins::Column::Email)
            .update_columns([
                app_admins::Column::Role,
                app_admins::Column::ScopeAll,
                app_admins::Column::GrantedBy,
                app_admins::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec(&txn)
    .await
    .map_err(|e| {
        tracing::error!("create_app_admin upsert failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Re-read rather than trust the insert's returned id: on the conflict path the row
    // keeps its ORIGINAL id, and the scope rows below are keyed by it.
    let model = AppAdmins::find()
        .filter(app_admins::Column::Email.eq(email.clone()))
        .one(&txn)
        .await
        .map_err(|e| {
            tracing::error!("create_app_admin read-back failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let admin_id = model.id;

    // Scope is REPLACED, never merged: the request states the whole reach, so stale rows
    // from a previous bounding must go or a re-scoped grant keeps orgs the owner just
    // removed. Delete-then-insert in that order — the window between them reaches
    // fewer orgs, never more.
    app_admin_scope_orgs::Entity::delete_many()
        .filter(app_admin_scope_orgs::Column::AppAdminId.eq(admin_id))
        .exec(&txn)
        .await
        .map_err(|e| {
            tracing::error!("create_app_admin scope clear failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Scope rows go in AFTER the grant row (FK) and only when bounded.
    //
    // This used to run outside a transaction, arguing that the partial state (bounded
    // grant, no scope rows) reaches NOTHING and is therefore the safe one. True, but it
    // stopped being the whole story once the audit row below joined the write: "the
    // grant changed and nothing recorded it" is not a safe partial state, and this is
    // now a write more than one person can make. All-or-nothing is both safe AND
    // leaves no half-state to explain.
    if !scope_all && !scope_org_ids.is_empty() {
        let rows: Vec<_> = scope_org_ids
            .iter()
            .map(|org_id| app_admin_scope_orgs::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                app_admin_id: ActiveValue::Set(admin_id),
                org_id: ActiveValue::Set(*org_id),
                created_at: ActiveValue::NotSet,
                created_by: ActiveValue::Set(Some(actor.id)),
            })
            .collect();
        app_admin_scope_orgs::Entity::insert_many(rows)
            .exec(&txn)
            .await
            .map_err(|e| {
                tracing::error!("create_app_admin scope insert failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    // In the SAME transaction as the grant. A platform grant is the most consequential
    // write on this console — it is the authority to reach other tenants — and until
    // now it recorded nothing at all. That was survivable while only the Global Owner
    // could make it; with Global Admins delegating, "who made this person staff" needs
    // an answer that is not "check the logs".
    //
    // `before`/`after` carry the full (role, scope) pair on both sides, so a re-scope
    // is legible as a change and not merely as "touched".
    audit::record_in_txn(
        &txn,
        audit::AuditEntry::for_request(
            &actor,
            if existing.is_some() {
                "platform.grant.updated"
            } else {
                "platform.grant.created"
            },
        )
        .target("platform_grant", admin_id.to_string(), email.clone())
        .change(
            match &existing {
                Some(prev) => serde_json::json!({
                    "role": prev.role,
                    "scope_all": prev.scope_all,
                    "scope_org_ids": prev_scope_orgs,
                }),
                None => serde_json::Value::Null,
            },
            serde_json::json!({
                "role": role.as_str(),
                "scope_all": scope_all,
                "scope_org_ids": scope_org_ids,
            }),
        ),
    )
    .await
    .map_err(|e| {
        tracing::error!("create_app_admin audit write failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("create_app_admin commit failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // AFTER commit: invalidating earlier lets a concurrent read repopulate the cache
    // from the pre-commit snapshot and pin the stale grant for a full TTL.
    invalidate_admin_cache();
    let mut response: AppAdminResponse = model.into();
    // The response must describe what was just WRITTEN — `From<Model>` defaults
    // `scope_org_ids` to empty, which would render an unbounded-looking reach for a
    // grant that is in fact bounded. No branch on `scope_all` needed: it is exactly
    // `body.scope_org_ids.is_none()`, so this vector is already empty when it's true.
    response.scope_org_ids = scope_org_ids;
    // The write just cleared the bound, so this row is writable by definition. Leaving
    // the fail-closed default here would ship a payload contradicting the list the
    // mutation is about to invalidate.
    response.can_manage = true;
    Ok(Json(response))
}

/// Revoke a grant.
///
/// **The bound is checked against the row being deleted, not against anything the
/// caller supplies.** A revoke has no incoming `(role, scope)` to authorize — the only
/// values in play are the target's — so reading the row first is not an optimization,
/// it is the entire check. Without it `Cap::ManagePlatformGrants` would let one Global
/// Admin strip every peer, and the delegation bound would hold on create while leaking
/// on delete.
pub async fn delete_app_admin(
    actor: oxy_app_core::audit::RequestActor,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, delegation::Refusal> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("delete_app_admin DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // The caller's own standing, read BEFORE the transaction opens.
    //
    // `actor_facts` goes to `&db`, so calling it while this request already holds a
    // connection and a locked row borrows a second one from the pool — and it is a
    // guaranteed cache miss, because `invalidate_admin_cache()` runs after every write to
    // this table. N concurrent revokes against an N-connection pool wedge each other
    // until the acquire timeout, each holding a lock nobody can clear.
    //
    // Nothing about the decision moves with it: this is the caller's standing, not the
    // target's, and it already tolerates TTL staleness by design. `create_app_admin`
    // loads it here for the same reason.
    let facts = delegation::actor_facts(&db, &actor).await?;

    // Transaction first, then read the target under `SELECT … FOR UPDATE`.
    //
    // Create closes this window and delete did not, which is the asymmetry that matters:
    // a concurrent promotion between an unlocked read and the delete turns an authorized
    // revoke of an `app_operator` into an unauthorized revoke of a `global_admin`. The
    // doc on this function calls reading the row "the entire check" — a check performed
    // against a row the delete does not hold is not the entire check.
    //
    // It also makes the audit `before` describe the row actually removed.
    let txn = db.begin().await.map_err(|e| {
        tracing::error!("delete_app_admin begin failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let target = AppAdmins::find_by_id(id)
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(|e| {
            tracing::error!("delete_app_admin target read failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let scope_org_ids: Vec<Uuid> = if target.scope_all {
        Vec::new()
    } else {
        app_admin_scope_orgs::Entity::find()
            .filter(app_admin_scope_orgs::Column::AppAdminId.eq(id))
            .all(&txn)
            .await
            .map_err(|e| {
                tracing::error!("delete_app_admin scope read failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .into_iter()
            .map(|r| r.org_id)
            .collect()
    };

    delegation::refuse(
        delegatable(&facts, &target.role, target.scope_all, &scope_org_ids),
        actor.email.as_deref().unwrap_or(""),
    )?;

    let res = AppAdmins::delete_by_id(id).exec(&txn).await.map_err(|e| {
        tracing::error!("delete_app_admin failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if res.rows_affected == 0 {
        // Unreachable now that the read above holds `FOR UPDATE` on this row for the
        // life of the transaction — kept as the defensive default. If it ever fires,
        // the lock was dropped and that is the thing to look at, not this branch.
        return Err(StatusCode::NOT_FOUND.into());
    }

    audit::record_in_txn(
        &txn,
        audit::AuditEntry::for_request(&actor, "platform.grant.revoked")
            .target("platform_grant", id.to_string(), target.email.clone())
            .change(
                serde_json::json!({
                    "role": target.role,
                    "scope_all": target.scope_all,
                    "scope_org_ids": scope_org_ids,
                }),
                serde_json::Value::Null,
            ),
    )
    .await
    .map_err(|e| {
        tracing::error!("delete_app_admin audit write failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("delete_app_admin commit failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    invalidate_admin_cache();
    Ok(StatusCode::NO_CONTENT)
}
