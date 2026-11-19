//! Minting a **service-account token** through `oxyc` (`oxyc tokens create
//! --org-service-account`; the mint's shape is
//! `oxy_auth::token::service_account_mint`).
//!
//! An org's API access page creates a service account and mints its
//! `oxy_sat_` token under an org admin's browser session. `oxyc` asks for the
//! same two writes with the PKCE exchange `oxyc login` uses (`cli_login.rs`),
//! or by device code (`device_login.rs`): the approval page, under the
//! approver's session, approves a `mint` of `kind: "service_account"`, and the
//! CLI redeems the code for the token.
//!
//! ## Only a browser session approves it
//!
//! `POST /orgs/{org_id}/service-accounts` and `…/{sa_id}/tokens` are
//! session-only (API-tokens design §4.6): a leaked token must not mint its
//! successor. Nothing here loosens that. `authorize` and the device-code
//! approval take `SessionOnly` before they read a body, so a token asking
//! answers 403 `session_required`; the exchange carries no credential at all,
//! only the code the session issued.
//!
//! ## Who may approve it
//!
//! Whoever passes the door those two routes take, for the org the mint names
//! ([`ManagesServiceAccounts`], `OrgAdminFor<ServiceAccounts>`): an owner or
//! admin of the org, or staff in a live assume-role session for it. The org is
//! not in the path, so it is resolved the way `org_middleware` resolves one
//! (`resolve_org_context`) and the guard decides on it (`OrgAdminFor::of`).
//! It is asked at the approval and again at the exchange: the code is good for
//! five minutes, and the approver's standing can change in them.
//!
//! ## What it mints
//!
//! The org's account of that name, or a new one at the asked role (`member`
//! unless `admin` was asked for). An existing account whose role differs is
//! refused (409 `role_mismatch`), never changed; a disabled one is refused
//! (409 `service_account_disabled`), since its token would authenticate
//! nothing. The token names no grant, so it gets the one org-wide grant at the
//! account's role that a token made on the API access page with none gets.
//!
//! The writes are the API access page's own: the account through
//! `accounts::create_row` (its `service_account.created` row), the token
//! through `account_tokens::plan` and `mint` (the org's lifetime cap, its
//! `token.created` row). Every check that can refuse the token runs before the
//! account is created, so a refused mint creates no account.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use entity::prelude::Organizations;
use entity::{organizations, service_accounts};
use oxy_app::surface::resolve_org_context;
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::account_access::{AccountRole, CreateAccountBody, CreateAccountTokenBody};
use oxy_auth::token::credential::source;
use oxy_auth::token::service_account as store;
use oxy_auth::token::service_account_mint::{self as mint, MintRequest};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::cli_login::Collected;
use super::error::TokenError;
use super::service::Minted;
use crate::authz;
use crate::org_api_access::account_tokens::{self, Planned};
use crate::org_api_access::{ManagesServiceAccounts, accounts};

fn parse(body: &Value, default_name: Option<&str>) -> Result<MintRequest, TokenError> {
    mint::parse(body, default_name).map_err(|e| TokenError::InvalidServiceAccountToken(e.0))
}

/// The org a mint names: by id when it is one, otherwise by slug.
async fn org_named(
    db: &DatabaseConnection,
    raw: &str,
) -> Result<Option<organizations::Model>, TokenError> {
    let found = match Uuid::parse_str(raw) {
        Ok(id) => Organizations::find_by_id(id).one(db).await?,
        Err(_) => {
            Organizations::find()
                .filter(organizations::Column::Slug.eq(raw))
                .one(db)
                .await?
        }
    };
    Ok(found)
}

/// A mint that has passed every check a session can make: the org, the
/// approver's standing in it, the account as it stands, and the token.
struct Checked {
    org: organizations::Model,
    /// The org's account of the asked name, when it has one.
    account: Option<service_accounts::Model>,
    planned: Planned,
}

