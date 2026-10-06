//! The sandbox agent token (`oxy_sbx_`): what a mint asks for, and the writes
//! behind it (sandbox agent credential design §2).
//!
//! A staff member mints one for an agent. It names one to five custom apps and
//! lives for hours; the agent runs the sandbox loop on those apps with it, as
//! its minter, in sandboxes it creates itself.
//!
//! The kind is **fixed at mint**: never all-access, staff standing on, partner
//! standing off, an expiry always, and no grant but `app_sandbox`. Nothing
//! edits it afterwards — it is not renamed, extended or regenerated, only
//! revoked — so a leaked one cannot be kept alive.
//!
//! Like [`super::personal`], this module is parsing and database primitives.
//! *Who* may mint for *which* app needs the caller's standing and is the
//! handler's, as is the audit row.

use chrono::{DateTime, Duration, Utc};
use entity::prelude::{ApiTokenGrants, ApiTokens, Apps};
use entity::{api_token_grants, api_tokens, apps};
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::access::{Invalid, clean_name};
use super::credential::{AppSandboxGrant, SandboxAppHome, StoredKind};
use super::grant_row::GrantRow;
use super::personal::{self, Minted, NewToken};

/// `kind` in a mint body that asks for this token.
pub const KIND: &str = "sandbox_agent";
/// The lifetime a mint gets when it does not ask for one.
pub const DEFAULT_HOURS: i64 = 8;
/// The longest lifetime a mint may ask for: seven days.
pub const MAX_HOURS: i64 = 168;
/// The most apps one token may name.
pub const MAX_APPS: usize = 5;

/// The fields of a personal token's create body. A sandbox agent token takes
/// none of them — its access is the kind's, not the caller's choice — and a
/// body that sends one is refused rather than have it quietly ignored.
const PERSONAL_FIELDS: [&str; 6] = [
    "all_access",
    "platform",
    "partner",
    "grants",
    "expires_in_days",
    "expires_at",
];

/// What a mint asks for, checked for shape and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintRequest {
    pub name: String,
    /// One to [`MAX_APPS`] distinct app ids, **as sent**. One that is not a
    /// UUID names no app; telling the caller so is the handler's, with the
    /// same answer an app they cannot reach gets.
    pub apps: Vec<String>,
    /// 1 to [`MAX_HOURS`].
    pub hours: i64,
}

fn invalid<T>(message: impl Into<String>) -> Result<T, Invalid> {
    Err(Invalid(message.into()))
}

/// Whether a create body asks for a sandbox agent token. `Ok(false)` for a
/// body with no `kind` or `kind: "personal"`, which is a personal token's and
/// is read exactly as before. Any other `kind` is refused: minting a personal
/// token for a body that asked for something else would be minting more than
/// was asked for.
pub fn asked_for(body: &Value) -> Result<bool, Invalid> {
    match body.get("kind") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::String(kind)) if kind == StoredKind::Personal.as_str() => Ok(false),
        Some(Value::String(kind)) if kind == KIND => Ok(true),
        Some(other) => invalid(format!(
            "'kind' must be \"personal\" or \"{KIND}\", not {other}"
        )),
    }
}

fn name_of(body: &Map<String, Value>, default_name: Option<&str>) -> Result<String, Invalid> {
    match (body.get("name"), default_name) {
        (Some(Value::String(name)), _) => clean_name(name),
        (None | Some(Value::Null), Some(default)) => clean_name(default),
        _ => invalid("'name' must be a string of 1 to 100 characters"),
    }
}

fn apps_of(body: &Map<String, Value>) -> Result<Vec<String>, Invalid> {
    let Some(Value::Array(items)) = body.get("apps") else {
        return invalid(format!(
            "'apps' must list 1 to {MAX_APPS} app ids: a sandbox agent token names its apps"
        ));
    };
    if items.is_empty() || items.len() > MAX_APPS {
        return invalid(format!("'apps' must list 1 to {MAX_APPS} app ids"));
    }
    let mut apps: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let Value::String(raw) = item else {
            return invalid("'apps' must be a list of app id strings");
        };
        // Compared as the id they name where they name one, so the same app
        // written in two cases is still named twice.
        let key = Uuid::parse_str(raw.trim())
            .map(|id| id.to_string())
            .unwrap_or_else(|_| raw.trim().to_string());
        if apps.contains(&key) {
            return invalid(format!("'apps' names {key} more than once"));
        }
        apps.push(key);
    }
    Ok(apps)
}

fn hours_of(body: &Map<String, Value>) -> Result<i64, Invalid> {
    let hours = match body.get("expires_in_hours") {
        None | Some(Value::Null) => return Ok(DEFAULT_HOURS),
        Some(value) => value.as_i64(),
    };
    match hours {
        Some(hours) if (1..=MAX_HOURS).contains(&hours) => Ok(hours),
        _ => invalid(format!(
            "'expires_in_hours' must be an integer from 1 to {MAX_HOURS}"
        )),
    }
}

/// Parse a mint: the body of `POST /api/user/tokens` with
/// `kind: "sandbox_agent"`, or the `mint` object of `POST
/// /api/auth/cli/authorize`. `default_name` names the token when the body
/// does not (the CLI's `--name` is optional); `None` makes `name` required.
pub fn parse(body: &Value, default_name: Option<&str>) -> Result<MintRequest, Invalid> {
    let Value::Object(body) = body else {
        return invalid("the body must be a JSON object");
    };
    if !matches!(body.get("kind"), Some(Value::String(kind)) if kind == KIND) {
        return invalid(format!("'kind' must be \"{KIND}\""));
    }
    if let Some(field) = PERSONAL_FIELDS.iter().find(|f| body.contains_key(**f)) {
        return invalid(format!(
            "'{field}' does not apply to a sandbox agent token: it reaches the sandboxes of \
             the apps it names and nothing else"
        ));
    }
    Ok(MintRequest {
        name: name_of(body, default_name)?,
        apps: apps_of(body)?,
        hours: hours_of(body)?,
    })
}

