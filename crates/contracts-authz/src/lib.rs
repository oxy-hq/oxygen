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
//!   account and sandbox agent branches). `oxy_server_authz::Caller` wraps it
//!   and keeps the projection from `oxy_auth`'s `AuthenticatedUser` and
//!   `CredentialContext`, which this crate never names.
//! * [`AuthzEnforcer`]: the driver trait, one method per enforcement read a
//!   domain makes, assume-role included. `oxy-server-authz` implements it by
//!   delegating to its existing functions, and the host keeps the one
//!   implementation on `AppState`.
//! * The values those reads answer: [`PlatformFlags`] and [`Checked`],
//!   [`ActingAs`] and [`AuthzError`].
//!
//! It depends on `oxy-authz`, `entity` (the org role `ActingAs` answers), `sea-orm` (the
//! caller's database handle), `uuid`, `async-trait` and `thiserror`, and on no
//! web framework.

mod assume;
mod caller;
mod enforcer;
mod error;
mod grant_memo;
mod platform;

pub use assume::ActingAs;
pub use caller::{Caller, CallerAccount, CallerParts, CallerToken, UnixTime};
pub use enforcer::{AuthzEnforcer, Enforce};
pub use error::AuthzError;
pub use grant_memo::GrantMemo;
pub use platform::{Checked, PlatformFlags};
