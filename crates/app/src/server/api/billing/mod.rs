//! Billing's base, now only a re-export. The admin split's PR 9 moved
//! billing's HTTP surface to `oxy-api-billing`; decompose B1 moved the one
//! switch ([`billing_service`] / [`billing_disabled`]) down to
//! `oxy_billing::gate`, re-exported here because `tests/platform` reads it at
//! this path; decompose
//! B2 moved the past-due dispatcher (`notify`) to `oxy-api-billing`'s
//! `billing::notify`, beside its only callers.

pub use oxy_billing::{billing_disabled, billing_service};
