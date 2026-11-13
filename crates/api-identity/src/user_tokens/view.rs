//! The reads that turn token rows into [`TokenDto`]s: their grants, the names
//! behind the ids, whether a legacy row's `api_keys` row is still active, and
//! the orgs whose token policy blocks each one ([`policy_view`]).
//! A fixed number of queries per call, whatever the number of tokens.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use entity::prelude::{ApiKeys, Apps, Organizations, Workspaces};
use entity::{api_keys, api_token_grants, api_tokens, apps, organizations, workspaces};
use oxy_auth::token::personal;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::dto::{self, Names, OwnerDto, TokenDto, TokenView};
use super::error::TokenError;
use super::policy_view;

/// The names of the orgs, workspaces and apps these grants name.
pub(crate) async fn names_for<C: ConnectionTrait>(
    db: &C,
    grants: &[api_token_grants::Model],
) -> Result<Names, TokenError> {
    let distinct = |ids: Vec<Uuid>| -> Vec<Uuid> {
        let set: HashSet<Uuid> = ids.into_iter().collect();
        set.into_iter().collect()
    };
    let org_ids = distinct(grants.iter().map(|g| g.org_id).collect());
    let workspace_ids = distinct(grants.iter().filter_map(|g| g.workspace_id).collect());
    let app_ids = distinct(grants.iter().filter_map(|g| g.app_id).collect());

    let mut names = Names::default();
    if !org_ids.is_empty() {
        let rows = Organizations::find()
            .filter(organizations::Column::Id.is_in(org_ids))
            .all(db)
            .await?;
        for org in rows {
            names.org_slugs.insert(org.id, org.slug);
            names.orgs.insert(org.id, org.name);
        }
    }
    if !workspace_ids.is_empty() {
        let rows = Workspaces::find()
            .filter(workspaces::Column::Id.is_in(workspace_ids))
            .all(db)
            .await?;
        names.workspaces = rows.into_iter().map(|w| (w.id, w.name)).collect();
    }
    if !app_ids.is_empty() {
        let rows = Apps::find()
            .filter(apps::Column::Id.is_in(app_ids))
            .all(db)
            .await?;
        for app in rows {
            names.app_slugs.insert(app.id, app.slug);
            names.apps.insert(app.id, app.name);
        }
    }
    Ok(names)
}

/// The legacy rows among `rows` whose `api_keys` row is revoked — which a pod
/// one release back does by writing `api_keys` alone.
pub(crate) async fn inactive_keys<C: ConnectionTrait>(
    db: &C,
    rows: &[api_tokens::Model],
) -> Result<HashSet<Uuid>, TokenError> {
    let key_ids: Vec<Uuid> = rows
        .iter()
        .filter(|r| dto::is_legacy(r))
        .map(dto::legacy_key_id)
        .collect();
    if key_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let inactive = ApiKeys::find()
        .filter(api_keys::Column::Id.is_in(key_ids))
        .filter(api_keys::Column::IsActive.eq(false))
        .all(db)
        .await?;
    Ok(inactive.into_iter().map(|k| k.id).collect())
}

/// The wire form of `rows`, in order. `owners` labels each row's principal.
pub(crate) async fn tokens<C: ConnectionTrait>(
    db: &C,
    rows: &[api_tokens::Model],
    owners: &HashMap<Uuid, String>,
) -> Result<Vec<TokenDto>, TokenError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let grants = personal::grants_for(db, &ids).await?;
    let names = names_for(db, &grants).await?;
    let inactive = inactive_keys(db, rows).await?;
    let mut blocked = policy_view::blocked_orgs(db, rows, &grants).await?;
    let now = Utc::now();
    Ok(rows
        .iter()
        .map(|row| {
            let label = owners
                .get(&row.principal_user_id)
                .cloned()
                .unwrap_or_default();
            let mut token = dto::token_dto(
                row,
                TokenView {
                    grants: &grants,
                    names: &names,
                    owner: OwnerDto::of(row, label),
                    key_inactive: inactive.contains(&dto::legacy_key_id(row)),
                    now,
                },
            );
            token.blocked_orgs = blocked.remove(&row.id).unwrap_or_default();
            token
        })
        .collect())
}

/// One token of `owner`'s, as the wire shows it.
pub(crate) async fn token<C: ConnectionTrait>(
    db: &C,
    row: &api_tokens::Model,
    owner_label: &str,
) -> Result<TokenDto, TokenError> {
    let owners = HashMap::from([(row.principal_user_id, owner_label.to_string())]);
    let mut out = tokens(db, std::slice::from_ref(row), &owners).await?;
    out.pop()
        .ok_or_else(|| TokenError::Internal("a token row mapped to nothing".into()))
}
