//! Service accounts and their tokens: the writes behind
//! `/api/orgs/{org_id}/service-accounts` (API-tokens design §3.3).
//!
//! Database primitives only, like [`super::personal`]: who may ask is decided
//! by the handler, which also writes the audit row in the same transaction.
//!
//! **A service account is two rows and no third.** A `users` row with a NULL
//! email — so no provider login, magic link, invitation or email-keyed
//! standing can ever resolve to it — and a `service_accounts` row naming its
//! org and its standing. There is deliberately **no `org_members` row**:
//! membership is what seats, member lists, invitations, app audiences and
//! teams enumerate, and an account must be in none of them.
//!
//! Its tokens are `api_tokens` rows of kind `service_account` whose principal
//! is the account's `users` row, always `all_access = false` with neither
//! standing flag.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::prelude::{ApiTokens, ServiceAccounts, Users};
use entity::{api_tokens, service_accounts, users};
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, SqlErr,
};
use uuid::Uuid;

use super::account_access::{AccountEdit, AccountRole, AccountWant};
use super::credential::{StoredKind, source};
use super::personal::{self, GrantSpec, Minted, NewToken};

/// Why revoking an account's tokens is recorded on each of them.
pub const REVOKED_WITH_ACCOUNT: &str = "service account deleted";

/// Why an account could not be created.
#[derive(Debug)]
pub enum CreateError {
    /// The org already has an account of this name.
    NameTaken,
    Db(OxyError),
}

impl From<OxyError> for CreateError {
    fn from(e: OxyError) -> Self {
        Self::Db(e)
    }
}

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// The account's standing, as stored. `None` for a role this release does not
/// know — which authentication refuses, and a listing shows as it is.
pub fn role_of(account: &service_accounts::Model) -> Option<AccountRole> {
    AccountRole::parse(&account.org_role)
}

/// Create the account: its email-less `users` row, then its standing. Run it
/// in a transaction — the two rows are one account.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    want: AccountWant,
    created_by: Uuid,
) -> Result<service_accounts::Model, CreateError> {
    if find_by_name(db, org_id, &want.name).await?.is_some() {
        return Err(CreateError::NameTaken);
    }
    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: Set(user_id),
        // NULL is the whole point: nothing email-keyed — a provider login, a
        // magic link, an invitation, a platform grant — can resolve to this row.
        email: Set(None),
        // The display label. A slug, so it never contains `@`.
        name: Set(want.name.clone()),
        picture: Set(None),
        email_verified: Set(false),
        magic_link_token: ActiveValue::NotSet,
        magic_link_token_expires_at: ActiveValue::NotSet,
        status: Set(users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .map_err(db_err("create service account user"))?;

    service_accounts::ActiveModel {
        user_id: Set(user_id),
        org_id: Set(org_id),
        org_role: Set(want.role.as_str().to_string()),
        name: Set(want.name),
        description: Set(want.description),
        created_by: Set(Some(created_by)),
        created_at: Set(Utc::now().fixed_offset()),
        disabled_at: Set(None),
    }
    .insert(db)
    .await
    .map_err(|e| match e.sql_err() {
        // Two creates of one name racing: the unique index decides.
        Some(SqlErr::UniqueConstraintViolation(_)) => CreateError::NameTaken,
        _ => CreateError::Db(db_err("create service account")(e)),
    })
}

async fn find_by_name<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    name: &str,
) -> Result<Option<service_accounts::Model>, OxyError> {
    ServiceAccounts::find()
        .filter(service_accounts::Column::OrgId.eq(org_id))
        .filter(service_accounts::Column::Name.eq(name))
        .one(db)
        .await
        .map_err(db_err("service account lookup"))
}

/// The org's accounts, by name.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
) -> Result<Vec<service_accounts::Model>, OxyError> {
    ServiceAccounts::find()
        .filter(service_accounts::Column::OrgId.eq(org_id))
        .order_by_asc(service_accounts::Column::Name)
        .all(db)
        .await
        .map_err(db_err("list service accounts"))
}

