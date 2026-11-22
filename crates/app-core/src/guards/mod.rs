//! The request guards every surface layers: role-guard extractors, org context,
//! the paywall's refusal, the staff console's per-app scope fence, and partner
//! authorization. Authorization plumbing — the decisions are `oxy-authz`'s (through
//! `oxy-server-authz`) and, for the paywall, `oxy-billing`'s.
//!
//! Went down from `oxy-app`'s `server::api::middlewares` on 2026-11-22 (decompose
//! G2); `oxy-app` re-exports each module at its old path. The staff console's
//! legacy-term guards (`oxy_app_admin_guard`, `oxy_owner_or_app_admin_guard`,
//! `platform_cap_guard`) went to `oxy-server-authz` instead (G2b), where
//! `authz_boundaries` allowlists them by path.

pub mod app_scope_guard;
pub mod org_context;
pub mod partner_authz;
pub mod role_guards;
pub mod subscription_guard;
