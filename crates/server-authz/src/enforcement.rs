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
use contracts_authz::{ActingAs, AuthzEnforcer, AuthzError, Checked, Enforce, PlatformFlags};
use oxy_authz::{Cap, PrincipalFacts};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::caller::Caller;

/// The enforcement port, answered by this crate's functions.
#[derive(Clone, Copy, Debug, Default)]
pub struct ServerAuthz;

/// The one implementation of the port, for the host to hold on `AppState`.
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
