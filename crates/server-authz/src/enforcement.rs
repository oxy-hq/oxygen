//! This crate's implementation of the enforcement driver port,
//! [`contracts_authz::AuthzEnforcer`].
//!
//! Every method is one call to the function of the same name here, with the
//! port's [`contracts_authz::Caller`] wrapped back into [`Caller`]. The wrap is a
//! clone of the same values, the grant memo's `Arc` included, so a read through
//! the port and a guard of the same request share one uncached grant read, and
//! answer exactly what the direct call answers. Nothing is decided here.
//!
//! The host builds it once ([`enforcer`]) and keeps it on `AppState`
//! (`oxy_app_core::app_state::AppState::authz`).

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::Extensions;
use contracts_authz::{
    ActingAs, AuthzEnforcer, AuthzError, Checked, Enforce, PartnerReach, PlatformFlags,
};
use oxy_authz::{Cap, PrincipalFacts};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::caller::Caller;

/// The enforcement port, answered by this crate's functions.
#[derive(Clone, Copy, Debug, Default)]
pub struct ServerAuthz;

/// The one implementation of the port, for the host to hold on `AppState`, and
/// the one every caller this crate builds carries ([`Caller::of`],
/// [`Caller::without_credential`]). `ServerAuthz` is stateless (its caches are
/// process statics), so every handle is the same enforcer.
pub fn enforcer() -> Arc<dyn AuthzEnforcer> {
    Arc::new(ServerAuthz)
}

/// The wrapper the existing functions take, holding the port caller's values.
fn wrap(caller: &contracts_authz::Caller) -> Caller {
    Caller::from(caller.clone())
}

#[async_trait]
impl AuthzEnforcer for ServerAuthz {
    async fn enforce_for(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
        ask: Enforce<'_>,
    ) -> bool {
        crate::enforce_for(
            db,
            &wrap(caller),
            ask.label,
            ask.action,
            ask.resource,
            ask.existing_allow,
        )
        .await
    }

    async fn load_principal_facts(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
    ) -> Option<PrincipalFacts> {
        crate::loader::load_principal_facts(db, &wrap(caller)).await
    }

    async fn load_principal_facts_scoped(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
        include_workspace_facts: bool,
    ) -> Option<PrincipalFacts> {
        crate::loader::load_principal_facts_scoped(db, &wrap(caller), include_workspace_facts).await
    }

    async fn platform_reaches(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
        cap: Cap,
        org_id: Uuid,
    ) -> bool {
        crate::globals::platform_reaches(db, &wrap(caller), cap, org_id).await
    }

    async fn platform_reaches_checked(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
        cap: Cap,
        org_id: Uuid,
    ) -> Result<bool, AuthzError> {
        Ok(crate::globals::platform_reaches_checked(db, &wrap(caller), cap, org_id).await?)
    }

    async fn platform_standing(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
    ) -> PlatformFlags {
        crate::globals::platform_standing(db, &wrap(caller)).await
    }

    async fn platform_standing_checked(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
    ) -> Option<Checked> {
        crate::globals::platform_standing_checked(db, &wrap(caller)).await
    }

    fn invalidate_admin_cache(&self) {
        crate::globals::invalidate_admin_cache();
    }

    async fn partner_grants_app_access(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
        org_id: Uuid,
    ) -> bool {
        crate::partner_authz::partner_grants_app_access(db, &wrap(caller), org_id).await
    }

    async fn partner_reach(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
        org_id: Uuid,
        cap: Cap,
    ) -> Option<PartnerReach> {
        crate::partner_authz::partner_reach(db, &wrap(caller), org_id, cap).await
    }

    async fn may_act_as(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
        org_id: Uuid,
    ) -> Option<ActingAs> {
        crate::assume_authority::may_act_as(db, &wrap(caller), org_id).await
    }

    async fn is_session_live(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
        org_id: Uuid,
    ) -> bool {
        crate::assume_liveness::is_session_live(db, &wrap(caller), org_id).await
    }

    async fn live_assumed_org_ids(
        &self,
        db: &DatabaseConnection,
        caller: &contracts_authz::Caller,
    ) -> Vec<Uuid> {
        crate::assume_liveness::live_assumed_org_ids(db, &wrap(caller)).await
    }

    async fn request_facts(&self, extensions: &mut Extensions) -> Option<PrincipalFacts> {
        crate::request_facts_in(extensions).await
    }

    async fn may_preview(&self, extensions: &mut Extensions) -> bool {
        crate::preview_guard::may_preview_in(extensions).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime")
            .block_on(future)
    }

