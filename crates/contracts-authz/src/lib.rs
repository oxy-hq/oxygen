//! `contracts-authz`: the **authorization enforcement driver port** (T1).
//!
//! Three crates hold authorization, one tier each (`internal-docs/crate-tiers.md`,
//! "Where authn and authz live"):
//!
//! * `oxy-authz` (T0) is the model: the one exhaustive `allows()`.
//! * `oxy-server-authz` (T2) is enforcement: it loads the facts, reads platform
//!   and partner standing and assume-role sessions, and holds the guards.
//! * this crate is the port a **domain** enforces through, so that it needs
//!   neither `oxy-server-authz` (a domain-to-domain edge the tiers forbid) nor
//!   `oxy-auth`.
//!
//! It holds:
//!
//! * [`Caller`]: the authenticated principal as plain values, with the pure
//!   reads of what its credential carries (ceilings, `carries_*`, the service
//!   account and sandbox agent branches, the synthesized service-account
//!   membership, the token's expiry). It is built from [`CallerParts`], which
//!   `oxy_auth::caller_parts` projects from `oxy_auth`'s `AuthenticatedUser`
//!   and `CredentialContext`; this crate never names either.
//!   `oxy_server_authz::Caller` wraps it and keeps the extractor and the
//!   request-extension plumbing.
//! * [`cap_org_role`] and [`cap_workspace_role`]: a tenant role under the
//!   token's ceiling (moved from `oxy_server_authz` in C3a).
//! * [`AuthzEnforcer`]: the driver trait, one method per enforcement read a
//!   domain makes, assume-role included. `oxy-server-authz` implements it by
//!   delegating to its existing functions, and the host keeps the one
//!   implementation on `AppState`.
//! * The values those reads answer: [`PlatformFlags`] and [`Checked`],
//!   [`ActingAs`], [`PartnerReach`] and [`AuthzError`].
//! * The enforcer travels **on the caller** ([`Caller::authz`]): every constructor
//!   takes it, so a domain holding a caller can enforce without `AppState`, and
//!   a missing wire is a compile error (C3a).
//! * [`EffectiveWorkspaceRole`]: the resolved workspace role, carried as a
//!   request extension, with its extractors (moved from `oxy_server_authz` in C3a).
//!
//! It depends on `oxy-authz`, `entity` (the org and workspace roles it answers and
//! caps), `sea-orm` (the caller's database handle), `chrono` (a token's expiry and
//! a synthesized membership's timestamps), `uuid`, `async-trait` and `thiserror`;
//! on `http` for the request extensions two trait methods read; and on
//! `axum` for [`EffectiveWorkspaceRole`]'s extractors (C3a reversed C2's "no web
//! framework", for that type alone).

mod assume;
mod caller;
mod enforcer;
mod enforcer_handle;
mod error;
mod grant_memo;
mod partner;
mod platform;
mod role_cap;
pub mod workspace_role;

pub use assume::ActingAs;
pub use caller::Caller;
pub use enforcer::{AuthzEnforcer, Enforce};
pub use error::AuthzError;
pub use grant_memo::GrantMemo;
/// The parts a [`Caller`] is made of. `oxy_authz`'s since C3a, so that the
/// projection that fills them in can live in `oxy-auth`; named here at the
/// paths C2 gave them.
pub use oxy_authz::{CallerAccount, CallerParts, CallerToken, UnixTime};
pub use partner::PartnerReach;
pub use platform::{Checked, PlatformFlags};
pub use role_cap::{cap_org_role, cap_workspace_role};
pub use workspace_role::EffectiveWorkspaceRole;
