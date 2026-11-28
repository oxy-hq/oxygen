//! Organization → API tokens → **Apps**: the platform actions the org's custom
//! apps ask for, and the org admin's approval of them
//! (`internal-docs/proposals/app-platform-access.md`). Named `oxygen-access` in
//! code and URLs, because Organization → App access is another feature (which
//! teams reach which apps).
//!
//! - `GET  /api/orgs/{org_id}/oxygen-access` — every app of the org whose live
//!   build asks for an action, or that holds an approval, with what is pending;
//! - `GET  /api/orgs/{org_id}/oxygen-access/{app_id}` — one app's, for the
//!   approval page;
//! - `POST /api/orgs/{org_id}/oxygen-access/{app_id}/approve` — approve actions the
//!   live build asks for (session-only);
//! - `POST /api/orgs/{org_id}/oxygen-access/{app_id}/revoke` — end the app's
//!   access (session-only).
//!
//! **What is asked for is read from the live build's manifest**, the build
//! production's functions run (`EnvironmentBuilds::production_runs`), never from
//! a request body or a bundle someone hands in. An approval can only name an
//! action that build declares, and approves it on what the build lists (an
//! upload's pipelines, by name). A later build asking for more shows the new
//! actions or pipelines as pending; what was approved keeps working.
//!
//! **Every change is a token lifecycle event of the app's row**, so the
//! inventory's Activity drawer shows it: `token.created` when an approval makes
//! the row, `token.grants_changed` when it adds actions, `token.revoked` on a
//! revoke. Making the app's account is `service_account.created`, as for any.

//!
//! The page is read by [`load`] in a fixed number of queries for any number
//! of apps, and each app's statuses are computed by [`dto::actions_of`], a pure
//! function.

mod dto;
mod load;
#[cfg(test)]
mod tests;

pub use dto::{AppAccessDto, AppAccessList, AppActionDto, ApproveBody, LiveBuildDto, TargetDto};

use std::collections::{BTreeMap, BTreeSet};

use entity::apps;
use entity::prelude::Apps;
use oxy_app_core::audit::{self, AuditEntry, RequestActor, TxnAuditSink};
use oxy_auth::token::app_access::{self as store, Approved, ApprovedAction};
use oxy_authz::Action;
use oxy_custom_apps::platform_access::{Asked, declared};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::audit as account_audit;
use crate::user_tokens::audit::{self as token_audit, Event};
use crate::user_tokens::error::TokenError;

/// One app's access, read as the list reads it.
async fn dto(db: &DatabaseConnection, app: &apps::Model) -> Result<AppAccessDto, TokenError> {
    Ok(load::load(db, std::slice::from_ref(app)).await?.dto(app))
}

/// Every app of the org that asks for an action or holds an approval.
pub(super) async fn list(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<AppAccessList, TokenError> {
    let rows = Apps::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .order_by_asc(apps::Column::Name)
        .all(db)
        .await?;
    let loaded = load::load(db, &rows).await?;
    let apps = rows
        .iter()
        .map(|app| loaded.dto(app))
        .filter(|row| !row.actions.is_empty() || row.token_id.is_some())
        .collect();
    Ok(AppAccessList { apps })
}

/// One app's access in this org, or 404 for an app of another org.
pub(super) async fn get(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_id: Uuid,
) -> Result<AppAccessDto, TokenError> {
    let app = app_in_org(db, org_id, app_id).await?;
    dto(db, &app).await
}

async fn app_in_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_id: Uuid,
) -> Result<apps::Model, TokenError> {
    Apps::find_by_id(app_id)
        .one(db)
        .await?
        .filter(|app| app.org_id == org_id)
        .ok_or(TokenError::NotFound)
}

