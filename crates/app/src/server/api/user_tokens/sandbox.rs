//! Minting a **sandbox agent token** (`oxy_sbx_`), and what the token routes
//! say about one (sandbox agent credential design §2).
//!
//! A staff member mints it for an agent: one to five custom apps, a lifetime in
//! hours. The token then runs the sandbox loop on those apps as its minter.
//!
//! ## Who may mint
//!
//! A caller who holds **both** `develop_apps` and `manage_apps` over the org of
//! every app the body names — what using a sandbox takes today. The standing is
//! read **past the 60 s grant cache** ([`globals::fresh_standing`]), so a grant
//! that was just taken away mints nothing.
//!
//! An app the caller cannot mint for answers 404 `app_not_found` with that
//! app's id, and so does one that does not exist or an id that is not an id: a
//! mint cannot be used to learn which apps exist.
//!
//! The check stops a caller naming apps they cannot reach. It is **not** what
//! holds the token inside its minter's reach — every request the token makes
//! re-reads the minter's live standing.
//!
//! ## Staging
//!
//! A mint that says `"staging": true` is also held to apps whose staging the
//! caller may open (`sandbox_staging::check`), and stores one `app_staging`
//! grant beside each app's `app_sandbox` one. Without it, nothing here moved.
//!
//! ## Fixed once minted
//!
//! Nothing edits one: rename, widen, extend and regenerate answer 409
//! `sandbox_token_fixed` ([`refuse_edit`]). It is listed, read and revoked like
//! the caller's other tokens.

use chrono::{DateTime, Utc};
use entity::prelude::{Apps, Organizations};
use entity::{api_tokens, apps, organizations};
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::StoredKind;
use oxy_auth::token::sandbox::{self as mint, GrantedApp, MintRequest, NewSandboxToken};
use oxy_authz::{Cap, Scope};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::access_audit::Access;
use super::audit::{self, Event, Own};
use super::error::TokenError;
use super::service::{self, Minted};
use super::{policy_cap, sandbox_staging, view};
use crate::server::authz::{self, globals};

/// The two capabilities a sandbox takes, and so a mint for one.
const MINT_CAPS: [Cap; 2] = [Cap::DevelopApps, Cap::ManageApps];

fn may_mint_in(standing: &globals::FreshStanding, org_id: Uuid) -> bool {
    MINT_CAPS.iter().all(|cap| standing.reaches(*cap, org_id))
}

/// The caller's standing, read now. A read that fails is a 500, never "no
/// standing" (which would answer 404 for an app the caller may mint for) and
/// never "any standing".
pub(super) async fn standing_of(
    db: &DatabaseConnection,
    actor: &RequestActor,
) -> Result<globals::FreshStanding, TokenError> {
    globals::fresh_standing(db, &authz::caller_of(actor))
        .await
        .map_err(|e| TokenError::Internal(format!("platform standing lookup: {e}")))
}

/// The apps a mint names, in the order it names them — each one an app the
/// caller may mint for right now, or 404 `app_not_found` naming the first that
/// is not.
async fn mintable(
    db: &DatabaseConnection,
    actor: &RequestActor,
    wanted: &[String],
) -> Result<Vec<apps::Model>, TokenError> {
    let ids: Vec<Uuid> = wanted
        .iter()
        .filter_map(|raw| Uuid::parse_str(raw).ok())
        .collect();
    let found = if ids.is_empty() {
        Vec::new()
    } else {
        Apps::find()
            .filter(apps::Column::Id.is_in(ids))
            .all(db)
            .await?
    };
    let standing = standing_of(db, actor).await?;
    wanted
        .iter()
        .map(|raw| {
            let id = Uuid::parse_str(raw).ok();
            found
                .iter()
                .find(|app| Some(app.id) == id && may_mint_in(&standing, app.org_id))
                .cloned()
                .ok_or_else(|| TokenError::AppNotFound(raw.clone()))
        })
        .collect()
}

