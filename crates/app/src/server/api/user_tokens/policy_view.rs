//! `Token.blocked_orgs`: the orgs whose token policy makes each token inert
//! (API-tokens design §5). Decided by the same `oxy_auth::token::policy`
//! functions the request path uses, over the same reach — so the list never
//! disagrees with what a request gets.
//!
//! A fixed number of queries whatever the number of tokens: the owners'
//! memberships (for all-access tokens), the policies of every org reached, and
//! the names of the orgs that block something. A list with no restrictive
//! policy anywhere stops after the second.
//!
//! A legacy key, and a revoked token, are never listed as blocked: a policy
//! never binds the first (§3.5), and the second reaches nothing anyway.

use std::collections::{HashMap, HashSet};

use entity::prelude::Organizations;
use entity::{api_token_grants, api_tokens, organizations};
use oxy_auth::token::policy::{TokenShape, Violation, blocked_in};
use oxy_auth::token::policy_store;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::dto::BlockedOrgDto;
use super::error::TokenError;

/// A row a policy may judge, with what it reaches.
struct Judged {
    token_id: Uuid,
    shape: TokenShape,
    reach: Vec<Uuid>,
}

fn grants_of(token_id: Uuid, grants: &[api_token_grants::Model]) -> Vec<api_token_grants::Model> {
    grants
        .iter()
        .filter(|g| g.token_id == token_id)
        .cloned()
        .collect()
}

/// The judgeable rows, and the all-access owners whose orgs must be read.
fn judgeable(rows: &[api_tokens::Model]) -> (Vec<(&api_tokens::Model, TokenShape)>, Vec<Uuid>) {
    let judged: Vec<(&api_tokens::Model, TokenShape)> = rows
        .iter()
        .filter(|r| r.revoked_at.is_none())
        .filter_map(|r| TokenShape::of(r).filter(|s| !s.legacy).map(|s| (r, s)))
        .collect();
    let owners: HashSet<Uuid> = judged
        .iter()
        .filter(|(_, s)| s.all_access_personal())
        .map(|(r, _)| r.principal_user_id)
        .collect();
    (judged, owners.into_iter().collect())
}

async fn org_names<C: ConnectionTrait>(
    db: &C,
    ids: HashSet<Uuid>,
) -> Result<HashMap<Uuid, String>, TokenError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = Organizations::find()
        .filter(organizations::Column::Id.is_in(ids))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|o| (o.id, o.name)).collect())
}

/// The tokens among `rows` that `org_id`'s own policy makes inert: the same
/// evaluation as [`blocked_orgs`], asked of one org. What the workspace
/// inventory leaves a blocked token out by.
///
/// `grants` need only hold the rows in `org_id`: a grant-bound token is judged
/// in the orgs of its live grants, and only this org's answer is read.
pub(crate) async fn blocked_in_org<C: ConnectionTrait>(
    db: &C,
    rows: &[api_tokens::Model],
    grants: &[api_token_grants::Model],
    org_id: Uuid,
) -> Result<HashSet<Uuid>, TokenError> {
    let blocked = blocked_orgs(db, rows, grants).await?;
    Ok(blocked
        .into_iter()
        .filter(|(_, orgs)| orgs.iter().any(|org| org.org_id == org_id))
        .map(|(token_id, _)| token_id)
        .collect())
}

/// Each token's policy blocks, by token id. A token blocked nowhere is absent.
pub(crate) async fn blocked_orgs<C: ConnectionTrait>(
    db: &C,
    rows: &[api_tokens::Model],
    grants: &[api_token_grants::Model],
) -> Result<HashMap<Uuid, Vec<BlockedOrgDto>>, TokenError> {
    let (judged, owners) = judgeable(rows);
    let memberships = policy_store::member_orgs(db, &owners).await?;
    let judged: Vec<Judged> = judged
        .into_iter()
        .map(|(row, shape)| Judged {
            token_id: row.id,
            reach: if shape.all_access_personal() {
                memberships
                    .get(&row.principal_user_id)
                    .cloned()
                    .unwrap_or_default()
            } else {
                policy_store::grant_orgs(&grants_of(row.id, grants))
            },
            shape,
        })
        .collect();
    let every_org: HashSet<Uuid> = judged.iter().flat_map(|j| j.reach.clone()).collect();
    let every_org: Vec<Uuid> = every_org.into_iter().collect();
    let policies = policy_store::restrictive(db, &every_org).await?;
    if policies.is_empty() {
        return Ok(HashMap::new());
    }
    let blocks: Vec<(Uuid, Vec<(Uuid, Violation)>)> = judged
        .iter()
        .map(|j| (j.token_id, blocked_in(&j.shape, &j.reach, &policies)))
        .filter(|(_, b)| !b.is_empty())
        .collect();
    let names = org_names(
        db,
        blocks
            .iter()
            .flat_map(|(_, b)| b.iter().map(|(o, _)| *o))
            .collect(),
    )
    .await?;
    Ok(blocks
        .into_iter()
        .map(|(token_id, b)| {
            let dtos = b
                .into_iter()
                .map(|(org_id, why)| BlockedOrgDto {
                    org_id,
                    org_name: names.get(&org_id).cloned().unwrap_or_default(),
                    reason: why.as_str().to_string(),
                })
                .collect();
            (token_id, dtos)
        })
        .collect())
}
