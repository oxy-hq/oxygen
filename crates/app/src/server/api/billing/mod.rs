//! Billing's base, now only a re-export. The admin split's PR 9 moved
//! billing's HTTP surface to `oxy-api-billing`; decompose B1 moved the one
//! switch down to `oxy_billing::gate`, re-exported here for
//! `tests/platform`'s `feature_flag_refresh`, which now imports
//! `oxy_billing::billing_disabled` directly from `oxy-workspace-runtime`'s group
//! (decompose R-b), so nothing reads this path and DEL drops it; decompose
//! B2 moved the past-due dispatcher (`notify`) to `oxy-api-billing`'s
//! `billing::notify`, beside its only callers.

pub use oxy_billing::billing_disabled;
