//! Admin billing endpoints — sales-gated provisioning surface.
//!
//! Replaces the pre-redesign `provision-enterprise` endpoint with a single
//! parameterized `provision-subscription` flow that takes Stripe Price IDs
//! (admin selects from `/api/admin/billing/prices` dropdown after creating
//! Prices ad-hoc in the Stripe Dashboard). Plus admin queue listing.
//!
//! `OXY_OWNER` enforcement happens at the router layer via
//! `oxy_owner_guard_middleware`, which `oxy-app`'s `admin::router` applies to
//! the section [`crate::admin_sections`] hands it (it names
//! `Action::PlatformOwnerOnly`); handlers here assume the caller has already
//! been allow-listed.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::TimeZone;
use oxy_billing::BillingError;
use oxy_billing::service::{
    AdminOrgRow, AdminPriceDto, AdminSubscriptionDetail, CheckoutSessionInfo,
    ProvisionCheckoutOutcome, ProvisionCheckoutRequest, ProvisionRequest, ProvisionResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use oxy_app::surface::billing::{billing_service, notify};
use oxy_app_core::AppState;
use oxy_app_core::emails::billing_checkout::{
    CheckoutEmail, EmailSendOutcome, send_checkout_email,
};

#[derive(Deserialize)]
pub struct ListOrgsQuery {
    pub status: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/orgs", get(list_orgs))
        .route("/billing/prices", get(list_prices))
        .route("/orgs/{org_id}/billing/subscription", get(get_subscription))
        .route(
            "/orgs/{org_id}/billing/provision-subscription",
            post(provision_subscription),
        )
        .route(
            "/orgs/{org_id}/billing/provision-checkout",
            post(provision_checkout),
        )
        .route("/orgs/{org_id}/billing/checkout", get(get_checkout))
        .route(
            "/orgs/{org_id}/billing/checkout/resend",
            post(resend_checkout),
        )
        .route(
            "/orgs/{org_id}/billing/checkout/cancel",
            post(cancel_checkout),
        )
        .route("/orgs/{org_id}/billing/resync", post(resync_subscription))
}

pub async fn list_orgs(
    Query(q): Query<ListOrgsQuery>,
) -> Result<Json<Vec<AdminOrgRow>>, StatusCode> {
    let svc = billing_service().await?;
    let page = q.page.unwrap_or(0);
    let page_size = q.page_size.unwrap_or(50).min(200);
    let rows = svc
        .list_admin_orgs(q.status.as_deref(), page, page_size)
        .await
        .map_err(|e| match e {
            BillingError::InvalidStatus(_) => StatusCode::BAD_REQUEST,
            err => {
                tracing::error!(?err, "list_admin_orgs failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;
    Ok(Json(rows))
}

pub async fn list_prices() -> Result<Json<Vec<AdminPriceDto>>, StatusCode> {
    let svc = billing_service().await?;
    svc.list_admin_prices().await.map(Json).map_err(|e| {
        tracing::error!(?e, "list_admin_prices failed");
        StatusCode::BAD_GATEWAY
    })
}

pub async fn get_subscription(
    Path(org_id): Path<Uuid>,
) -> Result<Json<AdminSubscriptionDetail>, Response> {
    let svc = billing_service()
        .await
        .map_err(IntoResponse::into_response)?;
    match svc.admin_subscription_detail(org_id).await {
        Ok(detail) => Ok(Json(detail)),
        Err(BillingError::NoSubscription) => {
            Err(error_body(StatusCode::NOT_FOUND, "no_subscription", None))
        }
        Err(BillingError::Stripe { status, body }) => {
            tracing::error!(
                stripe_status = status,
                stripe_body = %body,
                ?org_id,
                "stripe rejected admin_subscription_detail"
            );
            Err(error_body(
                StatusCode::BAD_GATEWAY,
                "stripe_error",
                Some(extract_stripe_message(&body).unwrap_or(body)),
            ))
        }
        Err(err) => {
            tracing::error!(?err, ?org_id, "admin_subscription_detail failed");
            Err(error_body(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                None,
            ))
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct ResyncRequest {
    /// If true (default), push local member count up to Stripe before
    /// mirroring its state back into `org_billing`. Set to false for a
    /// pull-only resync that won't change Stripe's seat quantity.
    #[serde(default = "default_true")]
    pub sync_seats: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) async fn resync_subscription(
    Path(org_id): Path<Uuid>,
    body: Option<Json<ResyncRequest>>,
) -> Result<StatusCode, Response> {
    let svc = billing_service()
        .await
        .map_err(IntoResponse::into_response)?;
    let sync_seats = body.map(|Json(r)| r.sync_seats).unwrap_or(true);
    match svc.admin_resync_subscription(org_id, sync_seats).await {
        Ok(notifications) => {
            notify::dispatch(notifications, svc.public_url().to_string()).await;
            Ok(StatusCode::NO_CONTENT)
        }
        Err(BillingError::NoSubscription) => {
            Err(error_body(StatusCode::NOT_FOUND, "no_subscription", None))
        }
        Err(BillingError::OrgBillingMissing(_)) => Err(error_body(
            StatusCode::NOT_FOUND,
            "org_billing_missing",
            None,
        )),
        Err(BillingError::Stripe { status, body }) => {
            tracing::error!(
                stripe_status = status,
                stripe_body = %body,
                ?org_id,
                "stripe rejected admin resync"
            );
            Err(error_body(
                StatusCode::BAD_GATEWAY,
                "stripe_error",
                Some(extract_stripe_message(&body).unwrap_or(body)),
            ))
        }
        Err(err) => {
            tracing::error!(?err, ?org_id, "admin_resync_subscription failed");
            Err(error_body(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                None,
            ))
        }
    }
}

pub async fn provision_subscription(
    Path(org_id): Path<Uuid>,
    Json(req): Json<ProvisionRequest>,
) -> Result<Json<ProvisionResponse>, Response> {
    let svc = billing_service()
        .await
        .map_err(IntoResponse::into_response)?;
    match svc.provision_subscription(org_id, req).await {
        Ok(resp) => Ok(Json(resp)),
        Err(err) => Err(map_provision_error(org_id, err)),
    }
}

#[derive(Serialize)]
pub struct ProvisionCheckoutResponseBody {
    session_id: String,
    url: String,
    expires_at: i64,
    email_sent_to: Option<String>,
    email_skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    email_skip_reason: Option<String>,
}

#[derive(Serialize)]
pub struct CheckoutPendingBody {
    session_id: String,
    url: String,
    expires_at: i64,
}

pub async fn provision_checkout(
    Path(org_id): Path<Uuid>,
    Json(req): Json<ProvisionCheckoutRequest>,
) -> Result<Json<ProvisionCheckoutResponseBody>, Response> {
    let svc = billing_service()
        .await
        .map_err(IntoResponse::into_response)?;
    match svc.provision_via_checkout(org_id, req).await {
        Ok(ProvisionCheckoutOutcome::Created(created)) => {
            let outcome =
                deliver_checkout_email(&created.owner_email, &created.org_name, &created.session)
                    .await;
            Ok(Json(build_checkout_response(
                &created.session,
                &created.owner_email,
                outcome,
            )))
        }
        Ok(ProvisionCheckoutOutcome::AlreadyPending(info)) => Err(checkout_pending_response(&info)),
        Err(err) => Err(map_provision_error(org_id, err)),
    }
}

pub async fn get_checkout(Path(org_id): Path<Uuid>) -> Result<Json<CheckoutPendingBody>, Response> {
    let svc = billing_service()
        .await
        .map_err(IntoResponse::into_response)?;
    match svc.get_pending_checkout(org_id).await {
        Ok(Some(info)) => Ok(Json(CheckoutPendingBody {
            session_id: info.session_id,
            url: info.url,
            expires_at: info.expires_at,
        })),
        Ok(None) => Err(error_body(
            StatusCode::NOT_FOUND,
            "no_pending_checkout",
            None,
        )),
        Err(err) => Err(map_provision_error(org_id, err)),
    }
}

pub async fn resend_checkout(
    Path(org_id): Path<Uuid>,
) -> Result<Json<ProvisionCheckoutResponseBody>, Response> {
    let svc = billing_service()
        .await
        .map_err(IntoResponse::into_response)?;
    let info = match svc.get_pending_checkout(org_id).await {
        Ok(Some(info)) => info,
        Ok(None) => {
            return Err(error_body(
                StatusCode::NOT_FOUND,
                "no_pending_checkout",
                None,
            ));
        }
        Err(err) => return Err(map_provision_error(org_id, err)),
    };
    let (owner_email, org_name, _slug) = match svc.org_owner_and_name(org_id).await {
        Ok(t) => t,
        Err(err) => return Err(map_provision_error(org_id, err)),
    };
    let outcome = deliver_checkout_email(&owner_email, &org_name, &info).await;
    Ok(Json(build_checkout_response(&info, &owner_email, outcome)))
}

pub async fn cancel_checkout(Path(org_id): Path<Uuid>) -> Result<StatusCode, Response> {
    let svc = billing_service()
        .await
        .map_err(IntoResponse::into_response)?;
    match svc.cancel_pending_checkout(org_id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(BillingError::NoPendingCheckout) => Err(error_body(
            StatusCode::NOT_FOUND,
            "no_pending_checkout",
            None,
        )),
        Err(err) => Err(map_provision_error(org_id, err)),
    }
}

async fn deliver_checkout_email(
    owner_email: &str,
    org_name: &str,
    session: &CheckoutSessionInfo,
) -> EmailDeliveryResult {
    let expires_at = chrono::Utc
        .timestamp_opt(session.expires_at, 0)
        .single()
        .unwrap_or_else(chrono::Utc::now);
    match send_checkout_email(CheckoutEmail {
        to_email: owner_email,
        org_name,
        checkout_url: &session.url,
        expires_at,
    })
    .await
    {
        Ok(EmailSendOutcome::Sent) => EmailDeliveryResult::Sent,
        Ok(EmailSendOutcome::Skipped { reason }) => EmailDeliveryResult::Skipped(reason),
        Err(err) => {
            tracing::error!(?err, "send_checkout_email failed");
            EmailDeliveryResult::Skipped(format!("send error: {err}"))
        }
    }
}

enum EmailDeliveryResult {
    Sent,
    Skipped(String),
}

fn build_checkout_response(
    session: &CheckoutSessionInfo,
    owner_email: &str,
    outcome: EmailDeliveryResult,
) -> ProvisionCheckoutResponseBody {
    let (email_sent_to, email_skipped, email_skip_reason) = match outcome {
        EmailDeliveryResult::Sent => (Some(owner_email.to_string()), false, None),
        EmailDeliveryResult::Skipped(reason) => (None, true, Some(reason)),
    };
    ProvisionCheckoutResponseBody {
        session_id: session.session_id.clone(),
        url: session.url.clone(),
        expires_at: session.expires_at,
        email_sent_to,
        email_skipped,
        email_skip_reason,
    }
}

fn checkout_pending_response(info: &CheckoutSessionInfo) -> Response {
    let body = serde_json::json!({
        "code": "checkout_already_pending",
        "session_id": info.session_id,
        "url": info.url,
        "expires_at": info.expires_at,
    });
    (StatusCode::CONFLICT, Json(body)).into_response()
}

fn map_provision_error(org_id: Uuid, err: BillingError) -> Response {
    match err {
        BillingError::AlreadyProvisioned => {
            error_body(StatusCode::CONFLICT, "billing_already_provisioned", None)
        }
        BillingError::PriceInactive(id) => error_body(
            StatusCode::BAD_REQUEST,
            "price_inactive",
            Some(format!("Price {id} is inactive")),
        ),
        BillingError::PriceNotRecurring(id) => error_body(
            StatusCode::BAD_REQUEST,
            "price_not_recurring",
            Some(format!("Price {id} is not recurring")),
        ),
        BillingError::UnsupportedPricingMode(id) => error_body(
            StatusCode::BAD_REQUEST,
            "unsupported_pricing_mode",
            Some(format!(
                "Price {id} uses an unsupported pricing mode. Only per-unit or graduated tiered prices are allowed."
            )),
        ),
        err @ BillingError::MismatchedBillingInterval { .. } => error_body(
            StatusCode::BAD_REQUEST,
            "mismatched_billing_interval",
            Some(err.to_string()),
        ),
        BillingError::NoProvisionItems => error_body(
            StatusCode::BAD_REQUEST,
            "no_provision_items",
            Some("At least one price is required.".into()),
        ),
        err @ BillingError::InvalidDaysUntilDue { .. } => error_body(
            StatusCode::BAD_REQUEST,
            "invalid_days_until_due",
            Some(err.to_string()),
        ),
        BillingError::InvalidSeatItemCount(n) => error_body(
            StatusCode::BAD_REQUEST,
            "invalid_seat_item_count",
            Some(format!(
                "Exactly one price must be marked as seat-sync (got {n})."
            )),
        ),
        BillingError::DuplicatePriceItem(id) => error_body(
            StatusCode::BAD_REQUEST,
            "duplicate_price_item",
            Some(format!("Price {id} appears more than once.")),
        ),
        BillingError::Stripe { status, body } => {
            tracing::error!(
                stripe_status = status,
                stripe_body = %body,
                ?org_id,
                "stripe rejected admin checkout call"
            );
            error_body(
                StatusCode::BAD_GATEWAY,
                "stripe_error",
                Some(extract_stripe_message(&body).unwrap_or(body)),
            )
        }
        other => {
            tracing::error!(?other, ?org_id, "admin checkout call failed");
            error_body(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", None)
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn error_body(status: StatusCode, code: &'static str, message: Option<String>) -> Response {
    (status, Json(ErrorBody { code, message })).into_response()
}

/// Stripe error responses look like `{ "error": { "message": "...", "code": ... } }`.
/// Extract the human-readable message; fall back to the raw body if parsing fails.
fn extract_stripe_message(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
}
