//! Toast POS webhook receiver.
//!
//! Accepts `order_updated` payloads (and the equivalent `channel_order_updated`)
//! and fans them onto the world-model broadcast bus. Only orders that have been
//! paid (non-null `paidDate`, not voided, not deleted) surface as ripples —
//! Toast emits an `order_updated` event on every order mutation, including
//! opens and edits, which would dilute the LIVE EVENTS stream.
//!
//! Schema: <https://doc.toasttab.com/doc/devguide/devOrdersWebhookRef.html>
//!
//! HMAC verification is gated on the workspace's `integrations:` config: when
//! a `toast` integration is declared, its `webhook_secret_var` is resolved
//! from workspace secrets and used to verify the `Toast-Webhook-Signature`
//! header. When the integration is absent the handler fails closed (401)
//! outside local mode, since an unauthenticated endpoint with no signing
//! secret would let anyone forge ripples against a workspace UUID.
//!
//! That fail-closed branch makes the *config source* load-bearing: this route
//! is `FleetOk`, so it runs on stateless `serve` replicas that hold no
//! workspace working copy. It therefore resolves `config.yml` from the compile
//! boundary (`workspace_compiled_configs`) first and only falls back to the FS
//! — otherwise every production delivery reads as "integration not configured"
//! and is rejected. Production workspaces must both configure the integration
//! via Settings → Apps *and* have a promoted revision.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Utc};
use entity::prelude::Workspaces;
use hmac::{Hmac, KeyInit, Mac};
use oxy::adapters::secrets::SecretsManager;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::effective_workspace_path;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::{apps_helpers, resolve_local_workspace_path};
use oxy::database::client::establish_connection;
use sea_orm::EntityTrait;
use serde::Deserialize;
use sha2::Sha256;
use uuid::Uuid;

use crate::server::router::AppState;
use crate::server::service::secret_manager::SecretManagerService;
use oxy::config::ReadOnly;
use oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
use oxy_world_model::live_feed::{OrderEvent, publish_order};

#[derive(Debug, Deserialize)]
pub struct ToastWebhookQuery {
    /// Workspace this webhook belongs to. Carried as a query-param because
    /// Toast can't include arbitrary path segments.
    pub project_id: Uuid,
}

