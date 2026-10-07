//! What each `/api/user/tokens` route does: the checks, the write, and the
//! lifecycle audit rows — in one transaction, so a token never changes without
//! the row that says who changed it. The credential cache is invalidated after
//! the commit, which bounds how long another pod honours the old state.
//!
//! These routes serve the tokens a person owns directly: their **personal
//! access tokens**, and the **sandbox agent tokens** they minted — which are
//! listed, read and revoked here and never edited (`sandbox.rs`). An **agent
//! token** is a personal token that is fixed in the same way (`agent.rs`). A
//! legacy API key is not a token: its id is a 404 here, and it is managed
//! through the legacy routes (`/api/{workspace_id}/api-keys`). The one caller
//! that still hands a legacy row to [`revoke`] is refused before it gets here
//! (`introspect.rs`).

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};
use entity::{api_token_grants, api_tokens};
use oxy_app_core::audit::RequestActor;
use oxy_auth::ExtendTo;
use oxy_auth::token::StoredKind;
use oxy_auth::token::access::{
    Access as AskedAccess, CreateBody, GrantWant, GrantsEdit, PatchBody,
};
use oxy_auth::token::credential::{blocked_orgs, source};
use oxy_auth::token::grant_plan::{self, GrantPlan};
use oxy_auth::token::personal::{self, GrantSpec, NewToken, Settings};
use sea_orm::{ConnectionTrait, DatabaseConnection, TransactionTrait};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::access_audit::{self, Access};
use super::audit::{self, Event};
use super::dto::TokenDto;
use super::error::TokenError;
use super::{agent, policy_cap, reach, sandbox, view};
use crate::server::api::api_keys::activity::{ActivityResponse, last_used_at, token_activity};
use crate::server::authz::{self, PrincipalFacts};

/// A token and the secret that is shown exactly once.
pub(super) struct Minted {
    pub token: TokenDto,
    pub secret: String,
}

/// The caller's own token, or 404 — someone else's reads the same as none.
pub(super) async fn owned(
    db: &DatabaseConnection,
    id: Uuid,
    user_id: Uuid,
) -> Result<api_tokens::Model, TokenError> {
    personal::find_owned(db, id, user_id)
        .await?
        .ok_or(TokenError::NotFound)
}

pub(super) fn invalidate(token_id: Uuid) {
    oxy_auth::token::cache::invalidate_token(token_id);
}

/// The orgs a token reaches right now — where its lifecycle events go.
pub(super) async fn reach_of<C: ConnectionTrait>(
    db: &C,
    token: &api_tokens::Model,
) -> Result<Vec<Uuid>, TokenError> {
    let grants = personal::grants_for(db, &[token.id]).await?;
    if token.all_access {
        // Everywhere its owner belongs — less the orgs that ended its reach.
        let blocked = blocked_orgs(&grants);
        let mut orgs = audit::owner_orgs(db, token.principal_user_id).await?;
        orgs.retain(|org| !blocked.contains(org));
        return Ok(audit::reach_orgs(true, &orgs, &[]));
    }
    Ok(audit::reach_orgs(false, &[], &grants))
}

/// The grant rows an edit needs. An `app_publish` grant the token does not
/// hold cannot be asked for here.
fn plan(
    existing: &[api_token_grants::Model],
    wanted: &[GrantWant],
) -> Result<GrantPlan, TokenError> {
    grant_plan::replace(existing, wanted).map_err(|not_held| {
        TokenError::Invalid(format!(
            "the token holds no app_publish grant on app {}: one cannot be added here",
            not_held.app_id
        ))
    })
}

pub(super) async fn list(
    db: &DatabaseConnection,
    actor: &RequestActor,
) -> Result<Vec<TokenDto>, TokenError> {
    let rows = personal::list_owned(db, actor.id).await?;
    let owners = HashMap::from([(actor.id, actor.label().to_string())]);
    view::tokens(db, &rows, &owners).await
}

pub(super) async fn get(
    db: &DatabaseConnection,
    actor: &RequestActor,
    id: Uuid,
) -> Result<TokenDto, TokenError> {
    let row = owned(db, id, actor.id).await?;
    view::token(db, &row, actor.label()).await
}

/// A personal token about to be minted: what `POST /api/user/tokens` read from
/// its body, or what an approved agent mint asks for (`agent.rs`).
pub(super) struct NewPersonal {
    pub name: String,
    pub access: AskedAccess,
    pub expires_at: Option<DateTime<Utc>>,
    /// `credential::source::{UI, OXYC_AGENT}`.
    pub source: &'static str,
    /// Added to the `token.created` rows: where the token was minted from.
    pub detail: Map<String, Value>,
}

