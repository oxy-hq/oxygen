//! Authentication HTTP surface: magic-link request/verify, OAuth login
//! (Google / Okta / GitHub), OAuth-state CSRF tokens, session-cookie
//! hydration, the public auth-config endpoint, and `return_to` redirect
//! validation.
//!
//! - [`dto`]: request/response serde types shared across handlers.
//! - [`ops`]: internal helpers — session-cookie construction, OAuth code
//!   exchange, magic-link email + rate limiting, and login finalization.
//! - [`handlers`]: the HTTP handler functions themselves.

mod dev_login;
mod dto;
mod handlers;
mod ops;

// Frontline sign-in mints the same session cookie as the magic-link path, so
// the installed PWA carries it on navigation without the page holding a token.
// Re-exported rather than making `ops` public: one function crosses the
// boundary, not the module.
pub use ops::{build_session_cookie, build_session_cookie_with_max_age, is_request_secure};

// The two handlers stay `pub` (mounted from `router/public.rs`, whose `pub`
// signatures name `PeerAddr`); every query about the allow-list is crate-only.
pub use dev_login::{PeerAddr, dev_login, dev_login_get};
pub(crate) use dev_login::{
    dev_login_is_loopback_only, dev_login_reachable_by, dev_login_source, is_dev_login_enabled,
};
pub use handlers::*;
pub(crate) use ops::clear_session_cookie;
pub(crate) use ops::validate_return_to_url;
// `pub`: reused by the extracted `oxy-api-partner-console` surface (invite links).
pub use ops::{extract_base_url_from_headers, extract_link_base_for_authenticated_request};