/// The create body of the token a mint asks for: its name and lifetime, and
/// no grant — so it gets the one org-wide grant at the account's role.
fn token_body(request: &MintRequest) -> CreateAccountTokenBody {
    CreateAccountTokenBody {
        name: request.name.clone(),
        grants: None,
        expires_in_days: Some(request.days),
        expires_at: None,
    }
}

/// The approver may manage the service accounts of the org `request` names:
/// the org as `org_middleware` would resolve it for them, and the door the
/// API access routes take. An org that does not exist and one they are not in
/// read the same.
pub(super) async fn managed_org(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org: &str,
) -> Result<organizations::Model, TokenError> {
    let not_found = || TokenError::OrgNotFound(org.to_string());
    let org = org_named(db, org).await?.ok_or_else(not_found)?;
    let ctx = match resolve_org_context(&actor.user, org.id).await {
        Ok(ctx) => ctx,
        Err(StatusCode::NOT_FOUND | StatusCode::FORBIDDEN) => return Err(not_found()),
        Err(status) => {
            return Err(TokenError::Internal(format!(
                "resolving the org of a service-account mint answered {status}"
            )));
        }
    };
    match ManagesServiceAccounts::of(db, &authz::caller_of(actor), ctx).await {
        Ok(_) => Ok(org),
        Err(_) => Err(TokenError::OrgAdminRequired),
    }
}

/// An existing account may be minted for only as it is: at the asked role,
/// enabled, and not a custom app's.
async fn usable(
    db: &DatabaseConnection,
    account: &service_accounts::Model,
    role: AccountRole,
) -> Result<(), TokenError> {
    if store::role_of(account) != Some(role) {
        return Err(TokenError::RoleMismatch {
            account: account.name.clone(),
            role: account.org_role.clone(),
        });
    }
    usable_as_is(db, account).await
}

/// An existing account may be given more only while it is enabled: whatever a
/// disabled account is given authenticates nothing. Nor may a custom app's
/// account (`app-<slug>`): it acts only through the actions an org admin
/// approved for the app, and a token or trust policy on it would carry its
/// admin standing without that narrowing. Checked here so the approval page
/// refuses it before Approve is offered; the mint and the trust policy write
/// refuse it again.
pub(super) async fn usable_as_is(
    db: &DatabaseConnection,
    account: &service_accounts::Model,
) -> Result<(), TokenError> {
    if account.disabled_at.is_some() {
        return Err(TokenError::ServiceAccountDisabled(account.name.clone()));
    }
    if oxy_auth::token::app_access::app_of_account(db, account.user_id)
        .await?
        .is_some()
    {
        return Err(TokenError::AppAccountFixed);
    }
    Ok(())
}

/// Every check, in the order a person would want to hear about them.
async fn check(
    db: &DatabaseConnection,
    actor: &RequestActor,
    request: &MintRequest,
) -> Result<Checked, TokenError> {
    let org = managed_org(db, actor, &request.org).await?;
    let account = store::find_by_name(db, org.id, &request.account).await?;
    if let Some(account) = &account {
        usable(db, account, request.role).await?;
    }
    let planned = account_tokens::plan(db, org.id, request.role, &token_body(request))
        .await
        .map_err(|e| match e {
            TokenError::Invalid(why) => TokenError::InvalidServiceAccountToken(why),
            other => other,
        })?;
    Ok(Checked {
        org,
        account,
        planned,
    })
}

/// A `mint` of `kind: "service_account"`, approved under the session (`POST
/// /auth/cli/authorize`, or a device-code approval): checked, and returned as
/// the code or request stores it — the org by id. The CLI's collection can
/// only answer `invalid_code`, so this is the last place a refusal can say why.
pub(super) async fn approve(
    db: &DatabaseConnection,
    actor: &RequestActor,
    hostname: &str,
    body: &Value,
) -> Result<Value, TokenError> {
    let request = parse(body, Some(&mint::default_name(hostname)))?;
    let checked = check(db, actor, &request).await?;
    Ok(request.stored(checked.org.id))
}

