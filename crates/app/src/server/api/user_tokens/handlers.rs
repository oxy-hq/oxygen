//! `/api/user/tokens` — the caller's personal tokens and legacy keys.
//!
//! Every handler takes [`SessionOnly`] first: a key or token gets 403
//! `session_required` before anything is parsed or read. Bodies are read as
//! bytes so a malformed one answers the contract's `{ "error" }`, not the
//! framework's rejection.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::ExtendTo;
use oxy_auth::extractor::SessionOnly;
use oxy_auth::token::access::{CreateBody, PatchBody};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::ManageTokens;
use super::dto::TokenDto;
use super::error::TokenError;
use super::service::{self, Minted};
use crate::server::api::api_keys::activity::{ActivityQuery, ActivityResponse, page_size};

#[derive(Serialize)]
pub struct TokenList {
    pub tokens: Vec<TokenDto>,
}

/// A token with the secret that is shown exactly once.
#[derive(Serialize)]
pub struct TokenWithSecret {
    pub token: TokenDto,
    pub secret: String,
}

impl From<Minted> for TokenWithSecret {
    fn from(minted: Minted) -> Self {
        Self {
            token: minted.token,
            secret: minted.secret,
        }
    }
}

pub(crate) fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, TokenError> {
    serde_json::from_slice(body).map_err(|e| TokenError::Invalid(format!("invalid body: {e}")))
}

/// An id that is not a UUID names no token: 404, like any other unknown id.
pub(crate) fn token_id(raw: &str) -> Result<Uuid, TokenError> {
    Uuid::parse_str(raw).map_err(|_| TokenError::NotFound)
}

/// List the caller's personal tokens and legacy keys, newest first
pub async fn list_tokens(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
) -> Result<Json<TokenList>, TokenError> {
    let db = establish_connection().await?;
    let tokens = service::list(&db, &actor).await?;
    Ok(Json(TokenList { tokens }))
}

/// Create a personal access token; the secret is returned once
pub async fn create_token(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    body: Bytes,
) -> Result<(StatusCode, Json<TokenWithSecret>), TokenError> {
    let body: CreateBody = parse(&body)?;
    let db = establish_connection().await?;
    let minted = service::create(&db, &actor, body).await?;
    Ok((StatusCode::CREATED, Json(minted.into())))
}

/// Read one of the caller's tokens
pub async fn get_token(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(id): Path<String>,
) -> Result<Json<TokenDto>, TokenError> {
    let id = token_id(&id)?;
    let db = establish_connection().await?;
    Ok(Json(service::get(&db, &actor, id).await?))
}

/// Rename a token or change what it can reach; `grants` replaces the set
pub async fn update_token(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<TokenDto>, TokenError> {
    let id = token_id(&id)?;
    let body: PatchBody = parse(&body)?;
    let db = establish_connection().await?;
    Ok(Json(service::patch(&db, &actor, id, body).await?))
}

/// Push out a token's expiry without changing the token
pub async fn extend_token(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<TokenDto>, TokenError> {
    let id = token_id(&id)?;
    let target =
        ExtendTo::from_json(&parse::<serde_json::Value>(&body)?).map_err(TokenError::Invalid)?;
    let db = establish_connection().await?;
    Ok(Json(service::extend(&db, &actor, id, target).await?))
}

/// Give a token a new secret; the old one stops working
pub async fn regenerate_token(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(id): Path<String>,
) -> Result<Json<TokenWithSecret>, TokenError> {
    let id = token_id(&id)?;
    let db = establish_connection().await?;
    Ok(Json(service::regenerate(&db, &actor, id).await?.into()))
}

/// Revoke one of the caller's tokens
pub async fn revoke_token(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(id): Path<String>,
) -> Result<StatusCode, TokenError> {
    let id = token_id(&id)?;
    let db = establish_connection().await?;
    let row = service::owned(&db, id, actor.id).await?;
    service::revoke(&db, &actor, row, "owner").await?;
    Ok(StatusCode::NO_CONTENT)
}

/// What happened to one of the caller's tokens, and what was done with it
pub async fn get_token_activity(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(id): Path<String>,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<ActivityResponse>, TokenError> {
    let id = token_id(&id)?;
    let db = establish_connection().await?;
    let limit = page_size(query.limit);
    Ok(Json(service::activity(&db, &actor, id, limit).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_body_or_id_reads_as_the_contract_says() {
        let bad: Result<CreateBody, _> = parse(&Bytes::from_static(b"{"));
        assert!(matches!(bad, Err(TokenError::Invalid(_))));
        // A grant with a malformed id is a bad body, not a 500.
        let bad_grant: Result<CreateBody, _> = parse(&Bytes::from_static(
            br#"{"name":"n","all_access":false,"grants":[{"org_id":"acme"}]}"#,
        ));
        assert!(matches!(bad_grant, Err(TokenError::Invalid(_))));
        assert!(matches!(token_id("not-a-uuid"), Err(TokenError::NotFound)));
        assert!(token_id("00000000-0000-0000-0000-000000000001").is_ok());
        let ok: CreateBody = parse(&Bytes::from_static(br#"{"name":"laptop"}"#)).unwrap();
        assert_eq!(ok.name, "laptop");
    }
}
