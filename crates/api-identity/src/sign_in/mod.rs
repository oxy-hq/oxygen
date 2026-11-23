//! Signing in over HTTP: magic-link request/verify, OAuth login (Google /
//! Okta / GitHub), OAuth-state CSRF tokens, session-cookie hydration, the
//! public auth-config endpoint, `return_to` redirect validation, the dev
//! sign-in bypass, and the Oxygen mobile app's sign-in.
//!
//! - `dto`: the sign-in routes' request/response serde types.
//! - `ops`: OAuth state and the providers' code exchanges, the magic-link
//!   rate limit, request and email, and the user insert a first sign-in races
//!   on.
//! - `handlers`: the HTTP handler functions themselves.
//! - `dev_login`: `GET|POST /auth/dev-login`, and the allow-list `oxy serve`
//!   announces at boot.
//! - [`mobile`]: the mobile app's authorize, exchange and refresh.
//!
//! The session plumbing every sign-in path shares (the cookie, the JWT, the
//! login payload, a token session's hydration) is `oxy-app-core`'s, below the
//! surfaces (`oxy_app_core::session`): frontline's PIN
//! sign-in and a token's browser sign-in mint the same session.

mod anonymous_link;
mod dev_login;
mod dto;
mod handlers;
pub mod mobile;
mod ops;

// The two handlers are mounted from `lib.rs`, whose signatures name `PeerAddr`;
// the allow-list queries are what `oxy serve` announces at boot.
pub(crate) use dev_login::dev_login_reachable_by;
pub use dev_login::{PeerAddr, dev_login, dev_login_get, dev_login_persona_email};
pub use dev_login::{dev_login_is_loopback_only, dev_login_source, is_dev_login_enabled};
pub use handlers::*;
