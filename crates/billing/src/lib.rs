//! `oxy-billing` — Stripe-backed per-seat subscription logic for Oxy orgs.
//!
//! Layered so platform crates never import this one directly; `oxy-app` and the
//! billing-reading surfaces depend on it. [`gate`] is the subsystem's one switch
//! ([`billing_service`] / [`billing_disabled`]). See `internal-docs/2026-04-24-stripe-billing-design.md` for
//! the authoritative design.

pub mod client;
pub mod config;
pub mod errors;
pub mod gate;
pub mod service;
pub mod state;
pub mod webhook;

pub use config::StripeConfig;
pub use errors::BillingError;
pub use gate::{billing_disabled, billing_service};
pub use service::{BillingService, SubscriptionOverview};
