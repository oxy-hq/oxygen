//! `GET` and `DELETE /api/auth/token` — the calling token, about itself.
//!
//! The one pair of token routes a token may call: it describes, and can end,
//! only the credential that calls it. That is what `oxyc whoami` and
//! `oxyc logout` need, and it hands a leaked token nothing but a way to kill
//! itself.
//!
//! - A browser session has no calling token: both answer 404 (`no_token`).
//! - A legacy credential — an `oxy_<hex>` key, or a token the legacy endpoint
//!   minted — can be described but not revoked here (409 `legacy_immutable`).
//!   It ends only by its owner's hand, in a session, or by its own expiry
//!   (design §3.5): the same key is often deployed in several places, and one
//!   of them logging out must not take the others down.

use axum::Json;
use axum::http::StatusCode;
use entity::api_tokens;
use entity::prelude::ApiTokens;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::CredentialContext;
use sea_orm::{DatabaseConnection, EntityTrait};

use super::dto::TokenDto;
use super::error::TokenError;
use super::{service, view};

/// The credential the request authenticated with, or 404 `no_token`.
fn calling(actor: &RequestActor) -> Result<&CredentialContext, TokenError> {
    actor.credential.as_ref().ok_or(TokenError::NoToken)
}

/// Its row. Only ever the caller's own: the id comes from the credential the
/// request authenticated with, and the principal is checked all the same.
async fn calling_row(
    db: &DatabaseConnection,
    actor: &RequestActor,
    credential: &CredentialContext,
) -> Result<api_tokens::Model, TokenError> {
    ApiTokens::find_by_id(credential.token_id)
        .one(db)
        .await?
        .filter(|row| row.principal_user_id == actor.id)
        .ok_or(TokenError::NoToken)
}

/// May this credential revoke itself here? A legacy one may not.
fn may_revoke_itself(credential: &CredentialContext) -> Result<(), TokenError> {
    if credential.is_legacy() {
        return Err(TokenError::LegacyImmutable);
    }
    Ok(())
}

/// Describe the token this request authenticated with
pub async fn get_calling_token(actor: RequestActor) -> Result<Json<TokenDto>, TokenError> {
    let credential = calling(&actor)?;
    let db = establish_connection().await?;
    let row = calling_row(&db, &actor, credential).await?;
    Ok(Json(view::token(&db, &row, actor.label()).await?))
}

/// Revoke the token this request authenticated with
pub async fn revoke_calling_token(actor: RequestActor) -> Result<StatusCode, TokenError> {
    let credential = calling(&actor)?;
    may_revoke_itself(credential)?;
    let db = establish_connection().await?;
    let row = calling_row(&db, &actor, credential).await?;
    service::revoke(&db, &actor, row, "self").await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_auth::token::StoredKind;
    use oxy_auth::types::AuthenticatedUser;
    use uuid::Uuid;

    fn credential(kind: StoredKind, legacy_api_key_id: Option<Uuid>) -> CredentialContext {
        CredentialContext {
            token_id: Uuid::from_u128(9),
            kind,
            principal_user_id: Uuid::from_u128(1),
            all_access: true,
            platform: true,
            partner: true,
            name: "t".into(),
            display_prefix: "oxy_".into(),
            legacy_api_key_id,
            blocked_orgs: Vec::new(),
            expires_at: None,
            service_account: None,
            grants: Vec::new(),
            app_publish: Vec::new(),
        }
    }

    fn actor(credential: Option<CredentialContext>) -> RequestActor {
        let mut actor = RequestActor::session(AuthenticatedUser {
            id: Uuid::from_u128(1),
            email: Some("ada@example.com".into()),
            name: "Ada".into(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        });
        actor.credential = credential;
        actor
    }

    #[test]
    fn a_session_has_no_calling_token() {
        assert!(matches!(calling(&actor(None)), Err(TokenError::NoToken)));
        let with_token = actor(Some(credential(StoredKind::Personal, None)));
        assert_eq!(
            calling(&with_token).map(|c| c.token_id).ok(),
            Some(Uuid::from_u128(9))
        );
    }

    #[test]
    fn a_legacy_credential_cannot_revoke_itself_and_a_personal_token_can() {
        let key = Some(Uuid::from_u128(9));
        for legacy in [
            credential(StoredKind::LegacyKey, key),
            // Minted by the legacy endpoint: new format, legacy reach.
            credential(StoredKind::Personal, key),
        ] {
            assert!(matches!(
                may_revoke_itself(&legacy),
                Err(TokenError::LegacyImmutable)
            ));
        }
        assert!(may_revoke_itself(&credential(StoredKind::Personal, None)).is_ok());
    }
}
