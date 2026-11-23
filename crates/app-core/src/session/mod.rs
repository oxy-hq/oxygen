//! The session plumbing every sign-in path shares: the session JWT and the
//! `oxy_session` cookie that carries it, the login payload, a token session's
//! response and hydration, `return_to` validation and the link bases.
//!
//! A `Request` module of the app-core charter (`charter_tests.rs`), not a
//! context: the sign-in routes themselves (magic link, OAuth, dev sign-in, the
//! mobile app's sign-in, `/user` and `/logout`) are `oxy-api-identity`'s, and
//! import this directly (the prelude's `session` until decompose R1), beside frontline's PIN
//! sign-in and a token's browser sign-in, which mint the same session. Down from
//! `oxy-app`'s `server::api::auth` on 2026-11-21, which re-exports it.
//!
//! - [`dto`]: the session's own shapes (`AuthResponse`, `UserInfo`, `OrgInfo`,
//!   the JWT `Claims`).
//! - [`ops`]: minting the token, the cookie and its attributes, the login
//!   response, `return_to` validation and the link bases.
//! - [`token_session`]: the login response of a session a personal access
//!   token opened, and its cookie hydration.

mod cookie_session;
mod dto;
mod ops;
mod token_session;

pub use dto::{AuthResponse, Claims, OrgInfo, UserInfo};
pub use ops::{
    LOCAL_DEFAULT_BASE, SESSION_TTL_SECS, SessionCookieAttributes, build_session_cookie,
    build_session_cookie_with_max_age, clear_session_cookie, create_auth_token,
    create_auth_token_with_ttl, extract_base_url_from_headers,
    extract_link_base_for_authenticated_request, finalize_login, is_request_secure, login_response,
    origin_named_by_request, pin_org_subdomain_to_app_host, render_session_cookie,
    session_cookie_attributes, validate_return_to_url,
};
pub use token_session::{hydrate as hydrate_token_session, token_session_response};

// The kiosk probe (`frontline_devices::device_status`) reports whose session
// a request's cookie carries, decoded without re-minting it.
pub use cookie_session::session_cookie_user_id;
