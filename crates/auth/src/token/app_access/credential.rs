//! The credential a host op of an app decides with, built from the app's
//! live rows (`super`). The pure half ([`credential_of`],
//! [`effective_grants`], [`approved_targets`]) is tested without a database.

use entity::prelude::AppPlatformGrants;
use entity::{api_tokens, app_platform_grants, apps, service_accounts};
use oxy_authz::{Action, AppActionGrant, AppReach, RoleCeiling, TokenGrant};
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

use super::{account_of, db_err, live_token};
use crate::token::account_access::AccountRole;
use crate::token::credential::{AccountStanding, CredentialContext, StoredKind};
use crate::token::service_account;

/// The approved actions `grants` hold for `app`. A row naming an action that is
/// not an app action grants nothing, nor does one for another app or org.
pub fn effective_grants(
    app: &apps::Model,
    grants: &[app_platform_grants::Model],
) -> Vec<AppActionGrant> {
    let mut out: Vec<AppActionGrant> = Vec::new();
    for g in grants
        .iter()
        .filter(|g| g.revoked_at.is_none() && g.app_id == app.id && g.org_id == app.org_id)
    {
        let Some(action) = Action::from_app_action_id(&g.action) else {
            continue;
        };
        let grant = AppActionGrant {
            action,
            org_id: app.org_id,
        };
        if !out.contains(&grant) {
            out.push(grant);
        }
    }
    out
}

/// The targets `grants` approve `action` on for `app`: the pipelines an upload
/// may name. Empty for an action approved on the whole org.
pub fn approved_targets(
    app: &apps::Model,
    grants: &[app_platform_grants::Model],
    action: Action,
) -> std::collections::BTreeSet<String> {
    let Some(id) = action.app_action_id() else {
        return Default::default();
    };
    grants
        .iter()
        .filter(|g| g.revoked_at.is_none() && g.app_id == app.id && g.org_id == app.org_id)
        .filter(|g| g.action == id)
        .filter_map(|g| g.target.clone())
        .collect()
}

/// The credential a host op of `app` decides with, read from the live rows:
/// `None` when the app has no live approval — no account, a disabled one, one
/// in another org, or no live row. The caller refuses then.
pub async fn credential_for<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
) -> Result<Option<CredentialContext>, OxyError> {
    let Some(account) = account_of(db, app.id).await? else {
        return Ok(None);
    };
    if account.disabled_at.is_some() || account.org_id != app.org_id {
        return Ok(None);
    }
    let Some(role) = service_account::role_of(&account) else {
        return Ok(None);
    };
    let Some(token) = live_token(db, account.user_id).await? else {
        return Ok(None);
    };
    let rows = AppPlatformGrants::find()
        .filter(app_platform_grants::Column::TokenId.eq(token.id))
        .filter(app_platform_grants::Column::RevokedAt.is_null())
        .all(db)
        .await
        .map_err(db_err("app platform grants"))?;
    Ok(Some(credential_of(app, &account, role, &token, &rows)))
}

/// [`credential_for`]'s answer from rows already read. Pure, so the shape of an
/// app's credential is tested without a database.
pub fn credential_of(
    app: &apps::Model,
    account: &service_accounts::Model,
    role: AccountRole,
    token: &api_tokens::Model,
    rows: &[app_platform_grants::Model],
) -> CredentialContext {
    let approved = effective_grants(app, rows);
    // One org-wide grant at Admin, when anything is approved: what lets the
    // account's standing reach the ring at all. The actions on the reach are
    // what confine it.
    let grants = if approved.is_empty() {
        Vec::new()
    } else {
        vec![TokenGrant {
            org_id: app.org_id,
            workspace_id: None,
            ceiling: RoleCeiling::Admin,
        }]
    };
    CredentialContext {
        token_id: token.id,
        kind: StoredKind::App,
        principal_user_id: account.user_id,
        all_access: false,
        platform: false,
        partner: false,
        name: token.name.clone(),
        display_prefix: token.display_prefix.clone(),
        grants,
        app_publish: Vec::new(),
        app_sandbox: Vec::new(),
        blocked_orgs: Vec::new(),
        service_account: Some(AccountStanding {
            org_id: account.org_id,
            admin: role == AccountRole::Admin,
        }),
        expires_at: None,
        app_access: Some(AppReach {
            app_id: app.id,
            grants: approved,
        }),
    }
}
