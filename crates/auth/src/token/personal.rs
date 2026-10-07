//! Personal access tokens: the writes behind `/api/user/tokens`.
//!
//! Database primitives only. Who may ask for what — a grant the caller can
//! reach, a standing they hold — is decided by the handler, which also writes
//! the lifecycle audit row in the same transaction. Each function therefore
//! takes any `ConnectionTrait`.
//!
//! A personal token lives in `api_tokens` alone: no `api_keys` row, so a pod
//! one release back does not know it and refuses it (a revert fails it closed,
//! design §4.7). A **legacy** row — one that mirrors `api_keys` — is not
//! written here: its extend and revoke go through
//! [`crate::api_key_domain::ApiKeyService`], which keeps both tables in step.

use chrono::{DateTime, Utc};
use entity::prelude::{ApiTokenGrants, ApiTokens};
use entity::{api_token_grants, api_tokens};
use oxy_authz::RoleCeiling;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use super::credential::StoredKind;
use super::format::{
    GeneratedToken, generate_ci, generate_personal, generate_sandbox_agent,
    generate_service_account,
};
use super::grant_plan::GrantPlan;
use super::grant_row::GrantRow;

/// A token created without an expiry choice lasts this long.
pub const DEFAULT_LIFETIME_DAYS: i64 = 90;

/// How long the token `oxyc login` mints lasts.
///
/// Longer than [`DEFAULT_LIFETIME_DAYS`] on purpose. A login is re-approved in
/// a browser each time it lapses, on every deployment a person works against,
/// and an agent or script that finds it lapsed fails where nobody is watching.
/// What makes a year acceptable is that the token is no longer invisible:
/// staff see every token that carries a standing and can end one
/// (`/api/admin/standing-tokens`), and the owner can still end or extend it.
pub const LOGIN_LIFETIME_DAYS: i64 = 365;

/// One workspace grant to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantSpec {
    pub org_id: Uuid,
    /// `None` = every workspace in the org.
    pub workspace_id: Option<Uuid>,
    pub ceiling: RoleCeiling,
}

/// What to mint.
#[derive(Clone, Debug)]
pub struct NewToken {
    pub user_id: Uuid,
    pub name: String,
    pub all_access: bool,
    pub platform: bool,
    pub partner: bool,
    /// Stored only when `all_access` is false.
    pub grants: Vec<GrantSpec>,
    pub expires_at: Option<DateTime<Utc>>,
    /// `credential::source::{UI, OXYC_LOGIN, OXYC_AGENT}`.
    pub source: &'static str,
}

/// A freshly minted (or regenerated) token: the row, and the secret shown once.
#[derive(Clone, Debug)]
pub struct Minted {
    pub row: api_tokens::Model,
    pub secret: String,
}

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// A fresh secret of the format `kind` is presented in. A legacy key is never
/// minted here: its format belongs to the legacy endpoint.
pub(super) fn generate_for(kind: &str) -> GeneratedToken {
    match StoredKind::parse(kind) {
        Some(StoredKind::ServiceAccount) => generate_service_account(),
        Some(StoredKind::Ci) => generate_ci(),
        Some(StoredKind::SandboxAgent) => generate_sandbox_agent(),
        _ => generate_personal(),
    }
}

/// Mint an `oxy_pat_` token and store its hash and grants.
pub async fn create<C: ConnectionTrait>(db: &C, new: NewToken) -> Result<Minted, OxyError> {
    let created_by = new.user_id;
    create_of_kind(db, StoredKind::Personal, created_by, new).await
}