/// A mint that has passed every check a session can make: its shape, and who
/// may mint for the apps it names.
pub(super) struct Checked {
    pub request: MintRequest,
    pub apps: Vec<apps::Model>,
}

impl Checked {
    pub(super) fn app_ids(&self) -> Vec<Uuid> {
        self.apps.iter().map(|app| app.id).collect()
    }

    /// Each app with the org that owns it: what the token is granted.
    fn granted(&self) -> Vec<GrantedApp> {
        let granted = |app: &apps::Model| GrantedApp {
            org_id: app.org_id,
            app_id: app.id,
        };
        self.apps.iter().map(granted).collect()
    }
}

/// Parse a mint and check who may mint it. `default_name` names the token when
/// the body does not; `None` makes `name` required.
pub(super) async fn check(
    db: &DatabaseConnection,
    actor: &RequestActor,
    body: &Value,
    default_name: Option<&str>,
) -> Result<Checked, TokenError> {
    let request =
        mint::parse(body, default_name).map_err(|e| TokenError::InvalidSandboxToken(e.0))?;
    let apps = mintable(db, actor, &request.apps).await?;
    if request.staging {
        sandbox_staging::check(db, actor, &apps).await?;
    }
    Ok(Checked { request, apps })
}

/// An org's lifetime cap applies to this token as to any grant-bound one:
/// refused with `exceeds_policy` rather than minted inert. Asked wherever a
/// person can still be told why — the mint, and the CLI's approval — and again
/// at the exchange.
pub(super) async fn within_policy(
    db: &DatabaseConnection,
    checked: &Checked,
    expires_at: DateTime<Utc>,
) -> Result<(), TokenError> {
    let orgs: Vec<Uuid> = checked.apps.iter().map(|app| app.org_id).collect();
    let shape = policy_cap::new_token(StoredKind::SandboxAgent, false);
    policy_cap::check(db, &shape, &orgs, Some(expires_at)).await
}

/// Mint what `checked` asks for, as `actor`, with the lifecycle audit row in
/// the same transaction — written to the chain of every granted app's org,
/// each row naming that org's apps only ([`mint_in`]).
pub(super) async fn create(
    db: &DatabaseConnection,
    actor: &RequestActor,
    checked: &Checked,
    source: &'static str,
    extra: Value,
) -> Result<Minted, TokenError> {
    let expires_at = checked.request.expires_at(Utc::now());
    within_policy(db, checked, expires_at).await?;

    let granted = checked.granted();
    let txn = db.begin().await?;
    let minted = mint::create(
        &txn,
        NewSandboxToken {
            minter: actor.id,
            name: checked.request.name.clone(),
            apps: granted.clone(),
            staging: checked.request.staging,
            expires_at,
            source,
        },
    )
    .await?;
    let stored = oxy_auth::token::personal::grants_for(&txn, &[minted.row.id]).await?;
    let access = Access::of(&minted.row, &stored);
    let mut detail = json!({ "expires_at": audit::rfc3339(minted.row.expires_at) });
    if let (Value::Object(detail), Value::Object(extra)) = (&mut detail, extra) {
        detail.extend(extra);
    }
    Event {
        action: audit::CREATED,
        token: &minted.row,
        orgs: service::reach_of(&txn, &minted.row).await?,
        detail,
        change: None,
    }
    .record_with(&txn, actor, |org| mint_in(&access, &granted, org))
    .await?;
    txn.commit().await?;
    Ok(Minted {
        token: view::token(db, &minted.row, actor.label()).await?,
        secret: minted.secret,
    })
}

/// What the row of `org` says of a mint: the token's access there, and the
/// apps of that org it names. An app of another org is on that org's row only.
fn mint_in(access: &Access<'_>, apps: &[GrantedApp], org: Option<Uuid>) -> Own {
    let here: Vec<Uuid> = apps
        .iter()
        .filter(|app| Some(app.org_id) == org)
        .map(|app| app.app_id)
        .collect();
    let mut detail = access.in_org(org);
    detail["apps"] = json!(here);
    Own::detail(detail)
}