// ── Toast's `order_updated` payload schema ─────────────────────────────────
// Trimmed to what the world-model needs. Full schema has >50 nested fields.
// `serde(rename_all = "camelCase")` matches Toast's JSON conventions.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastWebhookEnvelope {
    pub timestamp: Option<DateTime<Utc>>,
    pub event_category: Option<String>,
    pub event_type: Option<String>,
    #[allow(dead_code)]
    pub guid: Option<String>,
    pub details: ToastEventDetails,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastEventDetails {
    pub restaurant_guid: String,
    pub order: ToastOrder,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastOrder {
    pub guid: String,
    /// `null` when the order has been opened but not yet paid. We only
    /// surface ripples for paid orders.
    pub paid_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub voided: bool,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub checks: Vec<ToastCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastCheck {
    /// Total including tax for this check. Sum across checks for the order
    /// total. `Option` because some webhook fragments omit it.
    pub total_amount: Option<f64>,
}

/// `POST /api/webhooks/toast/orders?project_id=...`
///
/// Pipeline: HMAC verify → parse Toast envelope → filter (paid, not voided)
/// → publish to broadcast bus. Returns `202 Accepted` once parsing succeeds
/// so Toast doesn't retry on slow downstream consumers.
#[axum::debug_handler]
#[tracing::instrument(skip_all)]
pub async fn toast_order_webhook(
    State(app_state): State<AppState>,
    Query(query): Query<ToastWebhookQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, (StatusCode, String)> {
    // Resolve the workspace's Toast integration config (webhook secret +
    // restaurant guid allowlist). `None` means no `toast` integration is
    // declared, so there is no signing secret to verify against.
    let toast = load_toast_config(query.project_id).await?;
    match &toast {
        Some((secret, _)) => verify_hmac(&headers, &body, secret)?,
        None => {
            // This endpoint is unauthenticated and reachable by anyone with a
            // workspace UUID. Without a configured integration there's no
            // signing secret, so we cannot verify the payload. Accepting
            // unsigned events is only safe in single-tenant local dev — fail
            // closed everywhere else so a workspace that hasn't set up Toast
            // (i.e. every workspace on first install) can't have order_ripple
            // events forged against it.
            if !app_state.mode.is_local() {
                tracing::warn!(
                    workspace = %query.project_id,
                    "rejecting unsigned toast webhook: no toast integration configured",
                );
                return Err((
                    StatusCode::UNAUTHORIZED,
                    "toast integration not configured for this workspace".to_string(),
                ));
            }
        }
    }

    let envelope: ToastWebhookEnvelope = serde_json::from_slice(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid JSON: {e}")))?;

    // When an allowlist is configured, drop events for restaurants not
    // on it. Empty list = accept all (matches the config-default).
    if let Some((_, allowlist)) = &toast
        && !allowlist.is_empty()
        && !allowlist.contains(&envelope.details.restaurant_guid)
    {
        tracing::debug!(
            restaurant = %envelope.details.restaurant_guid,
            "skipping event for restaurant outside allowlist",
        );
        return Ok(StatusCode::ACCEPTED);
    }

    let order = envelope.details.order;

    // Skip non-paid / voided / deleted orders. Toast sends `order_updated`
    // on every mutation, so without this filter the LIVE EVENTS stream
    // would fire on opens, edits, voids, etc.
    if order.voided || order.deleted {
        tracing::debug!(order = %order.guid, "skipping voided/deleted order");
        return Ok(StatusCode::ACCEPTED);
    }
    let Some(paid_at) = order.paid_date else {
        tracing::debug!(order = %order.guid, "skipping unpaid order");
        return Ok(StatusCode::ACCEPTED);
    };

    let total: f64 = order.checks.iter().filter_map(|c| c.total_amount).sum();

    // The store, by name, when the org has mapped this restaurant GUID to one
    // of its locations. Best-effort: the ripple is published either way, and
    // a consumer that still keeps its own marker list keeps working.
    let place = resolve_store(query.project_id, &envelope.details.restaurant_guid).await;
    publish_order(
        query.project_id,
        OrderEvent {
            kind: "order_ripple",
            key: envelope.details.restaurant_guid,
            store_name: place.as_ref().map(|p| p.name.clone()),
            amount: total,
            order_id: order.guid,
            ts: paid_at,
            location_id: place.as_ref().map(|p| p.id),
            location_name: place.map(|p| p.name),
        },
    );

    tracing::debug!(workspace = %query.project_id, "toast order webhook accepted");
    Ok(StatusCode::ACCEPTED)
}

/// Resolves the workspace's Toast integration. Returns the webhook signing
/// secret + restaurant_guid allowlist when configured; `None` when no
/// `toast` integration exists. Errors only on DB / config / secrets
/// failures.
///
/// `pub` for the `toast_webhook_compile_boundary` integration test, which
/// drives it against a seeded compiled revision — the handler itself needs an
/// `AppState` and a live axum stack to reach.
pub async fn load_toast_config(
    project_id: Uuid,
) -> Result<Option<(String, Vec<String>)>, (StatusCode, String)> {
    let wm = build_workspace_manager(project_id).await?;
    apps_helpers::resolve_toast(wm.config_manager.get_config(), &wm.secrets_manager)
        .await
        .map_err(|e| {
            tracing::error!(workspace = %project_id, error = %e, "failed to resolve toast config");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to resolve toast config: {e}"),
            )
        })
}

/// Builds a per-request `WorkspaceManager` for an unauthenticated webhook.
/// The Toast handler lives on the public router (no auth middleware), so
/// it loads the workspace row + config manually rather than relying on
/// the workspace-context middleware.
///
/// Config comes from the compile boundary when the workspace has a promoted
/// revision, and only otherwise from `config.yml` on disk. Secrets always come
/// from the workspace secret store (Postgres) with an env fallback, so a
/// replica with no working copy can still verify a signature end-to-end.
async fn build_workspace_manager(
    project_id: Uuid,
) -> Result<WorkspaceManager<ReadOnly>, (StatusCode, String)> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!(workspace = %project_id, error = %e, "db connect failed");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "database unavailable".to_string(),
        )
    })?;
    let workspace_row = Workspaces::find_by_id(project_id)
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!(workspace = %project_id, error = %e, "workspace lookup failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "workspace lookup failed".to_string(),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, "workspace not found".to_string()))?;

    let path = resolve_manager_path(project_id, &workspace_row).await?;
    let secrets =
        SecretsManager::from_database_with_env_fallback(SecretManagerService::new(project_id))
            .map_err(|e| {
                tracing::error!(workspace = %project_id, error = %e, "secrets manager init failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "secrets manager init failed".to_string(),
                )
            })?;
    let builder = config_builder(project_id, &path)
        .await?
        .with_secrets_manager(secrets);
    builder.build().await.map_err(|e| {
        tracing::error!(workspace = %project_id, error = %e, "workspace build failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "workspace build failed".to_string(),
        )
    })
}

/// Workspace directory for this request.
///
/// Local mode: the nil-UUID workspace row carries no `path` column — its
/// directory is the server's resolved local workspace (the startup cwd),
/// exactly as `local_context_middleware` resolves it for the authenticated
/// routes. The cloud path reads `path` off the DB row.
async fn resolve_manager_path(
    project_id: Uuid,
    workspace_row: &entity::workspaces::Model,
) -> Result<std::path::PathBuf, (StatusCode, String)> {
    let invalid = |e: oxy_shared::errors::OxyError| {
        tracing::error!(workspace = %project_id, error = %e, "resolve workspace path failed");
        (
            StatusCode::BAD_REQUEST,
            "invalid workspace path".to_string(),
        )
    };
    if project_id == LOCAL_WORKSPACE_ID {
        return resolve_local_workspace_path().map_err(invalid);
    }
    effective_workspace_path(workspace_row, None)
        .await
        .map_err(invalid)
}