/// What an approval body approves, checked against what the live build asks
/// for: each named action on everything the build lists for it (an upload's
/// pipelines; the whole org for kiosks). Absent or empty names every action.
pub(super) fn requested(
    body: &ApproveBody,
    asked: &BTreeMap<Action, Asked>,
) -> Result<Vec<ApprovedAction>, TokenError> {
    let actions: Vec<Action> = match body.actions.as_deref() {
        None | Some([]) => asked.keys().copied().collect(),
        Some(ids) => ids
            .iter()
            .map(|id| match Action::from_app_action_id(id) {
                Some(action) if asked.contains_key(&action) => Ok(action),
                _ => Err(TokenError::NotDeclared(id.clone())),
            })
            .collect::<Result<_, _>>()?,
    };
    let mut out = Vec::new();
    for action in actions {
        let targets = &asked[&action].targets;
        if targets.is_empty() {
            out.push(ApprovedAction {
                action,
                target: None,
            });
        }
        out.extend(targets.iter().map(|t| ApprovedAction {
            action,
            target: Some(t.clone()),
        }));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Approve actions the live build asks for. One transaction: the account, the
/// row, the approvals and their audit rows commit together.
pub(super) async fn approve(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org_id: Uuid,
    app_id: Uuid,
    body: ApproveBody,
) -> Result<AppAccessDto, TokenError> {
    let app = app_in_org(db, org_id, app_id).await?;
    let build = load::live_build(db, &app).await?;
    let asked = declared(build.as_ref().and_then(|b| b.manifest_json.as_ref()));
    if asked.is_empty() {
        return Err(TokenError::Invalid(
            "the app's live build asks for no platform action".into(),
        ));
    }
    let wanted = requested(&body, &asked)?;

    let txn = db.begin().await?;
    let had_token = match store::account_of(&txn, app.id).await? {
        Some(account) => store::live_token(&txn, account.user_id).await?.is_some(),
        None => false,
    };
    let approved = store::approve(&txn, &app, &wanted, actor.id).await?;
    record_approval(&txn, actor, &app, &approved, had_token).await?;
    txn.commit().await?;
    dto(db, &app).await
}

/// The approval's audit rows, in its transaction: `service_account.created`
/// for the app's first approval, and `token.created` (a new live row) or
/// `token.grants_changed` (more approved on the existing one) when anything
/// was added.
async fn record_approval(
    txn: &sea_orm::DatabaseTransaction,
    actor: &RequestActor,
    app: &apps::Model,
    approved: &Approved,
    had_token: bool,
) -> Result<(), TokenError> {
    let Approved {
        account,
        token,
        added,
        account_created,
    } = approved;
    if *account_created {
        let entry = AuditEntry::for_request(actor, account_audit::CREATED)
            .org(account.org_id)
            .target(
                account_audit::TARGET_TYPE,
                account.user_id.to_string(),
                account.name.clone(),
            )
            .metadata(json!({
                "name": account.name,
                "org_role": account.org_role,
                "app_id": app.id,
                "app_slug": app.slug,
            }));
        audit::record_in_txn(txn, entry).await?;
    }
    if !added.is_empty() {
        let action = if had_token {
            token_audit::GRANTS_CHANGED
        } else {
            token_audit::CREATED
        };
        Event {
            action,
            token,
            orgs: vec![app.org_id],
            detail: approval_detail(app, &account.name, added),
            change: None,
        }
        .record(&TxnAuditSink::request(txn, actor))
        .await?;
    }
    Ok(())
}

fn approval_detail(app: &apps::Model, account: &str, added: &[ApprovedAction]) -> Value {
    let actions: BTreeSet<&str> = added
        .iter()
        .filter_map(|a| a.action.app_action_id())
        .collect();
    let pipelines: BTreeSet<&str> = added.iter().filter_map(|a| a.target.as_deref()).collect();
    json!({
        "app_id": app.id,
        "app_slug": app.slug,
        "service_account": account,
        "approved_actions": actions,
        "approved_pipelines": pipelines,
    })
}

/// End the app's access. 404 for an app with nothing live to end.
pub(super) async fn revoke(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org_id: Uuid,
    app_id: Uuid,
) -> Result<AppAccessDto, TokenError> {
    let app = app_in_org(db, org_id, app_id).await?;
    let txn = db.begin().await?;
    let Some(revoked) = store::revoke(&txn, app.id, actor.id).await? else {
        return Err(TokenError::NoToken);
    };
    Event {
        action: token_audit::REVOKED,
        token: &revoked,
        orgs: vec![app.org_id],
        detail: json!({
            "app_id": app.id,
            "app_slug": app.slug,
            "reason": store::REVOKED_BY_ORG,
        }),
        change: None,
    }
    .record(&TxnAuditSink::request(&txn, actor))
    .await?;
    txn.commit().await?;
    dto(db, &app).await
}