    /// Every caller the wrapper builds carries this crate's enforcer: asked
    /// through `caller.authz()`, it answers as `request_facts_in` does (from the
    /// memo, so no database). This is what a domain holding a caller relies on,
    /// with no `AppState` and no process global.
    #[test]
    fn a_wrapper_built_caller_carries_server_authz() {
        let user = oxy_auth::types::AuthenticatedUser {
            id: Uuid::from_u128(1),
            email: Some("ada@example.com".into()),
            name: "Ada".into(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        };
        let mut request = Extensions::new();
        request.insert(user.clone());
        let callers = [
            Caller::of(&user, None),
            Caller::from_user(&user),
            Caller::from_extensions(&request).expect("a user in the extensions"),
            Caller::of_request(&request, &user),
            Caller::without_credential(user.id, "ada@example.com"),
        ];
        for caller in callers {
            let mut extensions = Extensions::new();
            extensions.insert(PrincipalFacts {
                member_orgs: vec![Uuid::from_u128(7)],
                ..Default::default()
            });
            let read = block_on(caller.authz().request_facts(&mut extensions));
            assert_eq!(read.map(|f| f.member_orgs), Some(vec![Uuid::from_u128(7)]));
            // The port's caller it converts to keeps the same enforcer.
            let port: contracts_authz::Caller = caller.into();
            assert!(!block_on(port.authz().may_preview(&mut Extensions::new())));
        }
    }

    /// The handle is not part of who a caller is: the same parts compare equal.
    #[test]
    fn callers_equal_by_their_parts_compare_equal_whatever_their_handle() {
        let a = Caller::without_credential(Uuid::from_u128(1), "ada@example.com");
        let b = Caller::from(contracts_authz::Caller::without_credential(
            Uuid::from_u128(1),
            "ada@example.com",
            Arc::new(ServerAuthz),
        ));
        assert_eq!(a, b);
    }

    /// The port's facts read answers from the memo in the request's extensions,
    /// exactly as `request_facts(parts)` does: no caller, no database needed.
    #[test]
    fn the_ports_request_facts_reads_the_memo() {
        let facts = PrincipalFacts {
            member_orgs: vec![Uuid::from_u128(7)],
            ..Default::default()
        };
        let mut extensions = Extensions::new();
        extensions.insert(facts);
        let read = block_on(ServerAuthz.request_facts(&mut extensions));
        assert_eq!(read.map(|f| f.member_orgs), Some(vec![Uuid::from_u128(7)]));
    }

    /// With no workspace in the extensions there is nothing to preview: the port
    /// answers `false` before it reads anything, as the guard does.
    #[test]
    fn the_ports_may_preview_fails_closed_without_a_workspace() {
        let mut extensions = Extensions::new();
        let may = block_on(ServerAuthz.may_preview(&mut extensions));
        assert!(!may);
    }

    /// The port's caller and this crate's are the same values: wrapping one
    /// back keeps every read, and the memo a guard filled is the one a port
    /// read sees.
    #[test]
    fn a_wrapped_port_caller_is_the_caller_it_came_from() {
        let caller = Caller::without_credential(Uuid::from_u128(1), "ada@example.com");
        let grant = oxy_authz::PlatformStanding::from_role(
            oxy_authz::PlatformRole::AppOperator,
            oxy_authz::Scope::All,
        );
        caller.grant_memo().set(Some(grant.clone()));

        let port: contracts_authz::Caller = caller.clone().into();
        let back = wrap(&port);

        assert_eq!(back, caller);
        assert_eq!(back.email(), "ada@example.com");
        assert_eq!(back.grant_memo().get(), Some(Some(grant)));
    }

    /// A credential's expiry survives the plain projection to the nanosecond.
    #[test]
    fn a_token_expiry_round_trips_through_the_projection() {
        use chrono::TimeZone;
        let at = chrono::Utc
            .timestamp_opt(1_900_000_000, 123_456_789)
            .single()
            .expect("a valid instant");
        let user = oxy_auth::types::AuthenticatedUser {
            id: Uuid::from_u128(1),
            email: Some("ada@example.com".into()),
            name: "Ada".into(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        };
        let credential = oxy_auth::token::CredentialContext {
            token_id: Uuid::from_u128(9),
            kind: oxy_auth::token::StoredKind::Personal,
            principal_user_id: Uuid::from_u128(1),
            all_access: true,
            platform: true,
            partner: true,
            name: "t".into(),
            display_prefix: "oxy_pat_Ab3x".into(),
            blocked_orgs: Vec::new(),
            expires_at: Some(at),
            service_account: None,
            grants: Vec::new(),
            app_publish: Vec::new(),
            app_sandbox: Vec::new(),
            app_access: None,
        };
        let caller = Caller::of(&user, Some(&credential));
        assert_eq!(caller.token_expires_at(), Some(at));
        assert_eq!(wrap(caller.as_contract()).token_expires_at(), Some(at));
    }
}
