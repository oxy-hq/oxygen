//! External protocol integrations: the OAuth "Connect" flow (QuickBooks and
//! the uniform per-provider routes) and the provider table it reads. Out of
//! `oxy-app`'s `integrations` (2026-11-21); the module path inside this crate
//! is the same, so the route catalog's `handler` strings did not change.

pub mod oauth_provider;
pub mod quickbooks;
