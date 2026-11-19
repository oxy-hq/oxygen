//! A custom app's own platform access: the database primitives behind
//! approving it, revoking it, and building the credential a host op decides
//! with (`internal-docs/proposals/app-platform-access.md`).
//!
//! Who may approve or revoke is decided by the handler, which also writes the
//! audit row in the same transaction, as for [`super::service_account`].
//!
//! **Three rows, no secret.** An approved app has:
//!
//! - a service account (`service_accounts`, an admin of the app's org), linked
//!   to the app by `app_service_accounts`. Made on the first approval and kept
//!   afterwards, so every approval of the app acts as the same principal;
//! - one live `api_tokens` row of kind `app` acting as that account. Its hash is
//!   of random bytes thrown away at creation, so nobody holds a secret for it,
//!   and admission refuses the kind besides. It is what the org's API-access
//!   inventory lists, what audit rows name as `token_id`, and what a revoke ends;
//! - one `app_platform_grants` row per approved action under that token.
//!
//! The function host reads all three for every host op ([`credential_for`]),
//! so a revoke, a disabled account or a re-pointed app is seen by the next call.

use chrono::Utc;
use entity::prelude::{ApiTokens, AppPlatformGrants, AppServiceAccounts, ServiceAccounts};
use entity::{api_tokens, app_platform_grants, app_service_accounts, apps, service_accounts};
use oxy_authz::Action;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use super::account_access::{AccountRole, AccountWant};
use super::credential::{StoredKind, source};
use super::format::generate_with_prefix;
use super::service_account::{self, CreateError};

/// What `revoke_reason` says on a token an org admin revoked from the app's row.
pub const REVOKED_BY_ORG: &str = "app access revoked";

/// The display prefix an app's row shows where a token would show one. It is
/// not the start of any secret: there is none.
pub const DISPLAY_PREFIX: &str = "app";

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// The service account `app` acts as, if an approval ever made one.
pub async fn account_of<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
) -> Result<Option<service_accounts::Model>, OxyError> {
    let Some(link) = AppServiceAccounts::find_by_id(app_id)
        .one(db)
        .await
        .map_err(db_err("app service account link"))?
    else {
        return Ok(None);
    };
    ServiceAccounts::find_by_id(link.service_account_user_id)
        .one(db)
        .await
        .map_err(db_err("app service account"))
}

/// The app an account belongs to, if it is an app's. A token is never minted
/// for such an account: that would bring back the stored key this exists to
/// remove.
pub async fn app_of_account<C: ConnectionTrait>(
    db: &C,
    account_user_id: Uuid,
) -> Result<Option<Uuid>, OxyError> {
    Ok(AppServiceAccounts::find()
        .filter(app_service_accounts::Column::ServiceAccountUserId.eq(account_user_id))
        .one(db)
        .await
        .map_err(db_err("app of service account"))?
        .map(|link| link.app_id))
}

/// The app's account, made on its first approval: an admin of the app's org,
/// named for the app. Run inside the approval's transaction.
async fn ensure_account<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    approver: Uuid,
) -> Result<service_accounts::Model, OxyError> {
    if let Some(account) = account_of(db, app.id).await? {
        return Ok(account);
    }
    // Names are unique per org and an org admin may already have used this one
    // for an account of their own, so a taken name gets a numbered suffix.
    let mut created = None;
    for attempt in 0..5u8 {
        let name = match attempt {
            0 => format!("app-{}", app.slug),
            n => format!("app-{}-{}", app.slug, n + 1),
        };
        let want = AccountWant {
            name,
            description: Some(format!(
                "The custom app \"{}\" acts as this account for the platform actions an \
                 admin approved for it. No token can be minted for it.",
                app.name
            )),
            role: AccountRole::Admin,
        };
        match service_account::create(db, app.org_id, want, approver).await {
            Ok(account) => {
                created = Some(account);
                break;
            }
            Err(CreateError::NameTaken) => continue,
            Err(CreateError::Db(e)) => return Err(e),
        }
    }
    let account = created.ok_or_else(|| {
        OxyError::DBError(format!(
            "no free service account name for app {} in its org",
            app.slug
        ))
    })?;
    app_service_accounts::ActiveModel {
        app_id: Set(app.id),
        service_account_user_id: Set(account.user_id),
        created_at: Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .map_err(db_err("link app service account"))?;
    Ok(account)
}

/// The app's live row of kind `app`: unrevoked, acting as `account_user_id`.
pub async fn live_token<C: ConnectionTrait>(
    db: &C,
    account_user_id: Uuid,
) -> Result<Option<api_tokens::Model>, OxyError> {
    ApiTokens::find()
        .filter(api_tokens::Column::PrincipalUserId.eq(account_user_id))
        .filter(api_tokens::Column::Kind.eq(StoredKind::App.as_str()))
        .filter(api_tokens::Column::RevokedAt.is_null())
        .order_by_desc(api_tokens::Column::CreatedAt)
        .one(db)
        .await
        .map_err(db_err("app access token"))
}

/// The app's secretless row, made when its first action is approved (and again
/// after a revoke). Its hash is of a token generated and dropped here.
async fn create_token<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    account: &service_accounts::Model,
    approver: Uuid,
) -> Result<api_tokens::Model, OxyError> {
    let unheld = generate_with_prefix("oxy_app_");
    api_tokens::ActiveModel {
        id: Set(Uuid::new_v4()),
        kind: Set(StoredKind::App.as_str().to_string()),
        principal_user_id: Set(account.user_id),
        name: Set(format!("app:{}", app.slug)),
        display_prefix: Set(DISPLAY_PREFIX.to_string()),
        last_four: Set(String::new()),
        token_hash: Set(unheld.token_hash),
        all_access: Set(false),
        platform: Set(false),
        partner: Set(false),
        // An approval lasts until it is revoked, like an installed GitHub App.
        expires_at: Set(None),
        last_used_at: Set(None),
        created_at: Set(Utc::now().fixed_offset()),
        created_by: Set(Some(approver)),
        revoked_at: Set(None),
        revoked_by: Set(None),
        revoke_reason: Set(None),
        source: Set(source::APP_APPROVAL.to_string()),
        legacy_api_key_id: Set(None),
        trust_policy_id: Set(None),
        oidc_claims: Set(None),
    }
    .insert(db)
    .await
    .map_err(db_err("create app access token"))
}

