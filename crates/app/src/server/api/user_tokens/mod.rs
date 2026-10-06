//! Personal access tokens over HTTP (API-tokens design §3, §8 Phase 2).
//!
//! - `/api/user/tokens` — the caller's personal tokens **and** legacy keys:
//!   list, create, read, edit, extend, regenerate, revoke, activity
//!   ([`handlers`]), over [`service`];
//! - `GET /api/user/token-options` — what the create dialog may offer
//!   ([`options`]);
//! - `GET /api/{workspace_id}/api-tokens` — the tokens that can reach one
//!   workspace, for its admins ([`inventory`]);
//! - `GET|DELETE /api/auth/token` — the calling token, about itself
//!   ([`introspect`]).
//!
//! **Management is session-only** (§4.6): every `/api/user/…` route here takes
//! [`ManageTokens`], so a token cannot mint, widen, extend or revoke a token.
//! The introspection pair is the exception — it acts only on the credential
//! that calls it.
//!
//! **A legacy key is listed and never narrowed** (§3.5). Its owner may rename,
//! extend and revoke it; anything else answers 409 `legacy_immutable`. "Legacy"
//! is every row that mirrors `api_keys` — an `oxy_<hex>` key, and a token the
//! legacy `/api/{workspace_id}/api-keys` endpoint minted.

// `audit`, `dto` and `view` are shared with the org's side of the same tokens
// (`api::org_api_access`): one wire shape and one lifecycle-event writer.
pub(crate) mod audit;
pub mod cli_login;
pub(crate) mod dto;
pub mod error;
pub mod handlers;
pub mod hygiene;
pub mod introspect;
pub mod inventory;
pub mod leak;
pub mod options;
pub(crate) mod policy_cap;
mod policy_view;
mod reach;
pub(crate) mod recipients;
pub mod sandbox;
pub(crate) mod sandbox_staff;
pub(crate) mod sandboxes_queued;
mod service;
mod system_audit;
pub(crate) mod view;

use oxy_auth::extractor::{SESSION_REQUIRED, SessionAction};

/// The session-only gate on token management, with the contract's 403 body.
pub struct ManageTokens;

impl SessionAction for ManageTokens {
    const REFUSAL: &'static str = "managing API tokens requires a browser session";
    const CODE: Option<&'static str> = Some(SESSION_REQUIRED);
}
