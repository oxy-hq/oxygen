//! Billing's base: what stays in `oxy-app` after the admin split's PR 9 moved
//! billing's HTTP surface (the tenant's `/orgs/{org_id}/billing/*` routes, the
//! Stripe webhook and the staff console's billing section) to
//! `oxy-api-billing`.
//!
//! - [`billing_service`] / [`billing_disabled`]: the one switch for the billing
//!   subsystem. `subscription_guard` (the paywall), the seat reconciler in
//!   `router::entry` and `oxy-api-tenancy`'s seat sync read it as well as the
//!   moved routes, and it reads the feature-flag cache, which is `oxy-app`'s.
//! - [`notify`]: the past-due dispatcher. The webhook and the staff resync call
//!   it (through `oxy_app::surface::billing`), and `oxy-api-notifications`'
//!   tests drive its `past_due_notice`, which they could not reach in another
//!   surface (S3).

pub mod notify;

use std::sync::Arc;

use axum::http::StatusCode;
use oxy_billing::BillingService;

/// Construct a `BillingService` on demand. Gated by [`billing_disabled`] so
/// the feature flag is the symmetric authority for both this entry point
/// and `subscription_guard` — disabling the flag at runtime makes both the
/// guard short-circuit AND `billing_service()` return 503 even after the
/// `OnceCell` was previously initialized.
pub async fn billing_service() -> Result<Arc<BillingService>, StatusCode> {
    if billing_disabled() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    use tokio::sync::OnceCell;
    static CELL: OnceCell<Option<Arc<BillingService>>> = OnceCell::const_new();
    let slot = CELL
        .get_or_init(|| async {
            let cfg = oxy_billing::StripeConfig::maybe_from_env()?;
            let client = oxy_billing::client::StripeClient::new(cfg);
            let db = match oxy::database::client::establish_connection().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(?e, "billing_service: DB connect failed — disabling billing");
                    return None;
                }
            };
            Some(Arc::new(BillingService::new(client, db)))
        })
        .await;
    slot.clone().ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// Single authoritative gate for the billing subsystem. True when the
/// `billing` feature flag is off OR the Stripe env vars aren't fully set.
/// Used by `subscription_guard` to short-circuit and by `billing_service()`
/// to refuse construction so handlers can't drift out of sync with the
/// guard.
pub fn billing_disabled() -> bool {
    !crate::server::feature_flags::is_enabled("billing")
        || oxy_billing::StripeConfig::maybe_from_env().is_none()
}
