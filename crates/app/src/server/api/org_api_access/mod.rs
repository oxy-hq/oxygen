//! Organization → **API access** over HTTP (API-tokens design §3.3, §5, §8
//! Phase 3): the org's service accounts, their `oxy_sat_` tokens, and the
//! inventory of every token that reaches the org.
//!
//! - `/api/orgs/{org_id}/service-accounts` — create, read, edit, disable and
//!   delete an account ([`accounts`]);
//! - `…/service-accounts/{sa_id}/tokens` — mint, extend, regenerate, revoke,
//!   activity ([`account_tokens`]);
//! - `/api/orgs/{org_id}/tokens` — the inventory and one token's activity in
//!   this org ([`inventory`]);
//! - `…/tokens/{id}/revoke-grant` — end a personal token's reach into this
//!   org ([`revoke_grant`]);
//! - `/api/orgs/{org_id}/token-policy` — what the org asks of the tokens that
//!   reach it ([`token_policy`]).
//!
//! **Who may call them.** An org owner or admin, on four named actions of the
//! existing org-admin ring — taken as [`OrgAdminFor`] extractors, never a
//! hand-written role match. Anyone else gets 403.
//!
//! **Every mutation is session-only** ([`ManageApiAccess`]): a token cannot
//! mint, extend or revoke a token, and a service account cannot manage service
//! accounts whatever its standing. Reads are open to a token that reaches the
//! org's admin routes.
//!
//! **A legacy key is listed and never touched** (§3.5). The org cannot end its
//! reach: revoke-grant answers 409 `legacy_immutable`. Only its owner can.

mod account_tokens;
mod accounts;
mod audit;
mod dto;
pub mod handlers;
mod inventory;
pub mod repo_resolve;
mod revoke_grant;
mod token_policy;
pub(crate) mod trust_policies;

use oxy_auth::extractor::{SESSION_REQUIRED, SessionAction};

use crate::server::api::middlewares::role_guards::{OrgAdminAction, OrgAdminFor};
use crate::server::authz::Action;

/// Create, edit, disable or delete a service account, and manage its tokens.
pub struct ServiceAccounts;

impl OrgAdminAction for ServiceAccounts {
    const LABEL: &'static str = "guard.service_account_manage";
    const ACTION: Action = Action::ServiceAccountManage;
}

/// Read the org token inventory and a token's activity in the org.
pub struct TokenInventory;

impl OrgAdminAction for TokenInventory {
    const LABEL: &'static str = "guard.token_inventory_view";
    const ACTION: Action = Action::TokenInventoryView;
}

/// End a personal token's reach into the org.
pub struct TokenGrants;

impl OrgAdminAction for TokenGrants {
    const LABEL: &'static str = "guard.token_grant_revoke";
    const ACTION: Action = Action::TokenGrantRevoke;
}

/// Read or set the org's token policy.
pub struct TokenPolicy;

impl OrgAdminAction for TokenPolicy {
    const LABEL: &'static str = "guard.token_policy_manage";
    const ACTION: Action = Action::TokenPolicyManage;
}

/// The four doors, as handlers name them.
pub type ManagesServiceAccounts = OrgAdminFor<ServiceAccounts>;
pub type ViewsTokenInventory = OrgAdminFor<TokenInventory>;
pub type RevokesTokenGrants = OrgAdminFor<TokenGrants>;
pub type ManagesTokenPolicy = OrgAdminFor<TokenPolicy>;

/// The session-only gate on every mutation here, with the contract's 403 body.
pub struct ManageApiAccess;

impl SessionAction for ManageApiAccess {
    const REFUSAL: &'static str = "managing API access requires a browser session";
    const CODE: Option<&'static str> = Some(SESSION_REQUIRED);
}
