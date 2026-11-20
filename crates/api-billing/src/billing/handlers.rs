use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::org_billing::BillingCycle;
use oxy_billing::BillingError;
use oxy_billing::service::AdminSubscriptionItem;
use serde::Serialize;
use uuid::Uuid;

use oxy_app::surface::billing::billing_service;
use oxy_app::surface::feature_flags::is_enabled;
use oxy_app::surface::role_guards::{OrgAdminStrict, OrgMemberStrict};

#[derive(Serialize)]
pub struct BillingSummary {
    pub status: &'static str,
    pub billing_cycle: Option<&'static str>,
    pub seats_used: i32,
    pub seats_paid: i32,
    pub current_period_start: Option<chrono::DateTime<chrono::Utc>>,
    pub current_period_end: Option<chrono::DateTime<chrono::Utc>>,
    pub items: Vec<AdminSubscriptionItem>,
    pub grace_period_ends_at: Option<chrono::DateTime<chrono::Utc>>,
    pub payment_action_url: Option<String>,
}

#[derive(Serialize)]
pub struct UrlResp {
    pub url: String,
}

#[derive(Serialize)]
pub struct InvoiceResp {
    pub id: String,
    pub amount_due: i64,
    pub amount_paid: i64,
    pub currency: String,
    pub status: String,
    pub hosted_invoice_url: Option<String>,
    pub period_start: Option<i64>,
    pub period_end: Option<i64>,
}

/// Full admin summary — pricing items, billing cycle, period boundaries.
/// Admin-only to preserve the no-public-pricing guarantee.
pub async fn get_billing(
    OrgAdminStrict(ctx): OrgAdminStrict,
) -> Result<Json<BillingSummary>, StatusCode> {
    if billing_flag_off() {
        return Ok(Json(billing_disabled_summary()));
    }
    let svc = billing_service().await?;
    let billing = svc.load_billing(ctx.org.id).await.map_err(internal_error)?;
    let seats_used = svc.member_count(ctx.org.id).await.map_err(internal_error)? as i32;
    let effective = oxy_billing::state::effective_status(
        billing.status,
        billing
            .grace_period_ends_at
            .map(|t| t.with_timezone(&chrono::Utc)),
        chrono::Utc::now(),
    );
    // Best-effort overview from Stripe; suppress errors so summary always renders.
    let overview = match svc.current_subscription_overview(ctx.org.id).await {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(?e, "current_subscription_overview failed");
            oxy_billing::SubscriptionOverview::default()
        }
    };
    Ok(Json(BillingSummary {
        status: effective.as_str(),
        billing_cycle: overview.cycle.map(cycle_str),
        seats_used,
        seats_paid: billing.seats_paid,
        current_period_start: billing
            .current_period_start
            .map(|t| t.with_timezone(&chrono::Utc)),
        current_period_end: billing
            .current_period_end
            .map(|t| t.with_timezone(&chrono::Utc)),
        items: overview.items,
        grace_period_ends_at: billing
            .grace_period_ends_at
            .map(|t| t.with_timezone(&chrono::Utc)),
        payment_action_url: billing.payment_action_url,
    }))
}

/// Minimal payload returned by [`get_billing_status`]. Carries no pricing
/// or period info — safe to expose to any org member.
///
/// `payment_action_url` is a Stripe-hosted SCA / card-recovery URL.
/// It is intentionally readable by non-admin members so the FE paywall
/// CTA renders for everyone in the org (otherwise members hit a paywall
/// they can't unblock). Trade-off: any member with the response can open
/// the URL and complete card auth on the org's behalf. Move to
/// [`get_billing`] (admin-only) if that surface area is unacceptable.
#[derive(Serialize)]
pub struct BillingStatusResp {
    pub status: &'static str,
    pub grace_period_ends_at: Option<chrono::DateTime<chrono::Utc>>,
    pub payment_action_url: Option<String>,
}

