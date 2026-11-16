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
//! - [`store`] — the `api_tokens` lookup and `last_used_at`. One lookup: the
//!   `api_keys` fallback and the mirror writes went with legacy keys on
//!   2026-11-16;
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
//! - [`cli_login`] — the `oxyc login` PKCE code store; [`device_login`] — the
//!   same login's device-code requests, for a CLI no browser can reach;
//! - [`browser_session`] — the ticket a token trades for a browser session,
//!   and the session, which authenticates as that token;
//! - [`mobile_login`] — the Oxygen mobile app's PKCE code, bound to its
//!   `oxygen://` redirect URI;
//! - [`leak`] — what a leak report revokes; [`hygiene`] — the expiry notice
//!   and the unused-token sweep;
//! - [`sandbox`] — the sandbox agent token (`oxy_sbx_`): what a mint asks
//!   for, and the writes behind it;
//! - [`standing`] — the personal tokens that carry `platform` or `partner`
//!   standing: the staff reads behind `/api/admin/standing-tokens`;
//! - [`service`] — service tokens (`oxysvc_`), configured as code in
//!   `OXY_SERVICE_TOKENS`: a platform principal with no account to create.
//!
//! # SUNSET after 0.5.161: `api_tokens.legacy_api_key_id`
//!
//! Legacy API keys stopped authenticating in 0.5.160, but the column they are
//! stamped with is still **read** across this module — [`admission`] (refuse
//! the row), [`credential`], [`store`], [`policy`] (never judge a legacy row),
//! [`leak`], [`hygiene`], [`personal`] and [`standing`] (keep those rows out of
//! their lists). The `api_keys` table is still read by nothing but the
//! inventory.
//!
//! Dropping either before 0.5.162 breaks a revert at runtime: a binary one
//! release back needs the column to be there, which is
//! `internal-docs/legacy-removal.md` §4 and product-context's "a revert
//! tolerates newer ledger rows, not newer schema". The four keys still live on
//! 2026-11-16 are also deliberately left active until their owners migrate
//! (`internal-docs/api-token-migration.md`), so ending those is the step before
//! any drop, not after it.

pub mod access;
pub mod account_access;
pub mod admission;
pub mod agent;
pub mod browser_session;
pub mod cache;
pub mod ci;
pub mod ci_mint;
pub mod cli_login;
pub mod credential;
mod device_approver;
mod device_ask;
pub mod device_login;
pub mod dispatch;
pub mod exchange;
pub mod format;
pub mod grant_plan;
mod grant_row;
pub mod hygiene;
pub mod leak;
pub mod mobile_login;
pub mod org_grants;
pub mod personal;
pub mod policy;
pub mod policy_store;
pub mod sandbox;
pub mod sandbox_admission;
pub mod sandbox_recheck;
pub mod service;
pub mod service_account;
pub mod standing;
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