/// Every check a new personal token passes before it is minted: the standing
/// its flags need, the orgs and workspaces its grants name, and each org's
/// lifetime cap. **One function for every way a personal token is made**, so
/// what limits one limits all. Returns the grant rows to write.
pub(super) async fn admit_personal(
    db: &DatabaseConnection,
    facts: &PrincipalFacts,
    new: &NewPersonal,
) -> Result<Vec<GrantSpec>, TokenError> {
    reach::require_standing(facts, new.access.platform, new.access.partner)?;
    let grants = plan(&[], &new.access.grants)?.insert;
    reach::check_grants(db, facts, &grants).await?;
    let orgs: Vec<Uuid> = grants.iter().map(|g| g.org_id).collect();
    let shape = policy_cap::new_token(StoredKind::Personal, new.access.all_access);
    policy_cap::check(db, &shape, &orgs, new.expires_at).await?;
    Ok(grants)
}

/// [`admit_personal`], then the mint and its lifecycle audit rows in one
/// transaction.
pub(super) async fn mint_personal(
    db: &DatabaseConnection,
    actor: &RequestActor,
    facts: &PrincipalFacts,
    new: NewPersonal,
) -> Result<Minted, TokenError> {
    let grants = admit_personal(db, facts, &new).await?;
    let txn = db.begin().await?;
    let token = NewToken {
        user_id: actor.id,
        name: new.name,
        all_access: new.access.all_access,
        platform: new.access.platform,
        partner: new.access.partner,
        grants,
        expires_at: new.expires_at,
        source: new.source,
    };
    let minted = personal::create(&txn, token).await?;
    let stored = personal::grants_for(&txn, &[minted.row.id]).await?;
    let access = Access::of(&minted.row, &stored);
    let mut detail = new.detail;
    detail.insert("expires_at".into(), audit::rfc3339(minted.row.expires_at));
    Event {
        action: audit::CREATED,
        token: &minted.row,
        orgs: reach_of(&txn, &minted.row).await?,
        detail: Value::Object(detail),
        change: None,
    }
    .record_with(&txn, actor, access_audit::created(&access))
    .await?;
    txn.commit().await?;
    Ok(Minted {
        token: view::token(db, &minted.row, actor.label()).await?,
        secret: minted.secret,
    })
}

pub(super) async fn create(
    db: &DatabaseConnection,
    actor: &RequestActor,
    body: CreateBody,
) -> Result<Minted, TokenError> {
    let new = NewPersonal {
        name: body.name()?,
        access: body.access()?,
        expires_at: body.expires_at(Utc::now())?,
        source: source::UI,
        detail: Map::new(),
    };
    let facts = reach::facts(db, &authz::caller_of(actor)).await?;
    mint_personal(db, actor, &facts, new).await
}

/// 409 for a token that is fixed at mint — a sandbox agent token, an agent
/// token: nothing edits, extends or regenerates one.
fn refuse_fixed(row: &api_tokens::Model) -> Result<(), TokenError> {
    sandbox::refuse_edit(row)?;
    agent::refuse_edit(row)
}

pub(super) async fn patch(
    db: &DatabaseConnection,
    actor: &RequestActor,
    id: Uuid,
    body: PatchBody,
) -> Result<TokenDto, TokenError> {
    let row = owned(db, id, actor.id).await?;
    refuse_fixed(&row)?;
    if row.revoked_at.is_some() {
        return Err(TokenError::Revoked);
    }
    let edit = body.edit(&Settings::of(&row))?;
    let existing = personal::grants_for(db, &[row.id]).await?;
    let plan = match &edit.grants {
        GrantsEdit::Keep => GrantPlan::default(),
        GrantsEdit::Clear => grant_plan::clear(&existing),
        GrantsEdit::Replace(wanted) => {
            let plan = plan(&existing, wanted)?;
            if plan.live_after() == 0 {
                return Err(TokenError::Invalid(
                    "none of these grants can be given: their organization revoked them".into(),
                ));
            }
            plan
        }
    };
    // Only what the edit adds is checked: a grant the token already holds is
    // kept as it is, and a standing only when the body asks for it.
    if edit.asks_platform || edit.asks_partner || !plan.insert.is_empty() {
        let facts = reach::facts(db, &authz::caller_of(actor)).await?;
        reach::require_standing(&facts, edit.asks_platform, edit.asks_partner)?;
        reach::check_grants(db, &facts, &plan.insert).await?;
    }
    apply_edit(db, actor, row, &existing, edit.settings, &plan).await
}

fn same_access(a: &Settings, b: &Settings) -> bool {
    (a.all_access, a.platform, a.partner) == (b.all_access, b.platform, b.partner)
}

