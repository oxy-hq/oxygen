//! Signing in the Oxygen mobile app, and carrying its session into a WebView
//! (the code store lives in `oxy_auth::token::mobile_login`; read that first).
//!
//! - `POST /api/auth/mobile/authorize` — the browser, under its **session**,
//!   trades the app's S256 challenge and `oxygen://` redirect URI for a
//!   one-time code ([`authorize`]). Session-only, as `oxyc login`'s authorize
//!   is: a token, or the browser session a token opened, answers 403
//!   `session_required` — a credential never signs anything else in.
//! - `POST /api/auth/mobile/exchange` — **public**. The app trades `code +
//!   verifier + redirect_uri` for a login session JWT ([`exchange`]): the
//!   credential `oxy_session` holds after any other login, so session-only
//!   routes work for it too. The code is the only thing that names the user,
//!   so every failure answers the same 400 `invalid_code`.
//! - `POST /api/auth/mobile/refresh` — **public**. `Authorization: Bearer
//!   <jwt>` of a live login session buys a fresh one of the same window
//!   ([`refresh`]).
//!
//! Both answers carry a `session_cookie`: every attribute of the `oxy_session`
//! cookie a browser login would have set ([`cookie`]). The app sets it in its
//! WebView itself; **no route turns a bearer into a cookie**, because a form
//! any site can post that sets the parent-domain session is a login-CSRF
//! surface.
//!
//! Frontline crew are out of scope: a crew member's session cannot approve a
//! sign-in, and a code one approved is not honoured ([`frontline`]).
//!
//! Every route reads and writes Postgres only (`cli_auth_codes`, `users`,
//! `server_keys`), so all three are `route_fleet`.

mod code;
mod cookie;
mod error;
mod frontline;
mod session;

use oxy_auth::extractor::{SESSION_REQUIRED, SessionAction};

pub use code::{authorize, exchange};
pub use session::refresh;

/// The session-only gate on approving a mobile sign-in, with the 403 body
/// every other session-only auth route answers.
pub struct ApproveMobileSignIn;

impl SessionAction for ApproveMobileSignIn {
    const REFUSAL: &'static str = "signing in the mobile app requires a browser session";
    const CODE: Option<&'static str> = Some(SESSION_REQUIRED);
}