/// `WorkspaceBuilder` carrying this workspace's `Config`.
///
/// The compile boundary is the *primary* source here, not an optimisation:
/// this route is `RouteRole::FleetOk` — declared so at its mount — so it
/// runs on stateless `serve` replicas whose state dir holds no working copy at
/// all, and an FS read there returns nothing. A public webhook carries no
/// branch context, so the hint is `None`: read the promoted default-branch
/// revision.
async fn config_builder(
    project_id: Uuid,
    path: &std::path::Path,
) -> Result<WorkspaceBuilder<ReadOnly>, (StatusCode, String)> {
    let failed = |e: oxy_shared::errors::OxyError| {
        tracing::error!(workspace = %project_id, error = %e, "workspace builder failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "workspace builder failed".to_string(),
        )
    };
    if let Some(revision_id) =
        crate::server::api::compiled_reader::resolve_request_revision(project_id, None).await
    {
        return WorkspaceBuilder::new(project_id)
            .without_working_copy(path, Some(revision_id), oxy::config::OnMissing::Empty)
            .await
            .map_err(failed);
    }

    // No compiled revision. On the ide singleton (and in local mode) the
    // working copy is right there and this reads the real `config.yml`. On a
    // stateless replica the path doesn't exist and `build_with_fallback_config`
    // silently substitutes an empty `Config` — which downstream is
    // indistinguishable from "the user never configured the integration". Say
    // which one it is, or the next person debugging a dead LIVE EVENTS panel
    // reads a 401 that blames the customer's config.
    if !tokio::fs::try_exists(path.join("config.yml"))
        .await
        .unwrap_or(false)
    {
        tracing::warn!(
            workspace = %project_id,
            path = %path.display(),
            "toast webhook: no compiled config and no config.yml on this node — \
             integrations will resolve as unconfigured. On a stateless serve replica \
             this means the workspace has no promoted revision to read from.",
        );
    }
    WorkspaceBuilder::new(project_id)
        .without_working_copy(path, None, oxy::config::OnMissing::Empty)
        .await
        .map_err(failed)
}

fn verify_hmac(headers: &HeaderMap, body: &[u8], secret: &str) -> Result<(), (StatusCode, String)> {
    // Per Toast docs (`apiMessageSigning.html`): the signature is
    //   base64( HMAC-SHA256( secret_utf8_bytes, body_string + timestamp_string ) )
    // The timestamp comes from the body's top-level `timestamp` field.
    // (Toast's sample Java code: `String signature = body + timestamp;`.)
    //
    // `Toast-Webhook-Signature` is the canonical header name; keep
    // `Toast-Signature` as a fallback for older test harnesses.
    let provided = headers
        .get("Toast-Webhook-Signature")
        .or_else(|| headers.get("Toast-Signature"))
        .and_then(|v| v.to_str().ok())
        .ok_or((StatusCode::UNAUTHORIZED, "missing signature".to_string()))?;
    let provided_bytes = BASE64.decode(provided.trim()).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "signature not valid base64".to_string(),
        )
    })?;

    // Pull the timestamp out of the body without fully parsing the
    // envelope — keeps signing bytes-exact (no whitespace / key-order
    // mutation) even if the JSON is reformatted later.
    let timestamp = extract_timestamp(body).ok_or((
        StatusCode::UNAUTHORIZED,
        "no `timestamp` field in body to sign with".to_string(),
    ))?;

    let try_sign = |message_suffix: &str| -> Result<Vec<u8>, (StatusCode, String)> {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        mac.update(body);
        mac.update(message_suffix.as_bytes());
        Ok(mac.finalize().into_bytes().to_vec())
    };

    // Toast docs say body + timestamp; in practice some webhook variants
    // sign just the body. Accept either so a misconfigured subscription
    // doesn't silently drop events.
    for suffix in [timestamp.as_str(), ""] {
        let expected = try_sign(suffix)?;
        if constant_time_eq(&provided_bytes, &expected) {
            return Ok(());
        }
    }
    Err((StatusCode::UNAUTHORIZED, "bad signature".to_string()))
}

// Pull the top-level `timestamp` string out of a JSON body without
// decoding the full envelope. Returns the raw string (e.g.
// "2026-05-27T08:36:02.719Z"), or None if not present.
fn extract_timestamp(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("timestamp")?.as_str().map(str::to_string)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The org location a Toast restaurant GUID names, if the tenant mapped it
/// (`location_external_ids`, system `toast`). `None` for every failure:
/// the webhook must accept the order whether or not the store is known.
async fn resolve_store(
    project_id: Uuid,
    restaurant_guid: &str,
) -> Option<crate::server::api::operating_graph::binding::LocationBrief> {
    let db = establish_connection().await.ok()?;
    let org_id = Workspaces::find_by_id(project_id)
        .one(&db)
        .await
        .ok()
        .flatten()?
        .org_id?;
    match crate::server::api::operating_graph::binding::locations_by_external_ids(
        &db,
        org_id,
        "toast",
        &[restaurant_guid.to_string()],
    )
    .await
    {
        Ok(mut places) => places.remove(restaurant_guid),
        Err(e) => {
            tracing::debug!(workspace = %project_id, error = %e, "toast store not resolved");
            None
        }
    }
}