/// What a new account's description says, on the API access page.
fn description(hostname: &str) -> String {
    format!("Created by `oxyc tokens create --org-service-account` on {hostname}.")
}

/// The org's account of the asked name: the one there, or a new one. A create
/// that loses a race to another of the same name takes the winner, held to the
/// same rules as one that was there all along.
async fn account_for(
    db: &DatabaseConnection,
    actor: &RequestActor,
    request: &MintRequest,
    checked: &Checked,
    hostname: &str,
) -> Result<service_accounts::Model, TokenError> {
    if let Some(account) = &checked.account {
        return Ok(account.clone());
    }
    let body = CreateAccountBody {
        name: request.account.clone(),
        description: Some(description(hostname)),
        org_role: Some(request.role.as_str().to_string()),
    };
    match accounts::create_row(db, actor, checked.org.id, body).await {
        Ok(account) => Ok(account),
        Err(TokenError::NameTaken) => {
            let account = store::find_by_name(db, checked.org.id, &request.account)
                .await?
                .ok_or_else(|| TokenError::Internal("a taken name names no account".into()))?;
            usable(db, &account, request.role).await?;
            Ok(account)
        }
        Err(e) => Err(e),
    }
}

/// Mint the token an approval named, as `actor` — the user who approved it.
/// Everything is asked again first: the approval is minutes old.
pub(super) async fn redeem(
    db: &DatabaseConnection,
    actor: &RequestActor,
    approved: &Value,
    hostname: &str,
    collected: Collected,
) -> Result<Minted, TokenError> {
    let request = parse(approved, None)?;
    let checked = check(db, actor, &request).await?;
    let account = account_for(db, actor, &request, &checked, hostname).await?;
    let mut extra = Map::new();
    extra.insert("hostname".into(), json!(hostname));
    extra.extend(collected.via);
    let minted =
        account_tokens::mint(db, actor, &account, checked.planned, source::OXYC, extra).await?;
    Ok(Minted {
        token: minted.token,
        secret: minted.secret,
    })
}

// ── The approval page's preview ──────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PreviewOrg {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct PreviewAccount {
    pub name: String,
    pub role: &'static str,
    /// `false`: approving creates it.
    pub exists: bool,
}

#[derive(Debug, Serialize)]
pub struct PreviewToken {
    pub name: String,
    pub expires_in_days: i64,
    /// When a token minted now would expire.
    pub expires_at: Option<DateTime<Utc>>,
}

/// What the token reaches: one grant, over the whole org, at the account's role.
#[derive(Debug, Serialize)]
pub struct PreviewGrant {
    pub org_wide: bool,
    pub role_ceiling: &'static str,
}

/// What approving a mint would create, as the server would create it: what
/// the approval page lays out, so the page shows what was checked rather than
/// its own reading of the request.
#[derive(Debug, Serialize)]
pub struct Preview {
    pub org: PreviewOrg,
    pub account: PreviewAccount,
    pub token: PreviewToken,
    pub grant: PreviewGrant,
}

/// The preview of `body`, approved by `actor` on behalf of `hostname`: every
/// check the approval makes, and nothing written. A mint that would be refused
/// is refused here, with the same answer.
pub(super) async fn preview(
    db: &DatabaseConnection,
    actor: &RequestActor,
    hostname: &str,
    body: &Value,
) -> Result<Preview, TokenError> {
    let request = parse(body, Some(&mint::default_name(hostname)))?;
    let checked = check(db, actor, &request).await?;
    let role = request.role.as_str();
    Ok(Preview {
        org: PreviewOrg {
            id: checked.org.id,
            slug: checked.org.slug.clone(),
            name: checked.org.name.clone(),
        },
        account: PreviewAccount {
            name: request.account.clone(),
            role,
            exists: checked.account.is_some(),
        },
        token: PreviewToken {
            name: request.name.clone(),
            expires_in_days: request.days,
            expires_at: checked.planned.expires_at(),
        },
        grant: PreviewGrant {
            org_wide: true,
            role_ceiling: role,
        },
    })
}
