//! The `workspace_previews` registration rows. Plain Postgres, so every function
//! here is safe on any pod.

use entity::workspace_previews::{Column, Entity, Model};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Statement,
};
use uuid::Uuid;

/// Every preview of a workspace, most recently touched first.
pub async fn list<C: ConnectionTrait>(db: &C, workspace_id: Uuid) -> Result<Vec<Model>, DbErr> {
    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_desc(Column::UpdatedAt)
        .all(db)
        .await
}

pub async fn find<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    branch: &str,
) -> Result<Option<Model>, DbErr> {
    Entity::find_by_id((workspace_id, branch.to_string()))
        .one(db)
        .await
}

/// Register the preview if it is new, and record `git_sha` as the commit it is
/// now at. `created_by` is written on insert only: a refresh by someone else
/// does not re-attribute the preview.
pub async fn upsert<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    branch: &str,
    git_sha: &str,
    created_by: Uuid,
) -> Result<Model, DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO workspace_previews (workspace_id, branch, git_sha, created_by) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (workspace_id, branch) DO UPDATE SET \
             git_sha = EXCLUDED.git_sha, updated_at = now()",
        [
            workspace_id.into(),
            branch.into(),
            git_sha.into(),
            created_by.into(),
        ],
    ))
    .await?;
    find(db, workspace_id, branch)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("workspace preview just written".into()))
}

/// Remove the registration row, answering the commit it was at (`None` when
/// there was no such preview). Only the row: releasing its revisions is
/// `previews::revisions::release`, which the service runs in the same
/// transaction.
pub async fn delete<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    branch: &str,
) -> Result<Option<String>, DbErr> {
    let Some(row) = find(db, workspace_id, branch).await? else {
        return Ok(None);
    };
    Entity::delete_by_id((workspace_id, branch.to_string()))
        .exec(db)
        .await?;
    Ok(Some(row.git_sha))
}

/// Whether some live preview of this workspace is at `git_sha` — the one thing
/// that lets the preview pin honour a revision of that commit.
pub async fn previewed_at<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<bool, DbErr> {
    use sea_orm::PaginatorTrait;
    Ok(Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::GitSha.eq(git_sha))
        .count(db)
        .await?
        > 0)
}