async fn apply_edit(
    db: &DatabaseConnection,
    actor: &RequestActor,
    row: api_tokens::Model,
    existing: &[api_token_grants::Model],
    settings: Settings,
    plan: &GrantPlan,
) -> Result<TokenDto, TokenError> {
    let stored = Settings::of(&row);
    let access_changed = !plan.changes_nothing() || !same_access(&stored, &settings);
    if !access_changed && stored.name == settings.name {
        return view::token(db, &row, actor.label()).await;
    }
    let before = Access::of(&row, existing);
    let txn = db.begin().await?;
    let reached_before = reach_of(&txn, &row).await?;
    let updated = personal::update_settings(&txn, row, settings).await?;
    personal::apply_grant_plan(&txn, updated.id, plan).await?;
    // A rename alone changes no reach, so it records no lifecycle event.
    if access_changed {
        let grants = personal::grants_for(&txn, &[updated.id]).await?;
        let after = Access::of(&updated, &grants);
        Event {
            action: audit::GRANTS_CHANGED,
            token: &updated,
            orgs: audit::union(reached_before, reach_of(&txn, &updated).await?),
            detail: json!({}),
            change: None,
        }
        .record_with(&txn, actor, access_audit::changed(&before, &after))
        .await?;
    }
    txn.commit().await?;
    invalidate(updated.id);
    view::token(db, &updated, actor.label()).await
}

fn expiry_change(
    old: Option<DateTime<FixedOffset>>,
    new: Option<DateTime<FixedOffset>>,
) -> (Value, Option<(Value, Value)>) {
    let (old, new) = (audit::rfc3339(old), audit::rfc3339(new));
    (
        json!({ "old_expires_at": old, "new_expires_at": new }),
        Some((json!({ "expires_at": old }), json!({ "expires_at": new }))),
    )
}

/// Push out the expiry. Revives an expired token; never a revoked one.
pub(super) async fn extend(
    db: &DatabaseConnection,
    actor: &RequestActor,
    id: Uuid,
    target: ExtendTo,
) -> Result<TokenDto, TokenError> {
    let row = owned(db, id, actor.id).await?;
    refuse_fixed(&row)?;
    let previous = row.expires_at;
    let txn = db.begin().await?;
    if row.revoked_at.is_some() {
        return Err(TokenError::Revoked);
    }
    let at = target
        .resolve(previous.map(Into::into), Utc::now())
        .map_err(TokenError::Invalid)?;
    policy_cap::check_row(&txn, &row, at).await?;
    let updated = personal::set_expiry(&txn, row, at).await?;
    let (detail, change) = expiry_change(previous, updated.expires_at);
    Event {
        action: audit::EXTENDED,
        token: &updated,
        orgs: reach_of(&txn, &updated).await?,
        detail,
        change,
    }
    .record(&txn, actor)
    .await?;
    txn.commit().await?;
    invalidate(updated.id);
    view::token(db, &updated, actor.label()).await
}

/// A new secret for the same token: same id, grants and expiry. The old
/// secret dies with the commit.
pub(super) async fn regenerate(
    db: &DatabaseConnection,
    actor: &RequestActor,
    id: Uuid,
) -> Result<Minted, TokenError> {
    let row = owned(db, id, actor.id).await?;
    refuse_fixed(&row)?;
    if row.revoked_at.is_some() {
        return Err(TokenError::Revoked);
    }
    // Same expiry, so this refuses only a token that is already past the cap:
    // a new secret for it would be inert where it is capped.
    policy_cap::check_row(db, &row, row.expires_at.map(Into::into)).await?;
    let txn = db.begin().await?;
    let minted = personal::regenerate(&txn, row).await?;
    Event {
        action: audit::REGENERATED,
        token: &minted.row,
        orgs: reach_of(&txn, &minted.row).await?,
        detail: json!({}),
        change: None,
    }
    .record(&txn, actor)
    .await?;
    txn.commit().await?;
    invalidate(minted.row.id);
    Ok(Minted {
        token: view::token(db, &minted.row, actor.label()).await?,
        secret: minted.secret,
    })
}

/// Revoke `row`. Idempotent: an already-revoked token changes nothing and
/// records nothing. `reason` is `owner`, or `self` when the token revoked
/// itself.
pub(super) async fn revoke(
    db: &DatabaseConnection,
    actor: &RequestActor,
    row: api_tokens::Model,
    reason: &'static str,
) -> Result<(), TokenError> {
    let token_id = row.id;
    let txn = db.begin().await?;
    let revoked = personal::revoke(&txn, row, actor.id, reason).await?;
    if let Some(token) = &revoked {
        Event {
            action: audit::REVOKED,
            token,
            orgs: reach_of(&txn, token).await?,
            detail: json!({ "reason": reason }),
            change: None,
        }
        .record(&txn, actor)
        .await?;
    }
    txn.commit().await?;
    invalidate(token_id);
    Ok(())
}

pub(super) async fn activity(
    db: &DatabaseConnection,
    actor: &RequestActor,
    id: Uuid,
    limit: u64,
) -> Result<ActivityResponse, TokenError> {
    let row = owned(db, id, actor.id).await?;
    let fallback = row.last_used_at.map(|at| last_used_at(at.into()));
    Ok(token_activity(db, row.id, fallback, limit).await?)
}
