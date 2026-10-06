//! The org's API access routes. Each handler names its door first — an
//! [`OrgAdminFor`](crate::server::api::middlewares::role_guards::OrgAdminFor)
//! extractor, so anyone but an org owner or admin gets 403 before anything is
//! parsed or read — and every mutation then takes [`SessionOnly`], so a key or
//! token gets 403 `session_required`.
//!
//! Bodies are read as bytes so a malformed one answers the contract's
//! `{ "error" }`, and an id that is not a UUID names nothing: 404.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::ExtendTo;
use oxy_auth::extractor::SessionOnly;
use oxy_auth::token::account_access::{
    CreateAccountBody, CreateAccountTokenBody, PatchAccountBody,
};
use oxy_auth::token::trust_policy_access::{CreatePolicyBody, PatchPolicyBody};
use uuid::Uuid;

use super::dto::{
    OrgTokenList, ServiceAccountDto, ServiceAccountList, TrustPolicyDto, TrustPolicyList,
};
use super::inventory::InventoryQuery;
use super::token_policy::{self, TokenPolicyDto};
use super::{
    ManageApiAccess, ManagesServiceAccounts, ManagesTokenPolicy, RevokesTokenGrants,
    ViewsTokenInventory, account_tokens, accounts, inventory, revoke_grant, trust_policies,
};
use crate::server::api::api_keys::activity::{ActivityQuery, ActivityResponse, page_size};
use crate::server::api::middlewares::role_guards::OrgAdminFor;
use crate::server::api::user_tokens::dto::TokenDto;
use crate::server::api::user_tokens::error::TokenError;
use crate::server::api::user_tokens::handlers::{TokenList, TokenWithSecret, parse, token_id};

type Session = SessionOnly<ManageApiAccess>;

impl From<account_tokens::Minted> for TokenWithSecret {
    fn from(minted: account_tokens::Minted) -> Self {
        Self {
            token: minted.token,
            secret: minted.secret,
        }
    }
}

/// List the organization's service accounts
pub async fn list_service_accounts(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
) -> Result<Json<ServiceAccountList>, TokenError> {
    let db = establish_connection().await?;
    let service_accounts = accounts::list(&db, ctx.org.id).await?;
    Ok(Json(ServiceAccountList { service_accounts }))
}

/// Create a service account
pub async fn create_service_account(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    body: Bytes,
) -> Result<(StatusCode, Json<ServiceAccountDto>), TokenError> {
    let body: CreateAccountBody = parse(&body)?;
    let db = establish_connection().await?;
    let account = accounts::create(&db, &actor, ctx.org.id, body).await?;
    Ok((StatusCode::CREATED, Json(account)))
}

/// Read one service account
pub async fn get_service_account(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    Path((_org_id, sa_id)): Path<(Uuid, String)>,
) -> Result<Json<ServiceAccountDto>, TokenError> {
    let sa_id = token_id(&sa_id)?;
    let db = establish_connection().await?;
    Ok(Json(accounts::get(&db, ctx.org.id, sa_id).await?))
}

/// Edit a service account's description or role, or disable it
pub async fn update_service_account(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id)): Path<(Uuid, String)>,
    body: Bytes,
) -> Result<Json<ServiceAccountDto>, TokenError> {
    let sa_id = token_id(&sa_id)?;
    let body: PatchAccountBody = parse(&body)?;
    let db = establish_connection().await?;
    let account = accounts::patch(&db, &actor, ctx.org.id, sa_id, body).await?;
    Ok(Json(account))
}

/// Delete a service account; its tokens are revoked with it
pub async fn delete_service_account(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id)): Path<(Uuid, String)>,
) -> Result<StatusCode, TokenError> {
    let sa_id = token_id(&sa_id)?;
    let db = establish_connection().await?;
    accounts::delete(&db, &actor, ctx.org.id, sa_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// List a service account's tokens
pub async fn list_account_tokens(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    Path((_org_id, sa_id)): Path<(Uuid, String)>,
) -> Result<Json<TokenList>, TokenError> {
    let sa_id = token_id(&sa_id)?;
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    let tokens = account_tokens::list(&db, &account).await?;
    Ok(Json(TokenList { tokens }))
}

/// Create a service-account token; the secret is returned once
pub async fn create_account_token(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id)): Path<(Uuid, String)>,
    body: Bytes,
) -> Result<(StatusCode, Json<TokenWithSecret>), TokenError> {
    let sa_id = token_id(&sa_id)?;
    let body: CreateAccountTokenBody = parse(&body)?;
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    let minted = account_tokens::create(&db, &actor, &account, body).await?;
    Ok((StatusCode::CREATED, Json(minted.into())))
}

/// Push out a service-account token's expiry
pub async fn extend_account_token(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id, id)): Path<(Uuid, String, String)>,
    body: Bytes,
) -> Result<Json<TokenDto>, TokenError> {
    let (sa_id, id) = (token_id(&sa_id)?, token_id(&id)?);
    let target =
        ExtendTo::from_json(&parse::<serde_json::Value>(&body)?).map_err(TokenError::Invalid)?;
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    let token = account_tokens::extend(&db, &actor, &account, id, target).await?;
    Ok(Json(token))
}