/// `POST /api/user/tokens` with `kind: "sandbox_agent"`.
pub(super) async fn create_from_body(
    db: &DatabaseConnection,
    actor: &RequestActor,
    body: &Value,
) -> Result<Minted, TokenError> {
    let checked = check(db, actor, body, None).await?;
    let source = oxy_auth::token::credential::source::UI;
    create(db, actor, &checked, source, json!({})).await
}

/// 409 `sandbox_token_fixed` for a sandbox agent token: nothing edits one.
pub(super) fn refuse_edit(row: &api_tokens::Model) -> Result<(), TokenError> {
    if mint::is_sandbox_agent(row) {
        return Err(TokenError::SandboxTokenFixed);
    }
    Ok(())
}

/// The limits a mint is held to, for the create dialog.
#[derive(Debug, PartialEq, Serialize)]
pub struct SandboxAgentLimits {
    pub default_hours: i64,
    pub max_hours: i64,
    pub max_apps: usize,
}

impl SandboxAgentLimits {
    pub(super) fn current() -> Self {
        Self {
            default_hours: mint::DEFAULT_HOURS,
            max_hours: mint::MAX_HOURS,
            max_apps: mint::MAX_APPS,
        }
    }
}

/// One app the caller may mint a sandbox agent token for.
#[derive(Debug, PartialEq, Serialize)]
pub struct SandboxAppOption {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_slug: String,
    pub org_name: String,
    pub slug: String,
    pub name: String,
}

/// The app with its org's names, or nothing for an app whose org is gone.
pub(super) fn option_of(
    app: &apps::Model,
    orgs: &[organizations::Model],
) -> Option<SandboxAppOption> {
    let org = orgs.iter().find(|org| org.id == app.org_id)?;
    Some(SandboxAppOption {
        id: app.id,
        org_id: app.org_id,
        org_slug: org.slug.clone(),
        org_name: org.name.clone(),
        slug: app.slug.clone(),
        name: app.name.clone(),
    })
}

pub(super) async fn orgs_of<C: ConnectionTrait>(
    db: &C,
    apps: &[apps::Model],
) -> Result<Vec<organizations::Model>, TokenError> {
    let mut ids: Vec<Uuid> = apps.iter().map(|app| app.org_id).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(Organizations::find()
        .filter(organizations::Column::Id.is_in(ids))
        .all(db)
        .await?)
}

/// The apps the caller may mint for: `[]` for anyone who is not staff. Read
/// with the same fresh standing the mint itself checks, so the dialog never
/// offers an app the mint would then refuse.
pub(super) async fn mintable_options(
    db: &DatabaseConnection,
    actor: &RequestActor,
) -> Result<Vec<SandboxAppOption>, TokenError> {
    let standing = standing_of(db, actor).await?;
    let candidates = match standing.scope() {
        Scope::Orgs(orgs) if orgs.is_empty() => return Ok(Vec::new()),
        Scope::Orgs(orgs) => Apps::find().filter(apps::Column::OrgId.is_in(orgs)),
        Scope::All => Apps::find(),
    };
    let apps: Vec<apps::Model> = candidates
        .order_by_asc(apps::Column::Slug)
        .all(db)
        .await?
        .into_iter()
        .filter(|app| may_mint_in(&standing, app.org_id))
        .collect();
    let orgs = orgs_of(db, &apps).await?;
    let mut options: Vec<SandboxAppOption> = apps
        .iter()
        .filter_map(|app| option_of(app, &orgs))
        .collect();
    options.sort_by(|a, b| (&a.org_slug, &a.slug).cmp(&(&b.org_slug, &b.slug)));
    Ok(options)
}

pub(super) use super::sandbox_describe::describe;
pub use super::sandbox_describe::{GrantedAppDto, MinterDto, SelfDescription};

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
