//! The one switch for the billing subsystem, down from `oxy-app`'s
//! `server::api::billing` (decompose B1). [`billing_disabled`] is what the
//! paywall (`subscription_guard`) consults; [`billing_service`] is the entry
//! point every billing route, the Stripe webhook, the staff console, the seat
//! sync in `oxy-api-tenancy` and the seat reconciler take. Both read the
//! feature-flag cache (`oxy-feature-flags`) and the Stripe env, so neither can
//! drift from the other.

use std::sync::Arc;

use http::StatusCode;

use crate::BillingService;

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
            let cfg = crate::StripeConfig::maybe_from_env()?;
            let client = crate::client::StripeClient::new(cfg);
            let db = match oxy_platform::db::establish_connection().await {
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
    !oxy_feature_flags::is_enabled("billing") || crate::StripeConfig::maybe_from_env().is_none()
}
