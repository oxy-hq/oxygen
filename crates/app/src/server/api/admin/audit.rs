//! `/api/admin/audit` — platform audit-log search for Oxy staff.
//!
//! Read-only view over the append-only `audit_events` stream. Gated on
//! `Action::PlatformAudit` (`view_audit`) by `admin::router`, and narrowed to the
//! caller's grant scope here — the gate cannot see scope. Filtering, scoping and
//! paging are all done in the DB via [`audit::search_events`].

use axum::Json;
use axum::Router;
use axum::extract::{OriginalUri, Query};
use axum::http::StatusCode;
use axum::routing::get;
use oxy::database::client::establish_connection;
use oxy_app_core::pagination::{self, Paged, trim_overfetch};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::router::AppState;
use oxy_app_core::audit::{self, AuditFilter};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/audit", get(list_audit))
        // Makes the tamper-evident chain actually falsifiable (review #9): walks
        // one org's chain in seq order and recomputes every link.
        .route("/audit/verify/{org_id}", get(verify_audit_chain))
        // The chain checked against something the database cannot rewrite:
        // the latest S3 Object Lock anchor (`server::audit_anchor`).
        .route("/audit/anchor/{org_id}", get(verify_audit_anchor))
}

/// `GET /admin/audit/verify/{org_id}` — recompute an org's hash chain.
pub async fn verify_audit_chain(
    oxy_auth::extractor::AuthenticatedUserExtractor(actor): oxy_auth::extractor::AuthenticatedUserExtractor,
    axum::extract::Path(org_id): axum::extract::Path<Uuid>,
) -> Result<Json<audit::ChainReport>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("admin/audit: DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // Scope. Found by the merge-derived coverage test on its first run — a fifth
    // `{org_id}` router nobody had swept. Verifying another tenant's hash chain is a
    // read of that tenant's audit trail.
    crate::server::api::admin::scope::deny_out_of_scope(&db, &actor, org_id).await?;
    audit::verify_chain(&db, org_id)
        .await
        .map(Json)
        .map_err(|e| {
            tracing::error!("admin/audit: chain verification failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `GET /admin/audit/anchor/{org_id}` — compare an org's chain with the most
/// recent anchor written to S3 under Object Lock. `configured: false` when no
/// anchor bucket is set.
pub async fn verify_audit_anchor(
    oxy_auth::extractor::AuthenticatedUserExtractor(actor): oxy_auth::extractor::AuthenticatedUserExtractor,
    axum::extract::Path(org_id): axum::extract::Path<Uuid>,
) -> Result<Json<crate::server::audit_anchor::AnchorReport>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("admin/audit: DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // Same scope rule as the chain verifier: another tenant's anchor is a
    // read of that tenant's audit trail.
    crate::server::api::admin::scope::deny_out_of_scope(&db, &actor, org_id).await?;
    let cfg = crate::server::audit_anchor::AnchorConfig::from_env();
    let s3 = crate::server::api::custom_apps_storage::s3::client().await;
    // A verification that cannot complete — an unlocked or lapsed object, a
    // rewritten pointer, S3 unreachable — is itself what the operator came to
    // learn, so it is a 200 with `present`/`matches` false and the reason in
    // `detail`, not a bare 500 with the reason in a log line.
    Ok(Json(
        match crate::server::audit_anchor::verify_latest(&db, &s3, cfg.as_ref(), org_id).await {
            Ok(report) => report,
            Err(reason) => {
                tracing::warn!(%org_id, %reason, "admin/audit: anchor verification failed");
                crate::server::audit_anchor::AnchorReport::failed(org_id, reason)
            }
        },
    ))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub org_id: Option<Uuid>,
    pub outcome: Option<String>,
    /// Free-text search across action / actor / target label.
    pub q: Option<String>,
    /// One API token: actions performed with it and its lifecycle events.
    pub token_id: Option<Uuid>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Serialize)]
pub struct AuditEventDto {
    pub id: Uuid,
    pub created_at: String,
    pub actor_email: String,
    pub actor_type: String,
    pub action: String,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub partner_id: Option<Uuid>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub target_label: Option<String>,
    pub outcome: String,
    pub reason: Option<String>,
    /// Staff/org action taken through the assume-role override — the single most
    /// important thing to spot when auditing, so it's lifted out of `metadata`.
    pub via_global_override: bool,
}

impl From<entity::audit_events::Model> for AuditEventDto {
    fn from(e: entity::audit_events::Model) -> Self {
        let via_global_override = e
            .metadata
            .get("via_global_override")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Self {
            id: e.id,
            created_at: e.created_at.to_rfc3339(),
            actor_email: e.actor_email,
            actor_type: e.actor_type,
            action: e.action,
            org_id: e.org_id,
            workspace_id: e.workspace_id,
            partner_id: e.partner_id,
            target_type: e.target_type,
            target_id: e.target_id,
            target_label: e.target_label,
            outcome: e.outcome,
            reason: e.reason,
            via_global_override,
        }
    }
}

/// `GET /admin/audit` — search the audit stream, newest first.
///
/// **Narrowed by the caller's grant scope, in the query.** `view_audit` is held by every
/// Global Admin whatever their bound, and the capability gate cannot see scope, so
/// unfenced this handed a grant bounded to one org every other tenant's trail. A bounded
/// reader gets the events of the orgs their grant names and nothing else: asking for
/// `?org_id=` outside it returns an empty page, and an event with no org — a
/// platform-level action — is visible only to an unbounded grant and the Global Owner.
pub async fn list_audit(
    oxy_auth::extractor::AuthenticatedUserExtractor(actor): oxy_auth::extractor::AuthenticatedUserExtractor,
    OriginalUri(uri): OriginalUri,
    Query(q): Query<AuditQuery>,
) -> Result<Paged<AuditEventDto>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("admin/audit: DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let filter = AuditFilter {
        action: q.action,
        actor: q.actor,
        org_id: q.org_id,
        outcome: q.outcome,
        q: q.q,
        token_id: q.token_id,
        org_scope: crate::server::api::admin::scope::list_scope(&db, &actor).await?,
    };
    // CLAMPED AT BOTH ENDS. `?limit=0` past a top-only clamp is an infinite
    // pagination loop, not an empty page: the over-fetch reads one row,
    // `trim_overfetch` discards it and still reports "more", and the cursor
    // advances by zero — so `rel="next"` points back at this same request.
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let offset = q.offset.unwrap_or(0);
    // `limit + 1`: the extra event is how the `Link: rel="next"` below knows
    // there is another page. An audit log is append-only and only grows, so
    // "did I read all of it" is a question every caller of this endpoint has.
    let mut events = audit::search_events(&db, &filter, limit + 1, offset)
        .await
        .map_err(|e| {
            tracing::error!("admin/audit: search failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let has_more = trim_overfetch(&mut events, limit);
    Ok(pagination::page(
        events.into_iter().map(Into::into).collect(),
        has_more,
        &uri,
        // Saturating: `?offset=<u64::MAX>` panics a debug build here and wraps
        // to a nonsense cursor in release. Saturating pins it at the top, where
        // the query returns nothing and the walk ends.
        &[("offset", offset.saturating_add(limit).to_string())],
    ))
}
