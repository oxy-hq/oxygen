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

/// One row of `GET /admin/audit`: [`AuditEventDto`], plus what only the staff
/// console reads — where the request came from, and the API key or token the
/// row names. Everything here is already on the `audit_events` row.
///
/// A type of its own, not more fields on [`AuditEventDto`]: that shape is also
/// what a key's Activity serves to its owner and to an org's admins
/// (`api_keys::activity`), who are deliberately shown no address or user agent
/// of a request. A field added there would reach them.
#[derive(Serialize)]
pub struct StaffAuditEventDto {
    #[serde(flatten)]
    pub event: AuditEventDto,
    /// The address the load balancer saw; `null` for a row no request wrote.
    pub ip: Option<String>,
    /// As the client sent it, bounded when it was recorded.
    pub user_agent: Option<String>,
    /// The API key or token the row names, by id. When `actor_type` is
    /// `api_key` it is the credential that **performed** the action; on a token
    /// lifecycle event made in a session (`token.created`, …) it is the token
    /// the event is about. `null` when the row names none.
    pub token_id: Option<Uuid>,
    /// That token's name. Stamped on rows a key or token performed; a
    /// lifecycle event carries the name as its `target_label` instead.
    pub token_name: Option<String>,
    /// `personal`, `legacy_key`, `service_account`, `ci` or `sandbox_agent`.
    pub token_kind: Option<String>,
    /// The non-secret display prefix, never the token.
    pub token_prefix: Option<String>,
}

impl From<entity::audit_events::Model> for StaffAuditEventDto {
    fn from(mut e: entity::audit_events::Model) -> Self {
        let named = |key: &str| {
            e.metadata
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let token_id = named(audit::TOKEN_ID_KEY).and_then(|id| Uuid::parse_str(&id).ok());
        let token_name = named("token_name");
        let token_kind = named("token_kind");
        let token_prefix = named("display_prefix");
        Self {
            ip: e.ip.take(),
            user_agent: e.user_agent.take(),
            token_id,
            token_name,
            token_kind,
            token_prefix,
            event: e.into(),
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
) -> Result<Paged<StaffAuditEventDto>, StatusCode> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn row(metadata: Value) -> entity::audit_events::Model {
        entity::audit_events::Model {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now().fixed_offset(),
            actor_user_id: Some(Uuid::new_v4()),
            actor_email: "staff@oxy.tech".into(),
            actor_type: "api_key".into(),
            action: "app.environment.published".into(),
            org_id: Some(Uuid::new_v4()),
            workspace_id: None,
            partner_id: None,
            target_type: Some("custom_app_environment".into()),
            target_id: Some("app/dev-a".into()),
            target_label: Some("orders/dev-a".into()),
            before: None,
            after: None,
            ip: Some("203.0.113.7".into()),
            user_agent: Some("oxyc/0.5.0 agent/claude-code".into()),
            request_id: Some("req-1".into()),
            outcome: "success".into(),
            reason: None,
            metadata,
            prev_hash: None,
            hash: Some("h".into()),
            seq: 1,
            environment: "dev-a".into(),
        }
    }

    #[test]
    fn a_staff_row_says_which_token_from_where_and_with_what_client() {
        let token = Uuid::new_v4();
        let stamped = json!({
            "token_id": token,
            "token_name": "agent on laptop",
            "token_kind": "sandbox_agent",
            "display_prefix": "oxy_sbx_Ab3x",
            "build_id": "b-1",
        });
        let out = serde_json::to_value(StaffAuditEventDto::from(row(stamped))).unwrap();
        assert_eq!(out["token_id"], json!(token));
        assert_eq!(out["token_name"], "agent on laptop");
        assert_eq!(out["token_kind"], "sandbox_agent");
        assert_eq!(out["token_prefix"], "oxy_sbx_Ab3x");
        assert_eq!(out["ip"], "203.0.113.7");
        assert_eq!(out["user_agent"], "oxyc/0.5.0 agent/claude-code");
        // Additive: every field the console already read is still there, flat.
        assert_eq!(out["actor_type"], "api_key");
        assert_eq!(out["action"], "app.environment.published");
        assert_eq!(out["target_label"], "orders/dev-a");
        assert_eq!(out["via_global_override"], false);
        assert!(out.get("event").is_none() && out.get("metadata").is_none());
    }

    #[test]
    fn a_row_that_names_no_token_has_null_token_fields() {
        let mut session = row(json!({ "via_global_override": true }));
        session.ip = None;
        session.user_agent = None;
        let out = serde_json::to_value(StaffAuditEventDto::from(session)).unwrap();
        for key in [
            "token_id",
            "token_name",
            "token_kind",
            "token_prefix",
            "ip",
            "user_agent",
        ] {
            assert_eq!(out[key], Value::Null, "{key}");
            assert!(out.get(key).is_some(), "{key} is present, as null");
        }
        assert_eq!(out["via_global_override"], true);
    }

    /// A key's Activity flattens this shape for the key's owner and for an
    /// org's admins, who must not be handed a request's address or client.
    #[test]
    fn the_shared_row_shape_gains_none_of_the_staff_fields() {
        let stamped = json!({ "token_id": Uuid::new_v4(), "token_name": "n" });
        let out = serde_json::to_value(AuditEventDto::from(row(stamped))).unwrap();
        for key in [
            "ip",
            "user_agent",
            "token_id",
            "token_name",
            "token_kind",
            "token_prefix",
        ] {
            assert!(out.get(key).is_none(), "{key} leaked into the shared shape");
        }
    }
}
