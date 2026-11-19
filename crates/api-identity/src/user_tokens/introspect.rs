//! `GET` and `DELETE /api/auth/token` — the calling token, about itself.
//!
//! The one pair of token routes a token may call: it describes, and can end,
//! only the credential that calls it. That is what `oxyc whoami` and
//! `oxyc logout` need, and it hands a leaked token nothing but a way to kill
//! itself.
//!
//! - A browser session has no calling token: both answer 404 (`no_token`).
//! - A **sandbox agent token** is also told who minted it and which apps it
//!   may run the sandbox loop on (`minter`, `apps`): `oxyc` resolves
//!   `<org>/<app>` against that list, because the token reaches no app
//!   directory. Every other credential's answer is unchanged.
//! A legacy credential used to be describable here but not revocable
//! (409 `legacy_immutable`). Legacy keys were removed and no request
//! authenticates as one any more, so nothing reaches these routes as one and
//! the special case is gone with it.

use axum::Json;
use axum::http::StatusCode;
use entity::api_tokens;
use entity::prelude::ApiTokens;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::CredentialContext;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Serialize;

use super::dto::TokenDto;
use super::error::TokenError;
use super::sandbox::{self, GrantedAppDto, MinterDto};
use super::{service, view};

/// The calling token, as it is told about itself: the shared token shape,
/// and — for a sandbox agent token only — its minter and its apps. The two
/// fields are absent, not null, for every other kind.
#[derive(Debug, PartialEq, Serialize)]
pub struct CallingToken {
    #[serde(flatten)]
    pub token: TokenDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minter: Option<MinterDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<GrantedAppDto>>,
}

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

/// Describe the token this request authenticated with
pub async fn get_calling_token(actor: RequestActor) -> Result<Json<CallingToken>, TokenError> {
    let credential = calling(&actor)?;
    let db = establish_connection().await?;
    let row = calling_row(&db, &actor, credential).await?;
    let token = view::token(&db, &row, actor.label()).await?;
    let (minter, apps) = match sandbox::describe(&db, &actor, &row).await? {
        Some(own) => (Some(own.minter), Some(own.apps)),
        None => (None, None),
    };
    Ok(Json(CallingToken {
        token,
        minter,
        apps,
    }))
}

/// Revoke the token this request authenticated with
pub async fn revoke_calling_token(actor: RequestActor) -> Result<StatusCode, TokenError> {
    let credential = calling(&actor)?;
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

    fn credential(kind: StoredKind) -> CredentialContext {
        CredentialContext {
            token_id: Uuid::from_u128(9),
            kind,
            principal_user_id: Uuid::from_u128(1),
            all_access: true,
            platform: true,
            partner: true,
            name: "t".into(),
            display_prefix: "oxy_".into(),
            blocked_orgs: Vec::new(),
            expires_at: None,
            service_account: None,
            grants: Vec::new(),
            app_publish: Vec::new(),
            app_sandbox: Vec::new(),
            app_access: None,
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
        let with_token = actor(Some(credential(StoredKind::Personal)));
        assert_eq!(
            calling(&with_token).map(|c| c.token_id).ok(),
            Some(Uuid::from_u128(9))
        );
    }
}
