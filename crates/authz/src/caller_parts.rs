//! The parts a request's caller is made of, as plain values.
//!
//! Authentication fills these in (`oxy_auth::caller_parts`, from its
//! `AuthenticatedUser` and `CredentialContext`), and the enforcement port's
//! `contracts_authz::Caller::from_parts` turns them into the caller every
//! authorization read takes. They moved here from `contracts-authz` (C3a,
//! 2026-11-28), which re-exports them at their old paths, so that the projection
//! can live in `oxy-auth`: `oxy-auth` may name this crate, but not the contract
//! (T1 → T1).
//!
//! **Data only.** Nothing here reads a credential or decides anything; a value
//! built from invented parts claims whatever standing the parts claim. Build them
//! with `oxy_auth::caller_parts`, never by hand.

use uuid::Uuid;

use crate::{AccountStanding, AppPublishGrant, PlatformStanding, TokenReach};

/// When a credential ends, as a Unix time: whole seconds and the nanoseconds
/// past them, so it converts back to the timestamp it came from exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnixTime {
    pub secs: i64,
    pub subsec_nanos: u32,
}

/// The new-format token a request authenticated with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallerToken {
    /// `api_tokens.id`, or a configured service's id.
    pub id: Uuid,
    pub reach: TokenReach,
    /// When the credential ends; `None` for never.
    pub expires_at: Option<UnixTime>,
}

/// The service account a request acts as. `standing` is `None` only if the
/// credential named an account without one, which reads as an account with
/// no standing anywhere, never as a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallerAccount {
    pub standing: Option<AccountStanding>,
}

/// Everything a caller is made of: what the projection from the
/// authenticated request fills in.
#[derive(Clone, Debug, Default)]
pub struct CallerParts {
    pub user_id: Uuid,
    /// `""` for a user with no address.
    pub email: String,
    /// `None` for a browser session and for a legacy key.
    pub token: Option<CallerToken>,
    /// Set when the request acts as a service account.
    pub account: Option<CallerAccount>,
    /// The apps the credential's `app_publish` grants name.
    pub app_publish: Vec<AppPublishGrant>,
    /// The standing a configured service's (`oxysvc_`) entry gives it.
    pub service: Option<PlatformStanding>,
}
