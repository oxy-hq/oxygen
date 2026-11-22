//! `POST /webhooks/stripe` — signature-verified webhook receiver.
//!
//! On the happy path returns 200; on apply failure returns 500 so Stripe
//! redelivers (3 days of exponential backoff). The
//! [`stripe_webhook_events`](entity::stripe_webhook_events) row carries a
//! `status` column (`processing`/`success`/`failed`) so a redelivery of an
//! already-applied event short-circuits at the lookup, while a redelivery
//! of a previously-failed event re-runs apply. All apply paths are
//! idempotent (per-org `SELECT ... FOR UPDATE` in
//! `apply_subscription_snapshot` plus stateless writes), so concurrent or
//! repeat applies converge on the same DB state.
//!
//! Note on TOCTOU: `lookup_event` and `upsert_processing` are not in a
//! single transaction, so two concurrent redeliveries of the same event
//! can both observe `NeedsApply` and both proceed to `apply_event`. The
//! final DB state is still safe (apply paths take a per-org row lock and
//! converge), but outbound Stripe calls inside apply (e.g. the
//! flip-collection-method request in `on_invoice_payment_succeeded`) may
//! fire twice. Each such call passes a deterministic `Idempotency-Key`,
//! so Stripe deduplicates server-side and customer impact is zero.

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use serde_json::Value as JsonValue;

use oxy_app::surface::billing::billing_service;
use oxy_billing::service::LookupOutcome;

pub async fn stripe_webhook(headers: HeaderMap, body: Bytes) -> StatusCode {
    let Ok(svc) = billing_service().await else {
        tracing::debug!("/webhooks/stripe called but Stripe isn't configured");
        return StatusCode::NOT_FOUND;
    };

    let sig = match headers
        .get("stripe-signature")
        .and_then(|h| h.to_str().ok())
    {
        Some(s) => s,
        None => return StatusCode::BAD_REQUEST,
    };
    let now = chrono::Utc::now().timestamp();
    if let Err(e) = svc.verify_webhook_signature(&body, sig, now) {
        tracing::warn!(?e, "stripe webhook signature invalid");
        return StatusCode::BAD_REQUEST;
    }

    let payload: JsonValue = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(?e, "stripe webhook malformed");
            return StatusCode::BAD_REQUEST;
        }
    };
    let event_id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let event_type = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if event_id.is_empty() || event_type.is_empty() {
        return StatusCode::BAD_REQUEST;
    }

    match svc.lookup_event(&event_id).await {
        Ok(LookupOutcome::AlreadySuccess) => return StatusCode::OK,
        Ok(LookupOutcome::NeedsApply) => {}
        Err(e) => {
            tracing::error!(?e, event_id = %event_id, "lookup_event failed");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }

    if let Err(e) = svc
        .upsert_processing(&event_id, &event_type, &payload)
        .await
    {
        tracing::error!(?e, event_id = %event_id, "upsert_processing failed");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    match svc.apply_event(&payload).await {
        Ok(notifications) => {
            if let Err(e) = svc.mark_success(&event_id).await {
                // Apply already committed; failing to flip the status row
                // means a redelivery will re-apply (idempotent) and try to
                // mark success again. Log and move on.
                tracing::warn!(?e, event_id = %event_id, "mark_success failed");
            }
            crate::billing::notify::dispatch(notifications, svc.public_url().to_string()).await;
            StatusCode::OK
        }
        Err(e) => {
            let err_msg = e.to_string();
            if let Err(me) = svc.mark_failed(&event_id, &err_msg).await {
                tracing::warn!(?me, event_id = %event_id, "mark_failed failed");
            }
            tracing::error!(?e, event_id = %event_id, "apply failed, asking Stripe to retry");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