/// Mint a token of `kind` acting as `new.user_id`, recorded as created by
/// `created_by` — the person, when the principal is a service account.
pub(super) async fn create_of_kind<C: ConnectionTrait>(
    db: &C,
    kind: StoredKind,
    created_by: Uuid,
    new: NewToken,
) -> Result<Minted, OxyError> {
    let token = generate_for(kind.as_str());
    let id = Uuid::new_v4();
    let row = api_tokens::ActiveModel {
        id: Set(id),
        kind: Set(kind.as_str().to_string()),
        principal_user_id: Set(new.user_id),
        name: Set(new.name),
        display_prefix: Set(token.display_prefix),
        last_four: Set(token.last_four),
        token_hash: Set(token.token_hash),
        all_access: Set(new.all_access),
        platform: Set(new.platform),
        partner: Set(new.partner),
        expires_at: Set(new.expires_at.map(|t| t.fixed_offset())),
        last_used_at: Set(None),
        created_at: Set(Utc::now().fixed_offset()),
        created_by: Set(Some(created_by)),
        revoked_at: Set(None),
        revoked_by: Set(None),
        revoke_reason: Set(None),
        source: Set(new.source.to_string()),
        legacy_api_key_id: Set(None),
        trust_policy_id: Set(None),
        oidc_claims: Set(None),
    }
    .insert(db)
    .await
    .map_err(db_err("create api token"))?;
    if !new.all_access {
        insert_grants(db, id, &new.grants).await?;
    }
    Ok(Minted {
        row,
        secret: token.plaintext,
    })
}

async fn insert_grants<C: ConnectionTrait>(
    db: &C,
    token_id: Uuid,
    grants: &[GrantSpec],
) -> Result<(), OxyError> {
    let now = Utc::now().fixed_offset();
    for grant in grants {
        GrantRow::workspace(grant)
            .for_token(token_id, now)
            .insert(db)
            .await
            .map_err(db_err("create api token grant"))?;
    }
    Ok(())
}

/// Apply an edit's grant plan ([`super::grant_plan`]): delete the live rows it
/// drops, insert the ones it adds. The delete is confined to this token's
/// **live** rows whatever the plan names — a grant its org revoked is history,
/// not the owner's to rewrite, and stays.
pub async fn apply_grant_plan<C: ConnectionTrait>(
    db: &C,
    token_id: Uuid,
    plan: &GrantPlan,
) -> Result<(), OxyError> {
    if !plan.delete.is_empty() {
        ApiTokenGrants::delete_many()
            .filter(api_token_grants::Column::TokenId.eq(token_id))
            .filter(api_token_grants::Column::RevokedAt.is_null())
            .filter(api_token_grants::Column::Id.is_in(plan.delete.clone()))
            .exec(db)
            .await
            .map_err(db_err("clear api token grants"))?;
    }
    insert_grants(db, token_id, &plan.insert).await
}

/// Every grant row of these tokens, revoked ones included, oldest first.
pub async fn grants_for<C: ConnectionTrait>(
    db: &C,
    token_ids: &[Uuid],
) -> Result<Vec<api_token_grants::Model>, OxyError> {
    if token_ids.is_empty() {
        return Ok(Vec::new());
    }
    ApiTokenGrants::find()
        .filter(api_token_grants::Column::TokenId.is_in(token_ids.to_vec()))
        .order_by_asc(api_token_grants::Column::CreatedAt)
        .order_by_asc(api_token_grants::Column::Id)
        .all(db)
        .await
        .map_err(db_err("api token grants lookup"))
}

/// The kinds a person owns directly and manages on the token routes: their
/// personal tokens, and the sandbox agent tokens they minted.
const OWNED_KINDS: [StoredKind; 2] = [StoredKind::Personal, StoredKind::SandboxAgent];

/// True for a row the token routes serve: a personal token or a sandbox agent
/// token that mirrors no `api_keys` row. A legacy API key is never one — it
/// has its own routes (`/api/{workspace_id}/api-keys`) and its own section in
/// the UI.
fn is_owned_token(row: &api_tokens::Model) -> bool {
    row.legacy_api_key_id.is_none()
        && StoredKind::parse(&row.kind).is_some_and(|kind| OWNED_KINDS.contains(&kind))
}

/// The user's personal access tokens and sandbox agent tokens, newest first —
/// revoked ones included, so a revoked token keeps its row and its Activity.
/// Legacy API keys are not tokens and are never listed here.
pub async fn list_owned<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    ApiTokens::find()
        .filter(api_tokens::Column::PrincipalUserId.eq(user_id))
        .filter(api_tokens::Column::Kind.is_in(OWNED_KINDS.map(StoredKind::as_str)))
        .filter(api_tokens::Column::LegacyApiKeyId.is_null())
        .order_by_desc(api_tokens::Column::CreatedAt)
        .order_by_desc(api_tokens::Column::Id)
        .all(db)
        .await
        .map_err(db_err("list api tokens"))
}

