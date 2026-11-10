//! `oxy.workspace_ready`: a workspace finished setting up.
//!
//! Called where a clone turns a workspace `ready` (the GitHub import in
//! `api-tenancy`'s onboarding). A workspace created ready (a blank one, the
//! demo, an org's default) sends nothing: the person who made it is already
//! looking at it.
//!
//! Told: whoever created it, plus the org's owners and admins. When it is the
//! org's **first** workspace anyone can open, every member: they are the
//! people sitting on the "being set up" screen. "Can open" is what the web app
//! counts (`lastWorkspace.ts`): `ready` or `failed`.

use entity::org_members::OrgRole;
use entity::workspaces::{self, WorkspaceStatus};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait, QueryFilter};
use uuid::Uuid;

use super::kinds::PlatformNotice;
use super::people;
use super::send::{PlatformSent, notify};

/// Tell the people who wait on workspace `workspace_id` that it is ready.
/// `None` when it is not ready, belongs to no org, or is gone.
pub async fn notify_workspace_ready(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Option<PlatformSent>, DbErr> {
    let Some(ws) = workspaces::Entity::find_by_id(workspace_id).one(db).await? else {
        return Ok(None);
    };
    let Some(org_id) = ws.org_id.filter(|_| ws.status == WorkspaceStatus::Ready) else {
        return Ok(None);
    };
    let recipients = if first_navigable(db, org_id, workspace_id).await? {
        people::org_members_from(db, org_id, OrgRole::Member).await?
    } else {
        let mut told = Vec::new();
        if let Some(creator) = ws.created_by
            && people::is_org_member(db, org_id, creator).await?
        {
            told.push(creator);
        }
        told.extend(people::org_members_from(db, org_id, OrgRole::Admin).await?);
        told
    };
    let notice = PlatformNotice::WorkspaceReady {
        org_id,
        workspace_id,
    };
    notify(db, &notice, &recipients).await.map(Some)
}

/// Whether no other workspace in the org can be opened yet.
async fn first_navigable(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
) -> Result<bool, DbErr> {
    let others = workspaces::Entity::find()
        .filter(workspaces::Column::OrgId.eq(org_id))
        .filter(workspaces::Column::Id.ne(workspace_id))
        .filter(workspaces::Column::Id.ne(Uuid::nil()))
        .filter(workspaces::Column::Status.is_in([WorkspaceStatus::Ready, WorkspaceStatus::Failed]))
        .count(db)
        .await?;
    Ok(others == 0)
}
