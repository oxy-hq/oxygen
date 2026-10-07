//! API tokens: format, storage and request authentication.
//!
//! Design: `internal-docs/2026-09-30-api-tokens-design.md`. Phase 1 built one
//! token machinery with **no change in what any key can reach**; Phase 2 lets
//! a personal token be narrowed, and leaves every legacy key exactly as it was:
//!
//! - [`format`] — mint `oxy_pat_` tokens, recognise every format, verify the
//!   checksum offline, hash, display;
//! - [`credential`] — the [`CredentialContext`] request marker and the
//!   refuse-what-you-can't-enforce admission check;
//! - [`store`] — the `api_tokens` lookup, the `api_keys` fallback and mirror,
//!   and `last_used_at`;
//! - [`cache`] — the ≤30 s in-process cache;
//! - [`usage`] — per-token daily usage, counted in memory and flushed each minute;
//! - [`dispatch`] — [`authenticate_request`], behind every entry point;
//! - [`personal`] — the writes behind `/api/user/tokens`: mint, grants, edit,
//!   extend, regenerate, revoke;
//! - [`service_account`] — org-owned service accounts and their `oxy_sat_`
//!   tokens; [`account_access`] — their request bodies, parsed and checked;
//! - [`trust_policy`] — the trust policies on a service account, and their
//!   grants; [`trust_policy_access`] — their request bodies, parsed and checked;
//! - [`ci`] — the 15-minute `oxy_ci_` token a trust policy mints for one CI
//!   run, and the sweep of expired ones;
//! - [`exchange`] — which policy a verified run mints from;
//! - [`org_grants`] — an org ending a personal token's reach into it;
//! - [`policy`] — the org token policy, decided from values; [`policy_store`]
//!   — reading and writing it, and which orgs it blocks a token in;
//! - [`access`] — the request bodies of those routes, parsed and checked;
//! - [`grant_plan`] — how an edit's `grants` replaces the stored set;
//! - [`cli_login`] — the `oxyc login` PKCE code store;
//! - [`browser_session`] — the ticket a token trades for a browser session,
//!   and the session, which authenticates as that token;
//! - [`leak`] — what a leak report revokes; [`hygiene`] — the expiry notice
//!   and the unused-token sweep;
//! - [`sandbox`] — the sandbox agent token (`oxy_sbx_`): what a mint asks
//!   for, and the writes behind it.

pub mod access;
pub mod account_access;
pub mod admission;
pub mod browser_session;
pub mod cache;
pub mod ci;
pub mod ci_mint;
pub mod cli_login;
pub mod credential;
pub mod dispatch;
pub mod exchange;
pub mod format;
pub mod grant_plan;
mod grant_row;
pub mod hygiene;
pub mod leak;
pub mod org_grants;
pub mod personal;
pub mod policy;
pub mod policy_store;
pub mod sandbox;
pub mod sandbox_admission;
pub mod sandbox_recheck;
pub mod service_account;
pub mod store;
pub mod trust_policy;
pub mod trust_policy_access;
pub mod usage;

pub use credential::{
    AccountStanding, AppPublishGrant, AppSandboxGrant, CredentialContext, StoredKind,
};
pub use dispatch::{
    AuthSurface, Authenticated, SandboxAgent, authenticate_browser_session, authenticate_request,
    presents_api_token, presents_sandbox_agent,
};
pub use format::{
    TokenFormat, generate_ci, generate_personal, generate_sandbox_agent, generate_service_account,
    hash_token, parse_format, verify_checksum,
};