impl MintRequest {
    /// When a token minted `now` from this request expires.
    pub fn expires_at(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now + Duration::hours(self.hours)
    }

    /// The request as the `mint` of a PKCE code stores it, with the apps
    /// resolved to the ids the session was allowed to mint for. [`parse`]
    /// reads it back.
    pub fn stored(&self, apps: &[Uuid]) -> Value {
        json!({
            "kind": KIND,
            "name": self.name,
            "apps": apps.iter().map(Uuid::to_string).collect::<Vec<_>>(),
            "expires_in_hours": self.hours,
        })
    }
}

/// One app a new token is granted, with the org that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrantedApp {
    pub org_id: Uuid,
    pub app_id: Uuid,
}

/// What to mint.
#[derive(Clone, Debug)]
pub struct NewSandboxToken {
    /// The staff member the token acts as.
    pub minter: Uuid,
    pub name: String,
    pub apps: Vec<GrantedApp>,
    pub expires_at: DateTime<Utc>,
    /// `credential::source::{UI, OXYC}`.
    pub source: &'static str,
}

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// Mint an `oxy_sbx_` token and store its hash and its `app_sandbox` grants.
/// The row is the kind's fixed shape, whatever the caller holds.
pub async fn create<C: ConnectionTrait>(db: &C, new: NewSandboxToken) -> Result<Minted, OxyError> {
    let token = NewToken {
        user_id: new.minter,
        name: new.name,
        all_access: false,
        platform: true,
        partner: false,
        grants: Vec::new(),
        expires_at: Some(new.expires_at),
        source: new.source,
    };
    let minted = personal::create_of_kind(db, StoredKind::SandboxAgent, new.minter, token).await?;
    let now = Utc::now().fixed_offset();
    for app in &new.apps {
        GrantRow::app_sandbox(app.org_id, app.app_id)
            .for_token(minted.row.id, now)
            .insert(db)
            .await
            .map_err(db_err("create sandbox token grant"))?;
    }
    Ok(minted)
}

/// Where each granted app lives **now**: the org that owns it and the
/// workspace it is published from (`apps.org_id`, `apps.project_id` — an app
/// can be re-pointed at another workspace). What admission derives a sandbox
/// agent token's workspace grants from, on every request.
pub async fn app_homes<C: ConnectionTrait>(
    db: &C,
    grants: &[AppSandboxGrant],
) -> Result<Vec<SandboxAppHome>, OxyError> {
    if grants.is_empty() {
        return Ok(Vec::new());
    }
    let rows = Apps::find()
        .filter(apps::Column::Id.is_in(grants.iter().map(|g| g.app_id)))
        .all(db)
        .await
        .map_err(db_err("sandbox token apps lookup"))?;
    Ok(rows
        .into_iter()
        .map(|app| SandboxAppHome {
            app_id: app.id,
            org_id: app.org_id,
            workspace_id: app.project_id,
        })
        .collect())
}

/// Whether a row is a sandbox agent token: fixed at mint, so the routes that
/// edit a token refuse it.
pub fn is_sandbox_agent(row: &api_tokens::Model) -> bool {
    row.kind == StoredKind::SandboxAgent.as_str()
}

/// The ids of the sandbox agent tokens holding a grant — live or revoked — in
/// one of `orgs`. A grant an org already cut off is still that org's history.
async fn tokens_touching<C: ConnectionTrait>(db: &C, orgs: &[Uuid]) -> Result<Vec<Uuid>, OxyError> {
    if orgs.is_empty() {
        return Ok(Vec::new());
    }
    ApiTokenGrants::find()
        .select_only()
        .column(api_token_grants::Column::TokenId)
        .distinct()
        .filter(api_token_grants::Column::Kind.eq(api_token_grants::KIND_APP_SANDBOX))
        .filter(api_token_grants::Column::OrgId.is_in(orgs.to_vec()))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(db_err("sandbox agent token grants lookup"))
}

/// Sandbox agent tokens, newest first, revoked and expired ones included —
/// the staff view. `reach` narrows it to tokens holding a grant in one of
/// those orgs, **in the query**, so the `limit` newest rows are the newest the
/// caller may see; `None` is every token.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    reach: Option<&[Uuid]>,
    limit: u64,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    let mut query = ApiTokens::find()
        .filter(api_tokens::Column::Kind.eq(StoredKind::SandboxAgent.as_str()))
        .order_by_desc(api_tokens::Column::CreatedAt)
        .order_by_desc(api_tokens::Column::Id)
        .limit(limit);
    if let Some(orgs) = reach {
        let ids = tokens_touching(db, orgs).await?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        query = query.filter(api_tokens::Column::Id.is_in(ids));
    }
    query
        .all(db)
        .await
        .map_err(db_err("list sandbox agent tokens"))
}

/// One sandbox agent token by id, whoever minted it. `None` for any other
/// kind, so the staff routes cannot be pointed at a personal token.
pub async fn find<C: ConnectionTrait>(
    db: &C,
    token_id: Uuid,
) -> Result<Option<api_tokens::Model>, OxyError> {
    Ok(ApiTokens::find_by_id(token_id)
        .one(db)
        .await
        .map_err(db_err("sandbox agent token lookup"))?
        .filter(is_sandbox_agent))
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