/// One account **of this org**. Another org's reads the same as none.
pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    account_id: Uuid,
) -> Result<Option<service_accounts::Model>, OxyError> {
    Ok(ServiceAccounts::find_by_id(account_id)
        .one(db)
        .await
        .map_err(db_err("service account lookup"))?
        .filter(|a| a.org_id == org_id))
}

/// The account **of this org** as it is now, under its row lock
/// (`FOR UPDATE`). Another org's reads the same as none.
///
/// What an edit, a disable or a delete does **first** in its transaction, and
/// **the row it returns is the one to decide from**: a mint in flight for one
/// of the account's policies holds this row shared (`super::ci_mint`), so its
/// token is committed before the change goes on; and a row read before the
/// transaction may be one another request has since changed — a disable
/// that judged "already disabled" from it would do nothing, and say it had.
#[must_use = "decide from the row this returns: it is the account as it is under the lock"]
pub async fn lock<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    account_id: Uuid,
) -> Result<Option<service_accounts::Model>, OxyError> {
    Ok(ServiceAccounts::find_by_id(account_id)
        .lock_exclusive()
        .one(db)
        .await
        .map_err(db_err("lock service account"))?
        .filter(|a| a.org_id == org_id))
}

/// Apply an edit. Disabling stamps `disabled_at` once; enabling clears it.
/// `row` is the account under [`lock`]: what was and was not disabled is read
/// off it.
pub async fn update<C: ConnectionTrait>(
    db: &C,
    row: service_accounts::Model,
    edit: &AccountEdit,
) -> Result<service_accounts::Model, OxyError> {
    let was_disabled = row.disabled_at.is_some();
    let mut active: service_accounts::ActiveModel = row.into();
    if let Some(description) = &edit.description {
        active.description = Set(description.clone());
    }
    if let Some(role) = edit.role {
        active.org_role = Set(role.as_str().to_string());
    }
    match edit.disabled {
        Some(true) if !was_disabled => active.disabled_at = Set(Some(Utc::now().fixed_offset())),
        Some(false) => active.disabled_at = Set(None),
        _ => {}
    }
    active
        .update(db)
        .await
        .map_err(db_err("update service account"))
}

/// Delete the account: revoke its live tokens, drop its standing, and retire
/// its `users` row. Returns the tokens this call revoked, for the audit rows
/// and the credential cache.
///
/// The `users` row stays, marked deleted: `created_by` columns and audit rows
/// name it, and a deleted user authenticates nowhere. The name is free again.
///
/// The account's trust policies go with it: they reference this row and are
/// deleted by the same statement (`ON DELETE CASCADE`), so no run can mint for
/// an account that is gone. The `ci` tokens they minted are revoked here with
/// the account's own.
///
/// `row` is the account under [`lock`], taken first in the caller's
/// transaction: a mint in flight for one of its policies holds the row shared,
/// so that mint's token is committed — and found by `every_token` here —
/// before anything is revoked. The policies' locks follow in the cascade: the
/// order a mint takes them in.
pub async fn delete<C: ConnectionTrait>(
    db: &C,
    row: service_accounts::Model,
    by: Uuid,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    let mut revoked = Vec::new();
    for token in every_token(db, row.user_id).await? {
        if let Some(token) = personal::revoke(db, token, by, REVOKED_WITH_ACCOUNT).await? {
            revoked.push(token);
        }
    }
    let user_id = row.user_id;
    ServiceAccounts::delete_by_id(user_id)
        .exec(db)
        .await
        .map_err(db_err("delete service account"))?;
    Users::update_many()
        .col_expr(
            users::Column::Status,
            sea_orm::sea_query::Expr::value(users::UserStatus::Deleted),
        )
        .filter(users::Column::Id.eq(user_id))
        .exec(db)
        .await
        .map_err(db_err("retire service account user"))?;
    Ok(revoked)
}

fn account_tokens(account_id: Uuid) -> sea_orm::Select<ApiTokens> {
    ApiTokens::find()
        .filter(api_tokens::Column::PrincipalUserId.eq(account_id))
        .filter(api_tokens::Column::Kind.eq(StoredKind::ServiceAccount.as_str()))
}

