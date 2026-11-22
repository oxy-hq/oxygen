//! Billing's base: what stays in `oxy-app` after the admin split's PR 9 moved
//! billing's HTTP surface (the tenant's `/orgs/{org_id}/billing/*` routes, the
//! Stripe webhook and the staff console's billing section) to
//! `oxy-api-billing`.
//!
//! - [`billing_service`] / [`billing_disabled`]: the one switch for the billing
//!   subsystem, which moved down to `oxy_billing::gate` (decompose B1) and is
//!   re-exported here so `oxy_app::surface::billing` keeps its shape.
//! - [`notify`]: the past-due dispatcher. The webhook and the staff resync call
//!   it (through `oxy_app::surface::billing`), and `oxy-api-notifications`'
//!   tests drive its `past_due_notice`, which they could not reach in another
//!   surface (S3).

pub mod notify;

pub use oxy_billing::{billing_disabled, billing_service};
