//! An org ending a personal token's reach into it — the write behind
//! `POST /api/orgs/{org_id}/tokens/{id}/revoke-grant` (API-tokens design §5).
//!
//! **How the block is stored.** A *revoked, org-wide* `api_token_grants` row
//! (`kind = workspace`, `workspace_id IS NULL`, `revoked_at` set) is the block.
//! Admission reads it for every non-legacy token
//! ([`super::credential::blocked_orgs`]) and the reach model refuses the org
//! outright, so:
//!
//! - an **all-access** token, which has no grant row to revoke, is blocked by
//!   inserting one;
//! - a narrowed token's live grants in the org are revoked, and the org-wide
//!   row is added if none of them was org-wide — so a grant its owner adds in
//!   that org later reaches nothing either;
//! - everything the token reaches in other orgs is untouched.
//!
//! No new table and no new column: the owner's token list already shows a
//! grant with `revoked_at`, which is how they see what the org did.
//!
//! **Never a legacy key.** The caller refuses one before reaching here (409
//! `legacy_immutable`); admission would not read the row for one anyway.

use chrono::Utc;
use entity::prelude::ApiTokenGrants;
use entity::{api_token_grants, api_tokens};
use oxy_authz::RoleCeiling;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// What a revoke did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrgRevoke {
    /// The live grants this call revoked.
    pub revoked: Vec<Uuid>,
    /// The org-wide block row this call inserted, if it had to.
    pub inserted_block: Option<Uuid>,
}

impl OrgRevoke {
    /// `false` when the org had already ended this token's reach: nothing was
    /// written, so nothing is audited and nobody is emailed twice.
    pub fn changed(&self) -> bool {
        !self.revoked.is_empty() || self.inserted_block.is_some()
    }
}

/// The ceiling the block row records: what the token held org-wide — no cap
/// for an all-access token, else the highest ceiling among the grants revoked.
/// History for the owner's list; a revoked row caps nothing.
pub fn block_ceiling(all_access: bool, revoked: &[api_token_grants::Model]) -> RoleCeiling {
    if all_access {
        return RoleCeiling::Owner;
    }
    revoked
        .iter()
        .filter_map(|g| g.role_ceiling.as_deref().and_then(RoleCeiling::parse))
        .max()
        .unwrap_or(RoleCeiling::Viewer)
}

fn is_org_wide(grant: &api_token_grants::Model) -> bool {
    grant.kind == api_token_grants::KIND_WORKSPACE && grant.workspace_id.is_none()
}

/// End `token`'s reach into `org_id`. Idempotent. Run it in the transaction
/// that writes the audit row, and invalidate the credential cache after the
/// commit.
pub async fn revoke_org_reach<C: ConnectionTrait>(
    db: &C,
    token: &api_tokens::Model,
    org_id: Uuid,
    by: Uuid,
) -> Result<OrgRevoke, OxyError> {
    let rows = ApiTokenGrants::find()
        .filter(api_token_grants::Column::TokenId.eq(token.id))
        .filter(api_token_grants::Column::OrgId.eq(org_id))
        .all(db)
        .await
        .map_err(db_err("api token grants lookup"))?;
    let has_block_row = rows.iter().any(is_org_wide);
    let live: Vec<api_token_grants::Model> = rows
        .into_iter()
        .filter(|g| g.revoked_at.is_none())
        .collect();
    let ceiling = block_ceiling(token.all_access, &live);
    let now = Utc::now().fixed_offset();

    let mut out = OrgRevoke::default();
    for grant in live {
        out.revoked.push(grant.id);
        let mut active: api_token_grants::ActiveModel = grant.into();
        active.revoked_at = Set(Some(now));
        active.revoked_by = Set(Some(by));
        active
            .update(db)
            .await
            .map_err(db_err("revoke api token grant"))?;
    }
    if !has_block_row {
        let id = Uuid::new_v4();
        api_token_grants::ActiveModel {
            id: Set(id),
            token_id: Set(token.id),
            kind: Set(api_token_grants::KIND_WORKSPACE.to_string()),
            org_id: Set(org_id),
            workspace_id: Set(None),
            role_ceiling: Set(Some(ceiling.as_str().to_string())),
            app_id: Set(None),
            created_at: Set(now),
            revoked_at: Set(Some(now)),
            revoked_by: Set(Some(by)),
        }
        .insert(db)
        .await
        .map_err(db_err("block api token in org"))?;
        out.inserted_block = Some(id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(ceiling: &str) -> api_token_grants::Model {
        api_token_grants::Model {
            id: Uuid::new_v4(),
            token_id: Uuid::from_u128(1),
            kind: api_token_grants::KIND_WORKSPACE.to_string(),
            org_id: Uuid::from_u128(0xA),
            workspace_id: Some(Uuid::from_u128(0xA1)),
            role_ceiling: Some(ceiling.to_string()),
            app_id: None,
            created_at: Utc::now().fixed_offset(),
            revoked_at: None,
            revoked_by: None,
        }
    }

    #[test]
    fn the_block_row_records_what_the_token_held() {
        assert_eq!(block_ceiling(true, &[]), RoleCeiling::Owner);
        assert_eq!(
            block_ceiling(false, &[grant("viewer"), grant("admin")]),
            RoleCeiling::Admin
        );
        // Nothing readable was revoked: the lowest, never a guess upward.
        assert_eq!(block_ceiling(false, &[]), RoleCeiling::Viewer);
    }

    #[test]
    fn a_second_revoke_changes_nothing() {
        assert!(!OrgRevoke::default().changed());
        let first = OrgRevoke {
            revoked: Vec::new(),
            inserted_block: Some(Uuid::new_v4()),
        };
        assert!(first.changed());
    }
}