/// Every token that acts as the account: the `oxy_sat_` tokens an admin
/// minted and the `oxy_ci_` tokens its trust policies minted. What deleting
/// or disabling the account must reach.
pub async fn every_token<C: ConnectionTrait>(
    db: &C,
    account_id: Uuid,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    ApiTokens::find()
        .filter(api_tokens::Column::PrincipalUserId.eq(account_id))
        .filter(
            api_tokens::Column::Kind
                .is_in([StoredKind::ServiceAccount.as_str(), StoredKind::Ci.as_str()]),
        )
        .all(db)
        .await
        .map_err(db_err("list a service account's tokens"))
}

/// The account's own (`oxy_sat_`) tokens, newest first — revoked ones
/// included. The short-lived `ci` tokens its trust policies mint are not
/// listed here; the org token inventory shows them.
pub async fn tokens<C: ConnectionTrait>(
    db: &C,
    account_id: Uuid,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    account_tokens(account_id)
        .order_by_desc(api_tokens::Column::CreatedAt)
        .order_by_desc(api_tokens::Column::Id)
        .all(db)
        .await
        .map_err(db_err("list service account tokens"))
}

/// One token **of this account**. Any other token reads the same as none.
pub async fn find_token<C: ConnectionTrait>(
    db: &C,
    account_id: Uuid,
    token_id: Uuid,
) -> Result<Option<api_tokens::Model>, OxyError> {
    account_tokens(account_id)
        .filter(api_tokens::Column::Id.eq(token_id))
        .one(db)
        .await
        .map_err(db_err("service account token lookup"))
}

/// How many tokens each account holds that are not revoked.
pub async fn token_counts<C: ConnectionTrait>(
    db: &C,
    account_ids: &[Uuid],
) -> Result<HashMap<Uuid, u64>, OxyError> {
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = ApiTokens::find()
        .filter(api_tokens::Column::PrincipalUserId.is_in(account_ids.to_vec()))
        .filter(api_tokens::Column::Kind.eq(StoredKind::ServiceAccount.as_str()))
        .filter(api_tokens::Column::RevokedAt.is_null())
        .all(db)
        .await
        .map_err(db_err("count service account tokens"))?;
    let mut counts: HashMap<Uuid, u64> = HashMap::new();
    for row in rows {
        *counts.entry(row.principal_user_id).or_default() += 1;
    }
    Ok(counts)
}

/// What to mint for an account.
#[derive(Clone, Debug)]
pub struct NewAccountToken {
    pub name: String,
    /// Never empty: [`super::account_access::account_grants`].
    pub grants: Vec<GrantSpec>,
    pub expires_at: Option<DateTime<Utc>>,
    /// The person minting it — audit only.
    pub created_by: Uuid,
    /// `credential::source::UI` from the org's API access page; Phase 4's
    /// exchange passes its own.
    pub source: &'static str,
}

impl NewAccountToken {
    /// A token minted by an org admin in the browser.
    pub fn from_ui(
        name: String,
        grants: Vec<GrantSpec>,
        expires_at: Option<DateTime<Utc>>,
        created_by: Uuid,
    ) -> Self {
        Self {
            name,
            grants,
            expires_at,
            created_by,
            source: source::UI,
        }
    }
}

/// Mint an `oxy_sat_` token acting as `account`: grant-bound, never
/// all-access, with neither standing flag — whatever the caller would like.
pub async fn mint<C: ConnectionTrait>(
    db: &C,
    account: &service_accounts::Model,
    new: NewAccountToken,
) -> Result<Minted, OxyError> {
    let token = NewToken {
        user_id: account.user_id,
        name: new.name,
        all_access: false,
        platform: false,
        partner: false,
        grants: new.grants,
        expires_at: new.expires_at,
        source: new.source,
    };
    personal::create_of_kind(db, StoredKind::ServiceAccount, new.created_by, token).await
}
