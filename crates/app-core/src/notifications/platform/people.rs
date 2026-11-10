//! Who a platform notice is for, by standing in the org or a workspace.
//!
//! Every helper answers with active org **members** only: a platform notice is
//! about the org's own setup (billing, workspaces, runs, monitors), which
//! frontline crew do not hold, and a suspended account is told nothing.
//! Recipient selection, not access control: none of this decides what anyone
//! may do, so it reads roles directly rather than through `oxy-authz`.

use std::collections::HashSet;

use entity::org_members::{self, OrgRole};
use entity::workspace_members::{self, WorkspaceRole};
use entity::{org_frontline_members, users};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

/// The org roles at or above `at_least`.
fn roles_from(at_least: OrgRole) -> Vec<OrgRole> {
    [OrgRole::Owner, OrgRole::Admin, OrgRole::Member]
        .into_iter()
        .filter(|r| *r >= at_least)
        .collect()
}

/// Active members of `org_id` holding `at_least` or a higher role.
pub async fn org_members_from(
    db: &DatabaseConnection,
    org_id: Uuid,
    at_least: OrgRole,
) -> Result<Vec<Uuid>, DbErr> {
    let ids: Vec<Uuid> = org_members::Entity::find()
        .select_only()
        .column(org_members::Column::UserId)
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::Role.is_in(roles_from(at_least)))
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    active(db, ids).await
}

/// The orgs `user_id` belongs to, oldest first: memberships, then frontline
/// standing (crew hold no membership and are who custom-app notifications are
/// for). Not narrowed to what a request's token reaches; the caller does that.
pub async fn orgs_of(db: &DatabaseConnection, user_id: Uuid) -> Result<Vec<Uuid>, DbErr> {
    let members: Vec<Uuid> = org_members::Entity::find()
        .select_only()
        .column(org_members::Column::OrgId)
        .filter(org_members::Column::UserId.eq(user_id))
        .order_by_asc(org_members::Column::CreatedAt)
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    let crew: Vec<Uuid> = org_frontline_members::Entity::find()
        .select_only()
        .column(org_frontline_members::Column::OrgId)
        .filter(org_frontline_members::Column::UserId.eq(user_id))
        .filter(org_frontline_members::Column::Status.eq(org_frontline_members::STATUS_ACTIVE))
        .order_by_asc(org_frontline_members::Column::CreatedAt)
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    Ok(members.into_iter().chain(crew).collect())
}

/// `users` among `ids` whose account is active, in `ids`' order.
pub async fn active(db: &DatabaseConnection, ids: Vec<Uuid>) -> Result<Vec<Uuid>, DbErr> {
    if ids.is_empty() {
        return Ok(ids);
    }
    let live: HashSet<Uuid> = users::Entity::find()
        .select_only()
        .column(users::Column::Id)
        .filter(users::Column::Id.is_in(ids.iter().copied()))
        .filter(users::Column::Status.eq(users::UserStatus::Active))
        .into_tuple::<Uuid>()
        .all(db)
        .await?
        .into_iter()
        .collect();
    Ok(ids.into_iter().filter(|u| live.contains(u)).collect())
}

/// Whether `user_id` is an active member of `org_id`.
pub async fn is_org_member(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<bool, DbErr> {
    let member = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(db)
        .await?
        .is_some();
    Ok(member && !active(db, vec![user_id]).await?.is_empty())
}

/// The workspace's admins: its members holding admin or owner there, or —
/// when it names none — the org's owners and admins, who administer every
/// workspace in it.
pub async fn workspace_admins(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
) -> Result<Vec<Uuid>, DbErr> {
    let named: Vec<Uuid> = workspace_members::Entity::find()
        .select_only()
        .column(workspace_members::Column::UserId)
        .filter(workspace_members::Column::WorkspaceId.eq(workspace_id))
        .filter(workspace_members::Column::Role.is_in([WorkspaceRole::Owner, WorkspaceRole::Admin]))
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    let mut admins = Vec::new();
    for user in named {
        if is_org_member(db, org_id, user).await? {
            admins.push(user);
        }
    }
    if !admins.is_empty() {
        return Ok(admins);
    }
    org_members_from(db, org_id, OrgRole::Admin).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_means_it_and_everything_above_it() {
        assert_eq!(roles_from(OrgRole::Owner), vec![OrgRole::Owner]);
        assert_eq!(
            roles_from(OrgRole::Admin),
            vec![OrgRole::Owner, OrgRole::Admin]
        );
        assert_eq!(roles_from(OrgRole::Member).len(), 3);
    }
}