/// The caller's own personal or sandbox agent token, revoked or not. `None`
/// when it does not exist, is someone else's, is a legacy API key, or is of a
/// kind a user does not own directly — all indistinguishable on purpose.
pub async fn find_owned<C: ConnectionTrait>(
    db: &C,
    token_id: Uuid,
    user_id: Uuid,
) -> Result<Option<api_tokens::Model>, OxyError> {
    Ok(ApiTokens::find_by_id(token_id)
        .one(db)
        .await
        .map_err(db_err("api token lookup"))?
        .filter(|t| t.principal_user_id == user_id && is_owned_token(t)))
}

/// What an edit may change on a personal token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub name: String,
    pub all_access: bool,
    pub platform: bool,
    pub partner: bool,
}

impl Settings {
    pub fn of(row: &api_tokens::Model) -> Self {
        Self {
            name: row.name.clone(),
            all_access: row.all_access,
            platform: row.platform,
            partner: row.partner,
        }
    }
}

pub async fn update_settings<C: ConnectionTrait>(
    db: &C,
    row: api_tokens::Model,
    settings: Settings,
) -> Result<api_tokens::Model, OxyError> {
    let mut active: api_tokens::ActiveModel = row.into();
    active.name = Set(settings.name);
    active.all_access = Set(settings.all_access);
    active.platform = Set(settings.platform);
    active.partner = Set(settings.partner);
    active.update(db).await.map_err(db_err("update api token"))
}

/// Set the expiry (Extend), and reset the token's hygiene state: a new expiry
/// gets a new notice, and a revived token is not swept again.
pub async fn set_expiry<C: ConnectionTrait>(
    db: &C,
    row: api_tokens::Model,
    expires_at: Option<DateTime<Utc>>,
) -> Result<api_tokens::Model, OxyError> {
    let mut active: api_tokens::ActiveModel = row.into();
    active.expires_at = Set(expires_at.map(|t| t.fixed_offset()));
    let row = active
        .update(db)
        .await
        .map_err(db_err("extend api token"))?;
    super::hygiene::reset_on_extend(db, row.id).await?;
    Ok(row)
}

/// Give the token a new secret. Same id, grants and expiry; the old secret
/// stops resolving the moment this commits (and within the cache TTL on other
/// pods — invalidate here after the commit).
pub async fn regenerate<C: ConnectionTrait>(
    db: &C,
    row: api_tokens::Model,
) -> Result<Minted, OxyError> {
    let token = generate_for(&row.kind);
    let mut active: api_tokens::ActiveModel = row.into();
    active.token_hash = Set(token.token_hash);
    active.display_prefix = Set(token.display_prefix);
    active.last_four = Set(token.last_four);
    active.last_used_at = Set(None);
    let row = active
        .update(db)
        .await
        .map_err(db_err("regenerate api token"))?;
    super::hygiene::mark_renewed(db, row.id).await?;
    Ok(Minted {
        row,
        secret: token.plaintext,
    })
}

/// Revoke a personal token. `Ok(None)` when it was already revoked: nothing
/// changed, so the caller records nothing.
pub async fn revoke<C: ConnectionTrait>(
    db: &C,
    row: api_tokens::Model,
    by: Uuid,
    reason: &str,
) -> Result<Option<api_tokens::Model>, OxyError> {
    if row.revoked_at.is_some() {
        return Ok(None);
    }
    let mut active: api_tokens::ActiveModel = row.into();
    active.revoked_at = Set(Some(Utc::now().fixed_offset()));
    active.revoked_by = Set(Some(by));
    active.revoke_reason = Set(Some(reason.to_string()));
    active
        .update(db)
        .await
        .map(Some)
        .map_err(db_err("revoke api token"))
}

/// The user's live tokens of one name and source — how `oxyc login` finds the
/// previous `oxyc on <hostname>` token to retire.
pub async fn live_named<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    name: &str,
    source: &str,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    ApiTokens::find()
        .filter(api_tokens::Column::PrincipalUserId.eq(user_id))
        .filter(api_tokens::Column::Kind.eq(StoredKind::Personal.as_str()))
        .filter(api_tokens::Column::Source.eq(source))
        .filter(api_tokens::Column::Name.eq(name))
        .filter(api_tokens::Column::RevokedAt.is_null())
        .all(db)
        .await
        .map_err(db_err("api token lookup"))
}