/// Give a service-account token a new secret; the old one stops working
pub async fn regenerate_account_token(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id, id)): Path<(Uuid, String, String)>,
) -> Result<Json<TokenWithSecret>, TokenError> {
    let (sa_id, id) = (token_id(&sa_id)?, token_id(&id)?);
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    let minted = account_tokens::regenerate(&db, &actor, &account, id).await?;
    Ok(Json(minted.into()))
}

/// Revoke a service-account token
pub async fn revoke_account_token(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id, id)): Path<(Uuid, String, String)>,
) -> Result<StatusCode, TokenError> {
    let (sa_id, id) = (token_id(&sa_id)?, token_id(&id)?);
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    account_tokens::revoke(&db, &actor, &account, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// What happened to a service-account token, and what was done with it
pub async fn get_account_token_activity(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    Path((_org_id, sa_id, id)): Path<(Uuid, String, String)>,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<ActivityResponse>, TokenError> {
    let (sa_id, id) = (token_id(&sa_id)?, token_id(&id)?);
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    let limit = page_size(query.limit);
    Ok(Json(
        account_tokens::activity(&db, &account, id, limit).await?,
    ))
}

/// List every API token that reaches the organization
pub async fn list_org_tokens(
    OrgAdminFor(ctx, _): ViewsTokenInventory,
    Query(query): Query<InventoryQuery>,
) -> Result<Json<OrgTokenList>, TokenError> {
    let db = establish_connection().await?;
    let tokens = inventory::list(&db, ctx.org.id, &query).await?;
    Ok(Json(OrgTokenList { tokens }))
}

/// What a token did in this organization
pub async fn get_org_token_activity(
    OrgAdminFor(ctx, _): ViewsTokenInventory,
    Path((_org_id, id)): Path<(Uuid, String)>,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<ActivityResponse>, TokenError> {
    let id = token_id(&id)?;
    let db = establish_connection().await?;
    let limit = page_size(query.limit);
    Ok(Json(inventory::activity(&db, ctx.org.id, id, limit).await?))
}

/// End a personal token's reach into this organization
pub async fn revoke_org_grant(
    OrgAdminFor(ctx, _): RevokesTokenGrants,
    _: Session,
    actor: RequestActor,
    Path((_org_id, id)): Path<(Uuid, String)>,
) -> Result<StatusCode, TokenError> {
    let id = token_id(&id)?;
    let db = establish_connection().await?;
    revoke_grant::revoke(&db, &actor, &ctx.org, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Trust policies ───────────────────────────────────────────────────────────
//
// Managing an account's trust policies is managing the account: the same
// door (`service_account_manage`), and every mutation session-only.

/// List a service account's trust policies
pub async fn list_trust_policies(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    Path((_org_id, sa_id)): Path<(Uuid, String)>,
) -> Result<Json<TrustPolicyList>, TokenError> {
    let sa_id = token_id(&sa_id)?;
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    let trust_policies = trust_policies::list(&db, &account).await?;
    Ok(Json(TrustPolicyList { trust_policies }))
}

/// Register a trust policy: which GitHub Actions runs may act as the account
pub async fn create_trust_policy(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id)): Path<(Uuid, String)>,
    body: Bytes,
) -> Result<(StatusCode, Json<TrustPolicyDto>), TokenError> {
    let sa_id = token_id(&sa_id)?;
    let body: CreatePolicyBody = parse(&body)?;
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    let policy = trust_policies::create(&db, &actor, &account, body).await?;
    Ok((StatusCode::CREATED, Json(policy)))
}

/// Edit a trust policy; `grants` replaces the set
pub async fn update_trust_policy(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id, id)): Path<(Uuid, String, String)>,
    body: Bytes,
) -> Result<Json<TrustPolicyDto>, TokenError> {
    let (sa_id, id) = (token_id(&sa_id)?, token_id(&id)?);
    let body: PatchPolicyBody = parse(&body)?;
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    let policy = trust_policies::patch(&db, &actor, &account, id, body).await?;
    Ok(Json(policy))
}

/// Delete a trust policy, ending the tokens it minted
pub async fn delete_trust_policy(
    OrgAdminFor(ctx, _): ManagesServiceAccounts,
    _: Session,
    actor: RequestActor,
    Path((_org_id, sa_id, id)): Path<(Uuid, String, String)>,
) -> Result<StatusCode, TokenError> {
    let (sa_id, id) = (token_id(&sa_id)?, token_id(&id)?);
    let db = establish_connection().await?;
    let account = accounts::find(&db, ctx.org.id, sa_id).await?;
    trust_policies::delete(&db, &actor, &account, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Read the organization's token policy (the defaults when it set none)
pub async fn get_token_policy(
    OrgAdminFor(ctx, _): ManagesTokenPolicy,
) -> Result<Json<TokenPolicyDto>, TokenError> {
    let db = establish_connection().await?;
    Ok(Json(token_policy::get(&db, ctx.org.id).await?))
}

/// Replace the organization's token policy
pub async fn put_token_policy(
    OrgAdminFor(ctx, _): ManagesTokenPolicy,
    _: Session,
    actor: RequestActor,
    body: Bytes,
) -> Result<Json<TokenPolicyDto>, TokenError> {
    let body: TokenPolicyDto = parse(&body)?;
    let db = establish_connection().await?;
    Ok(Json(token_policy::put(&db, &actor, &ctx.org, body).await?))
}