/// Readable by any org member. Drives the FE paywall, `BillingBanner`,
/// and `OrgGuard` — all of which need to render for non-admin members.
pub async fn get_billing_status(
    OrgMemberStrict(ctx): OrgMemberStrict,
) -> Result<Json<BillingStatusResp>, StatusCode> {
    if billing_flag_off() {
        return Ok(Json(BillingStatusResp {
            status: "active",
            grace_period_ends_at: None,
            payment_action_url: None,
        }));
    }
    let svc = billing_service().await?;
    let billing = svc.load_billing(ctx.org.id).await.map_err(internal_error)?;
    let effective = oxy_billing::state::effective_status(
        billing.status,
        billing
            .grace_period_ends_at
            .map(|t| t.with_timezone(&chrono::Utc)),
        chrono::Utc::now(),
    );
    Ok(Json(BillingStatusResp {
        status: effective.as_str(),
        grace_period_ends_at: billing
            .grace_period_ends_at
            .map(|t| t.with_timezone(&chrono::Utc)),
        payment_action_url: billing.payment_action_url,
    }))
}

pub async fn post_portal(OrgAdminStrict(ctx): OrgAdminStrict) -> Result<Json<UrlResp>, StatusCode> {
    let svc = billing_service().await?;
    let url = svc
        .create_portal_session(ctx.org.id)
        .await
        .map_err(internal_error)?;
    Ok(Json(UrlResp { url }))
}

/// Response for the checkout session verify endpoint. `paid` is true once
/// Stripe reports the session as `status=complete` with the first invoice
/// paid (or `no_payment_required` for trials / manual collection).
#[derive(Serialize)]
pub struct CheckoutSessionStatusResp {
    pub paid: bool,
}

/// Verify a Stripe Checkout Session belongs to this org and has been paid.
/// Used by the customer-facing `/billing/checkout-success` page to confirm
/// the redirect before polling billing status — independent of the webhook.
pub async fn get_checkout_session(
    OrgMemberStrict(ctx): OrgMemberStrict,
    Path((_org_id, session_id)): Path<(Uuid, String)>,
) -> Result<Json<CheckoutSessionStatusResp>, StatusCode> {
    let svc = billing_service().await?;
    let paid = svc
        .verify_checkout_session(ctx.org.id, &session_id)
        .await
        .map_err(|e| match e {
            BillingError::Forbidden(_) => StatusCode::FORBIDDEN,
            err => internal_error(err),
        })?;
    Ok(Json(CheckoutSessionStatusResp { paid }))
}

pub async fn get_invoices(
    OrgAdminStrict(ctx): OrgAdminStrict,
) -> Result<Json<Vec<InvoiceResp>>, StatusCode> {
    let svc = billing_service().await?;
    let invoices = svc
        .list_invoices(ctx.org.id)
        .await
        .map_err(internal_error)?;
    Ok(Json(
        invoices
            .into_iter()
            .map(|i| InvoiceResp {
                id: i.id,
                amount_due: i.amount_due,
                amount_paid: i.amount_paid,
                currency: i.currency,
                status: i.status,
                hosted_invoice_url: i.hosted_invoice_url,
                period_start: i.period_start,
                period_end: i.period_end,
            })
            .collect(),
    ))
}

// ---- internals ----

fn internal_error<E: std::fmt::Display>(e: E) -> StatusCode {
    tracing::error!("billing handler error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

fn cycle_str(c: BillingCycle) -> &'static str {
    match c {
        BillingCycle::Monthly => "monthly",
        BillingCycle::Annual => "annual",
    }
}

/// True when the feature flag alone disables billing. Used by the two
/// member-facing readouts (`get_billing`, `get_billing_status`) that want to
/// return a synthetic "active" payload when billing is administratively off
/// even if Stripe env vars are present (i.e. the flag is the single switch).
fn billing_flag_off() -> bool {
    !is_enabled("billing")
}

fn billing_disabled_summary() -> BillingSummary {
    BillingSummary {
        status: "active",
        billing_cycle: None,
        seats_used: 0,
        seats_paid: 0,
        current_period_start: None,
        current_period_end: None,
        items: Vec::new(),
        grace_period_ends_at: None,
        payment_action_url: None,
    }
}
