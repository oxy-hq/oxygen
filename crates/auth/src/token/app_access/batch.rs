//! The app-access reads of `super`, for many apps at once: a list endpoint
//! must not pay a query per row. Each keeps its single-app reader's filters.

use std::collections::HashMap;

use entity::prelude::{ApiTokens, AppPlatformGrants, AppServiceAccounts, ServiceAccounts};
use entity::{api_tokens, app_platform_grants, app_service_accounts, service_accounts};
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use super::db_err;
use crate::token::credential::StoredKind;

/// [`super::account_of`] for many apps in two queries, for a list. An app with no
/// account is absent from the map.
pub async fn accounts_of<C: ConnectionTrait>(
    db: &C,
    app_ids: &[Uuid],
) -> Result<HashMap<Uuid, service_accounts::Model>, OxyError> {
    let links = AppServiceAccounts::find()
        .filter(app_service_accounts::Column::AppId.is_in(app_ids.iter().copied()))
        .all(db)
        .await
        .map_err(db_err("app service account links"))?;
    let mut accounts: HashMap<Uuid, service_accounts::Model> = ServiceAccounts::find()
        .filter(
            service_accounts::Column::UserId.is_in(links.iter().map(|l| l.service_account_user_id)),
        )
        .all(db)
        .await
        .map_err(db_err("app service accounts"))?
        .into_iter()
        .map(|a| (a.user_id, a))
        .collect();
    Ok(links
        .into_iter()
        .filter_map(|l| {
            accounts
                .remove(&l.service_account_user_id)
                .map(|a| (l.app_id, a))
        })
        .collect())
}

/// [`super::live_token`] for many accounts in one query: each account's newest live
/// row of kind `app`, keyed by the account.
pub async fn live_tokens<C: ConnectionTrait>(
    db: &C,
    account_user_ids: &[Uuid],
) -> Result<HashMap<Uuid, api_tokens::Model>, OxyError> {
    let rows = ApiTokens::find()
        .filter(api_tokens::Column::PrincipalUserId.is_in(account_user_ids.iter().copied()))
        .filter(api_tokens::Column::Kind.eq(StoredKind::App.as_str()))
        .filter(api_tokens::Column::RevokedAt.is_null())
        .order_by_desc(api_tokens::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err("app access tokens"))?;
    let mut out = HashMap::new();
    for row in rows {
        // Newest first, so the first row seen for an account is its live one.
        out.entry(row.principal_user_id).or_insert(row);
    }
    Ok(out)
}

/// [`super::approvals`] for many apps in one query, keyed by app, oldest first.
pub async fn approvals_of<C: ConnectionTrait>(
    db: &C,
    app_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<app_platform_grants::Model>>, OxyError> {
    let rows = AppPlatformGrants::find()
        .filter(app_platform_grants::Column::AppId.is_in(app_ids.iter().copied()))
        .filter(app_platform_grants::Column::RevokedAt.is_null())
        .order_by_asc(app_platform_grants::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err("app platform grants"))?;
    let mut out: HashMap<Uuid, Vec<app_platform_grants::Model>> = HashMap::new();
    for row in rows {
        out.entry(row.app_id).or_default().push(row);
    }
    Ok(out)
}
