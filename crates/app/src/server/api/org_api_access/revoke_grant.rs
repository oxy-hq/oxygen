//! `POST /api/orgs/{org_id}/tokens/{id}/revoke-grant` — the org ends a
//! personal token's reach into itself (API-tokens design §5).
//!
//! The token is not revoked and nothing it reaches elsewhere changes; the
//! block is one org's (`oxy_auth::token::org_grants` says how it is stored,
//! and why it holds for an all-access token too). The owner is told by email,
//! and sees the grant marked revoked in their own token list.
//!
//! Two kinds are refused, by contract:
//!
//! - a **legacy key** — 409 `legacy_immutable`. Nothing but its owner ends a
//!   legacy key (§3.5), and this route must never be a way around that;
//! - a **service-account** (or trusted-access) token — 409
//!   `use_service_account_routes`. It is the org's own: revoke it, or disable
//!   its account.

use entity::prelude::Users;
use entity::{api_tokens, organizations};
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::StoredKind;
use oxy_auth::token::org_grants::{self, OrgRevoke};
use sea_orm::{DatabaseConnection, EntityTrait, TransactionTrait};
use serde_json::json;
use uuid::Uuid;

use super::inventory;
use crate::emails::token_grant_revoked::{GrantRevokedEmail, send_grant_revoked_email};
use crate::server::api::user_tokens::audit::{self as token_audit, Event};
use crate::server::api::user_tokens::dto;
use crate::server::api::user_tokens::error::TokenError;

/// Whether the org may end this token's reach, or the refusal the contract
/// names. Legacy is checked first: a row that mirrors `api_keys` is a legacy
/// key whatever it is stored as.
pub(super) fn revocable(row: &api_tokens::Model) -> Result<(), TokenError> {
    if dto::is_legacy(row) {
        return Err(TokenError::LegacyImmutable);
    }
    if row.kind != StoredKind::Personal.as_str() {
        return Err(TokenError::UseServiceAccountRoutes);
    }
    Ok(())
}

async fn record(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org_id: Uuid,
    row: &api_tokens::Model,
) -> Result<OrgRevoke, TokenError> {
    let txn = db.begin().await?;
    let done = org_grants::revoke_org_reach(&txn, row, org_id, actor.id).await?;
    if done.changed() {
        Event {
            action: token_audit::GRANT_REVOKED_BY_ORG,
            token: row,
            orgs: vec![org_id],
            detail: json!({
                "all_access": row.all_access,
                "revoked_grants": done.revoked,
                "owner_user_id": row.principal_user_id,
            }),
            change: None,
        }
        .record(&txn, actor)
        .await?;
    }
    txn.commit().await?;
    Ok(done)
}

/// Tell the owner. Best-effort: the reach is already ended and audited, and a
/// mail that cannot be sent must not turn that into an error.
async fn notify_owner(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org: &organizations::Model,
    row: &api_tokens::Model,
) {
    let owner = match Users::find_by_id(row.principal_user_id).one(db).await {
        Ok(owner) => owner,
        Err(e) => {
            tracing::warn!(token_id = %row.id, error = %e, "could not look up a token's owner to notify");
            return;
        }
    };
    let Some(to_email) = owner.and_then(|u| u.email) else {
        return;
    };
    let mail = GrantRevokedEmail {
        to_email: &to_email,
        org_name: &org.name,
        token_name: &row.name,
        display_prefix: &row.display_prefix,
        revoked_by: actor.label(),
    };
    if let Err(e) = send_grant_revoked_email(mail).await {
        tracing::warn!(token_id = %row.id, error = %e, "could not email a token's owner about a revoked grant");
    }
}

/// End `token_id`'s reach into `org`. Idempotent: a second call changes
/// nothing, records nothing and sends nothing.
pub(super) async fn revoke(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org: &organizations::Model,
    token_id: Uuid,
) -> Result<(), TokenError> {
    let row = inventory::find(db, org.id, token_id).await?;
    revocable(&row)?;
    let done = record(db, actor, org.id, &row).await?;
    oxy_auth::token::cache::invalidate_token(row.id);
    if done.changed() {
        notify_owner(db, actor, org, &row).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn token(kind: &str, legacy_api_key_id: Option<Uuid>) -> api_tokens::Model {
        api_tokens::Model {
            id: Uuid::new_v4(),
            kind: kind.to_string(),
            principal_user_id: Uuid::from_u128(1),
            name: "t".into(),
            display_prefix: "oxy_".into(),
            last_four: String::new(),
            token_hash: vec![0; 32],
            all_access: true,
            platform: true,
            partner: true,
            expires_at: None,
            last_used_at: None,
            created_at: Utc::now().fixed_offset(),
            created_by: None,
            revoked_at: None,
            revoked_by: None,
            revoke_reason: None,
            source: "ui".into(),
            legacy_api_key_id,
            trust_policy_id: None,
            oidc_claims: None,
        }
    }

    #[test]
    fn only_a_personal_token_can_have_its_reach_ended_by_the_org() {
        assert!(revocable(&token("personal", None)).is_ok());
        // A legacy key in either of its shapes: never.
        assert!(matches!(
            revocable(&token("legacy_key", None)),
            Err(TokenError::LegacyImmutable)
        ));
        assert!(matches!(
            revocable(&token("personal", Some(Uuid::new_v4()))),
            Err(TokenError::LegacyImmutable)
        ));
        // The org's own tokens are managed from their account.
        for kind in ["service_account", "ci"] {
            assert!(matches!(
                revocable(&token(kind, None)),
                Err(TokenError::UseServiceAccountRoutes)
            ));
        }
    }
}
