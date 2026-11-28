//! The paywall's driven port: what the subscription guard
//! (`crate::guards::subscription_guard`) asks billing.
//!
//! The guard is refusal plumbing: it loads the org's `org_billing` row and
//! turns a refusal into the 402 every org- and workspace-scoped surface returns.
//! The decision itself is billing's: whether billing is switched off, which
//! status the app should observe, and whether a status grants access. Billing
//! lives above this crate, so `oxy-api-billing` fills the port from
//! `oxy_billing` and the composition root hands it in as a required field,
//! [`SurfaceSeams::subscription_gate`](super::SurfaceSeams::subscription_gate):
//! a root that forgets it does not compile. The router layers it onto the API
//! tree as an `Extension`, which the guard reads; a guarded route reached
//! without one answers 500 rather than skip the paywall.
//!
//! Tests that want billing off say so with [`SubscriptionGate::billing_off`],
//! the answer `oxy_billing::billing_disabled` gives wherever the `billing` flag
//! is off or Stripe is not configured.

use chrono::{DateTime, Utc};
use entity::org_billing::BillingStatus;

/// Billing's answers to the paywall. Plain functions, each asked at the moment
/// the guard asked `oxy_billing` before (decompose C5, 2026-11-28).
#[derive(Clone, Copy)]
pub struct SubscriptionGate {
    /// Billing is switched off for this process (the `billing` flag is off, or
    /// Stripe is not configured): the guard lets every request through.
    pub disabled: fn() -> bool,
    /// The status as the rest of the app should observe it, given the row's
    /// status, its grace deadline and now: what the 402 body names.
    pub effective_status: fn(BillingStatus, Option<DateTime<Utc>>, DateTime<Utc>) -> BillingStatus,
    /// Whether the row's status, its grace deadline and now grant access.
    pub grants_access: fn(BillingStatus, Option<DateTime<Utc>>, DateTime<Utc>) -> bool,
}

impl SubscriptionGate {
    /// Billing switched off: the guard lets every request through and never
    /// asks the other two. For [`SurfaceSeams::empty`](super::SurfaceSeams::empty)
    /// and tests that build a router without billing.
    pub fn billing_off() -> Self {
        Self {
            disabled: || true,
            effective_status: |status, _, _| status,
            grants_access: |_, _, _| true,
        }
    }
}
