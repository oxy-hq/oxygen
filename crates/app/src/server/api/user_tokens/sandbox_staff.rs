//! The handlers of `/api/admin/sandbox-agent-tokens` — every staff member's
//! sandbox agent tokens, for the people who operate Oxy (sandbox agent
//! credential design §2, "Revoked by"). Mounted, behind `operate_platform`, by
//! `admin::sandbox_agent_tokens`.
//!
//! **Capabilities gate verbs; scope filters rows.** The section's capability
//! gate decides on the platform singleton, so a grant bounded to a few orgs
//! passes it. The rows are therefore narrowed here, by the one scope read the
//! console has (`admin::scope::list_scope`): a bounded grant sees and revokes
//! only tokens with a grant in an org it reaches, and a token it does not see
//! answers 404, not 403.
//!
//! A revoke is recorded as the staff member, on the chain of every granted
//! app's org, like every other end a token can meet.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::Path;
use entity::prelude::Users;
use entity::{api_token_grants, api_tokens, users};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::{personal, sandbox};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::dto::TokenDto;
use super::error::TokenError;
use super::handlers::{TokenList, token_id};
use super::{service, view};
use crate::server::api::admin::scope;

/// The most tokens one listing returns. A token lives a week at most, so this
/// is far past what is live at once.
const LIST_LIMIT: u64 = 500;
/// `api_tokens.revoke_reason`, and `metadata.reason` on `token.revoked`, for a
/// revoke by staff on either staff list (this one and `standing_staff`).
pub(super) const REVOKED_BY_STAFF: &str = "staff";

/// The orgs the caller's grant is bounded to; `None` when it is not bounded.
/// An unreadable grant is a 500 — never "unbounded".
pub(super) async fn bound(
    db: &DatabaseConnection,
    actor: &RequestActor,
) -> Result<Option<Vec<Uuid>>, TokenError> {
    scope::list_scope(db, &actor.user)
        .await
        .map_err(|status| TokenError::Internal(format!("platform scope unreadable: {status}")))
}

/// Whether a caller bounded to `reach` sees a token holding `grants`. Revoked
/// grants count: a token an org already cut off is still that org's history.
/// A token with no grant left belongs to no org, so only an unbounded caller
/// sees it.
fn sees(reach: Option<&[Uuid]>, grants: &[&api_token_grants::Model]) -> bool {
    match reach {
        None => true,
        Some(orgs) => grants.iter().any(|g| orgs.contains(&g.org_id)),
    }
}

/// The minters' addresses, to label each token's owner.
pub(super) async fn minter_labels(
    db: &DatabaseConnection,
    rows: &[api_tokens::Model],
) -> Result<HashMap<Uuid, String>, TokenError> {
    let ids: HashSet<Uuid> = rows.iter().map(|r| r.principal_user_id).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let users = Users::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await?;
    Ok(users
        .into_iter()
        .map(|u| (u.id, u.email.unwrap_or(u.name)))
        .collect())
}

/// List every sandbox agent token the caller's platform grant reaches
pub async fn list_sandbox_agent_tokens(actor: RequestActor) -> Result<Json<TokenList>, TokenError> {
    let db = establish_connection().await?;
    let reach = bound(&db, &actor).await?;
    // Narrowed in the query, so the newest rows are the newest this caller
    // may see — the same rule `sees` states for one token.
    let visible = sandbox::list(&db, reach.as_deref(), LIST_LIMIT).await?;
    let owners = minter_labels(&db, &visible).await?;
    let tokens: Vec<TokenDto> = view::tokens(&db, &visible, &owners).await?;
    Ok(Json(TokenList { tokens }))
}

/// Revoke a sandbox agent token, whoever minted it
pub async fn revoke_sandbox_agent_token(
    actor: RequestActor,
    Path(id): Path<String>,
) -> Result<Json<TokenDto>, TokenError> {
    let id = token_id(&id)?;
    let db = establish_connection().await?;
    let row = sandbox::find(&db, id).await?.ok_or(TokenError::NotFound)?;
    let grants = personal::grants_for(&db, &[row.id]).await?;
    let own: Vec<&api_token_grants::Model> = grants.iter().collect();
    if !sees(bound(&db, &actor).await?.as_deref(), &own) {
        return Err(TokenError::NotFound);
    }
    // Idempotent: revoking a revoked token changes nothing and records nothing.
    service::revoke(&db, &actor, row, REVOKED_BY_STAFF).await?;
    let revoked = sandbox::find(&db, id).await?.ok_or(TokenError::NotFound)?;
    let owners = minter_labels(&db, std::slice::from_ref(&revoked)).await?;
    let label = owners.get(&revoked.principal_user_id).cloned();
    Ok(Json(
        view::token(&db, &revoked, label.as_deref().unwrap_or_default()).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    const ORG: Uuid = Uuid::from_u128(0xA);
    const ELSEWHERE: Uuid = Uuid::from_u128(0xB);

    fn grant(org_id: Uuid, revoked: bool) -> api_token_grants::Model {
        let now = Utc::now().fixed_offset();
        api_token_grants::Model {
            id: Uuid::new_v4(),
            token_id: Uuid::from_u128(1),
            kind: api_token_grants::KIND_APP_SANDBOX.to_string(),
            org_id,
            workspace_id: None,
            role_ceiling: None,
            app_id: Some(Uuid::from_u128(0xAA)),
            created_at: now,
            revoked_at: revoked.then_some(now),
            revoked_by: None,
        }
    }

    #[test]
    fn an_unbounded_grant_sees_every_token() {
        assert!(sees(None, &[&grant(ORG, false)]));
        // Including one whose every grant is gone with its app.
        assert!(sees(None, &[]));
    }

    #[test]
    fn a_bounded_grant_sees_only_tokens_touching_its_orgs() {
        let here = grant(ORG, false);
        let there = grant(ELSEWHERE, false);
        assert!(sees(Some(&[ORG]), &[&here]));
        assert!(sees(Some(&[ORG]), &[&there, &here]));
        assert!(!sees(Some(&[ORG]), &[&there]));
        // A grant the org already revoked is still its history.
        assert!(sees(Some(&[ORG]), &[&grant(ORG, true)]));
        // No grant left: no org to reach it through.
        assert!(!sees(Some(&[ORG]), &[]));
        // A grant bounded to nothing sees nothing.
        assert!(!sees(Some(&[]), &[&here]));
    }
}
