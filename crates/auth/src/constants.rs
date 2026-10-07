//! Auth-related string constants.
//!
//! Header names and JWT secret keys lifted out of `oxy::config::constants` so
//! that `oxy-auth` no longer needs to import from `oxy`. The `oxy` crate
//! re-exports these from `oxy::config::constants` for source compatibility.

pub const DEFAULT_API_KEY_HEADER: &str = "X-API-Key";

/// HTTP header carrying the bearer JWT.
pub const AUTHENTICATION_HEADER_KEY: &str = "authorization";

/// Cookie name carrying the JWT for browser sessions. Set by every login
/// finalize path (Google/GitHub/Okta/magic-link) so that `*.oxygen-hq.com`
/// subdomains can be gated by the external auth proxy at
/// `/api/auth/check`. Read by `BuiltInAuthenticator::extract_token` as a
/// fallback when the `Authorization` header is absent.
pub const SESSION_COOKIE_NAME: &str = "oxy_session";