/// The live approvals under the app's live row, oldest first.
pub async fn approvals<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
) -> Result<Vec<app_platform_grants::Model>, OxyError> {
    AppPlatformGrants::find()
        .filter(app_platform_grants::Column::AppId.eq(app_id))
        .filter(app_platform_grants::Column::RevokedAt.is_null())
        .order_by_asc(app_platform_grants::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err("app platform grants"))
}

/// One thing an org admin approves: an app action, and what it is approved
/// on. `target` is a pipeline's name for an upload, and `None` for an action
/// on the whole org (kiosks). Never a workspace.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApprovedAction {
    pub action: Action,
    pub target: Option<String>,
}

/// What an approval changed.
#[derive(Clone, Debug)]
pub struct Approved {
    pub account: service_accounts::Model,
    pub token: api_tokens::Model,
    /// What this approval added; what was already live is not repeated.
    pub added: Vec<ApprovedAction>,
    /// Whether this approval made the account (the app's first).
    pub account_created: bool,
}

/// Approve `wanted` for `app`. Run it in a transaction, under a session the
/// handler has already decided may approve.
///
/// `wanted` must already be app actions, with the targets, the app's
/// production build declares; the handler checks both.
pub async fn approve<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    wanted: &[ApprovedAction],
    approver: Uuid,
) -> Result<Approved, OxyError> {
    let had_account = account_of(db, app.id).await?.is_some();
    let account = ensure_account(db, app, approver).await?;
    let token = match live_token(db, account.user_id).await? {
        Some(token) => token,
        None => create_token(db, app, &account, approver).await?,
    };
    let live = AppPlatformGrants::find()
        .filter(app_platform_grants::Column::TokenId.eq(token.id))
        .filter(app_platform_grants::Column::RevokedAt.is_null())
        .all(db)
        .await
        .map_err(db_err("app platform grants"))?;
    let now = Utc::now().fixed_offset();
    let mut added = Vec::new();
    for approval in wanted {
        let action = approval.action;
        let Some(id) = action.app_action_id() else {
            return Err(OxyError::ArgumentError(format!(
                "{action:?} cannot be approved for an app"
            )));
        };
        if live
            .iter()
            .any(|g| g.action == id && g.target == approval.target)
            || added.contains(approval)
        {
            continue;
        }
        app_platform_grants::ActiveModel {
            id: Set(Uuid::new_v4()),
            token_id: Set(token.id),
            app_id: Set(app.id),
            org_id: Set(app.org_id),
            action: Set(id.to_string()),
            target: Set(approval.target.clone()),
            created_at: Set(now),
            created_by: Set(Some(approver)),
            revoked_at: Set(None),
            revoked_by: Set(None),
        }
        .insert(db)
        .await
        .map_err(db_err("approve app platform action"))?;
        added.push(approval.clone());
    }
    Ok(Approved {
        account,
        token,
        added,
        account_created: !had_account,
    })
}

/// End the app's access: its live row and every approval under it. `None` when
/// there was nothing live to end. The account stays, disabled by nothing, so a
/// later approval acts as the same principal.
pub async fn revoke<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    revoker: Uuid,
) -> Result<Option<api_tokens::Model>, OxyError> {
    let Some(account) = account_of(db, app_id).await? else {
        return Ok(None);
    };
    let Some(token) = live_token(db, account.user_id).await? else {
        return Ok(None);
    };
    let now = Utc::now().fixed_offset();
    AppPlatformGrants::update_many()
        .col_expr(
            app_platform_grants::Column::RevokedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .col_expr(
            app_platform_grants::Column::RevokedBy,
            sea_orm::sea_query::Expr::value(revoker),
        )
        .filter(app_platform_grants::Column::TokenId.eq(token.id))
        .filter(app_platform_grants::Column::RevokedAt.is_null())
        .exec(db)
        .await
        .map_err(db_err("revoke app platform grants"))?;
    let mut row: api_tokens::ActiveModel = token.into();
    row.revoked_at = Set(Some(now));
    row.revoked_by = Set(Some(revoker));
    row.revoke_reason = Set(Some(REVOKED_BY_ORG.to_string()));
    let revoked = row
        .update(db)
        .await
        .map_err(db_err("revoke app access token"))?;
    Ok(Some(revoked))
}

mod batch;
mod credential;
pub use batch::{accounts_of, approvals_of, live_tokens};
pub use credential::{approved_targets, credential_for, credential_of, effective_grants};

#[cfg(test)]
#[path = "app_access_tests.rs"]
mod tests;
