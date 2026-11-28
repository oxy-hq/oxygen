//! The enforcement driver port: what a domain asks of authorization.
//!
//! `oxy-server-authz` implements it (`oxy_server_authz::enforcer`), and the
//! host holds the one implementation on `AppState` as an `Arc<dyn
//! AuthzEnforcer>`. A domain that enforces through this trait needs neither
//! `oxy-server-authz` nor `oxy-auth`.
//!
//! **The port adds no decision.** Each method is one existing
//! `oxy-server-authz` function, under its own name, so a call site reads the
//! same through the port as it did directly:
//!
//! | Method | Delegates to |
//! | --- | --- |
//! | [`AuthzEnforcer::enforce_for`] | `oxy_server_authz::enforce_for` |
//! | [`AuthzEnforcer::load_principal_facts`] | `loader::load_principal_facts` |
//! | [`AuthzEnforcer::load_principal_facts_scoped`] | `loader::load_principal_facts_scoped` |
//! | [`AuthzEnforcer::platform_reaches`] | `globals::platform_reaches` |
//! | [`AuthzEnforcer::platform_reaches_checked`] | `globals::platform_reaches_checked` |
//! | [`AuthzEnforcer::platform_standing`] | `globals::platform_standing` |
//! | [`AuthzEnforcer::platform_standing_checked`] | `globals::platform_standing_checked` |
//! | [`AuthzEnforcer::invalidate_admin_cache`] | `globals::invalidate_admin_cache` |
//! | [`AuthzEnforcer::partner_grants_app_access`] | `partner_authz::partner_grants_app_access` |
//! | [`AuthzEnforcer::may_act_as`] | `assume_authority::may_act_as` |
//! | [`AuthzEnforcer::is_session_live`] | `assume_liveness::is_session_live` |
//! | [`AuthzEnforcer::live_assumed_org_ids`] | `assume_liveness::live_assumed_org_ids` |
//!
//! So the rule holds through the port: a decision is `existing_allow &&
//! allows(..)`. [`AuthzEnforcer::enforce_for`] is that conjunction; the facts
//! reads hand back facts for the caller's own `oxy_authz::enforce` or
//! `oxy_authz::allows`; the platform and partner reads are the shipped checks
//! a caller passes as `existing_allow`. No method answers "allowed" from
//! anything but those.
//!
//! **Unknown is not "no".** The facts reads answer `None` when the facts could
//! not be read, and the `_checked` reads answer `Err`, exactly as the functions
//! they delegate to; each caller still decides what unknown costs it.

use async_trait::async_trait;
use oxy_authz::{Action, Cap, PrincipalFacts, Resource};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::assume::ActingAs;
use crate::caller::Caller;
use crate::error::AuthzError;
use crate::platform::{Checked, PlatformFlags};

/// One ring to enforce: the arguments `oxy_server_authz::enforce_for` takes
/// after the database and the caller, grouped.
#[derive(Clone, Debug)]
pub struct Enforce<'a> {
    /// The call site's name in the `authz` tracing output.
    pub label: &'a str,
    pub action: Action,
    pub resource: Resource,
    /// The shipped check. The decision is `existing_allow && allows(..)`, so the
    /// model can only subtract from it.
    pub existing_allow: bool,
}

/// The authorization enforcement driver port. See the module doc for what each
/// method delegates to.
#[async_trait]
pub trait AuthzEnforcer: Send + Sync {
    /// `existing_allow && allows(..)` over the caller's facts, capped by the
    /// token the request arrived with. Unknown facts defer to `existing_allow`
    /// for a session and deny for a narrowing token.
    async fn enforce_for(&self, db: &DatabaseConnection, caller: &Caller, ask: Enforce<'_>)
    -> bool;

    /// The caller's facts, or `None` when they could not be read.
    async fn load_principal_facts(
        &self,
        db: &DatabaseConnection,
        caller: &Caller,
    ) -> Option<PrincipalFacts>;

    /// As [`Self::load_principal_facts`]; `include_workspace_facts = false`
    /// skips the `workspace_members` read only the WorkspaceAdmin ring needs.
    async fn load_principal_facts_scoped(
        &self,
        db: &DatabaseConnection,
        caller: &Caller,
        include_workspace_facts: bool,
    ) -> Option<PrincipalFacts>;

    /// Does the caller's platform grant, as the credential carries it, give
    /// `cap` over `org_id`? An unreadable grant answers `false`.
    async fn platform_reaches(
        &self,
        db: &DatabaseConnection,
        caller: &Caller,
        cap: Cap,
        org_id: Uuid,
    ) -> bool;

    /// [`Self::platform_reaches`], telling "does not reach" from "could not
    /// find out" (`Err`).
    async fn platform_reaches_checked(
        &self,
        db: &DatabaseConnection,
        caller: &Caller,
        cap: Cap,
        org_id: Uuid,
    ) -> Result<bool, AuthzError>;

    /// The caller's platform standing **as flags to display**. Decides nothing.
    async fn platform_standing(&self, db: &DatabaseConnection, caller: &Caller) -> PlatformFlags;

    /// The flags and the grant behind them, or `None` when the grant lookup
    /// failed.
    async fn platform_standing_checked(
        &self,
        db: &DatabaseConnection,
        caller: &Caller,
    ) -> Option<Checked>;

    /// Drop the cached platform grants, after a write to them.
    fn invalidate_admin_cache(&self);

    /// Does a partner that manages `org_id` give the caller its data plane
    /// (`develop_apps` over that client)? Fails closed.
    async fn partner_grants_app_access(
        &self,
        db: &DatabaseConnection,
        caller: &Caller,
        org_id: Uuid,
    ) -> bool;

    /// **Who may act as an org**: staff whose grant reaches it with
    /// `ManageOrgSettings`, or a partner assigned the client with
    /// `develop_apps`. Independent of any session; ask
    /// [`Self::is_session_live`] for that, and only when this is `Some`.
    async fn may_act_as(
        &self,
        db: &DatabaseConnection,
        caller: &Caller,
        org_id: Uuid,
    ) -> Option<ActingAs>;

    /// Whether the caller's credential holds a live assume-role session for
    /// `org_id`. Fails closed.
    async fn is_session_live(&self, db: &DatabaseConnection, caller: &Caller, org_id: Uuid)
    -> bool;

    /// Every org the caller's credential holds a live assume-role session for.
    /// Fails closed (empty).
    async fn live_assumed_org_ids(&self, db: &DatabaseConnection, caller: &Caller) -> Vec<Uuid>;
}
