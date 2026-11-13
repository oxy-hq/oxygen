//! `oxy seed --org-workspace <slug>`: give one existing org a local workspace.
//!
//! The product no longer makes a workspace without a repository: a new org
//! gets one only from the GitHub import (`internal-docs/factory-retirement.md`
//! phase 6). A single-process dev or CI box still needs one it can edit on
//! disk — the custom-app canary writes the workspace's `config.yml` the way an
//! engineer edits a repository, then compiles — so the seeds make it, as they
//! make the Demo workspace. Local databases only, like the rest of the seed.

use chrono::Utc;
use entity::{organizations, workspaces};
use oxy::adapters::workspace::workspace_root_path;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::seed_compile::SeededWorkspace;

/// The workspace of the org whose slug is `org_slug`: its existing one, or a
/// new Ready `Default` with a minimal `config.yml` under the state dir. Prints
/// the id on stdout, alone, for a script to read.
pub async fn seed_org_workspace(org_slug: &str) -> Result<Uuid, OxyError> {
    super::seed_partners::refuse_if_not_local()?;
    let conn = establish_connection()
        .await
        .map_err(|e| OxyError::DBError(format!("connect: {e}")))?;
    let org = organizations::Entity::find()
        .filter(organizations::Column::Slug.eq(org_slug))
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(format!("find org {org_slug}: {e}")))?
        .ok_or_else(|| OxyError::ArgumentError(format!("no org with slug {org_slug}")))?;
    if let Some(existing) = workspaces::Entity::find()
        .filter(workspaces::Column::OrgId.eq(org.id))
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(format!("find workspace: {e}")))?
    {
        println!("{}", existing.id);
        return Ok(existing.id);
    }

    let id = Uuid::new_v4();
    let dir = workspace_root_path(id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| OxyError::IOError(format!("create {}: {e}", dir.display())))?;
    oxy_project::write_minimal_config_yml(&dir)
        .await
        .map_err(|e| OxyError::IOError(format!("write config.yml: {e}")))?;
    insert_row(&conn, id, org.id, &dir.to_string_lossy()).await?;

    // The tenant the old blank `Default` got at creation, when there is a
    // local Airhouse to give it one.
    super::seed_airhouse::find_local_airhouse().await;
    let target = SeededWorkspace {
        label: format!("{org_slug} / Default"),
        id,
        org_id: Some(org.id),
        path: Some(dir),
    };
    super::seed_airhouse::provision_tenants(&conn, &[target]).await;

    println!("{id}");
    Ok(id)
}

async fn insert_row(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    org_id: Uuid,
    path: &str,
) -> Result<(), OxyError> {
    let now = Utc::now().fixed_offset();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Default".to_string()),
        org_id: ActiveValue::Set(Some(org_id)),
        path: ActiveValue::Set(Some(path.to_string())),
        status: ActiveValue::Set(workspaces::WorkspaceStatus::Ready),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        ..Default::default()
    }
    .insert(conn)
    .await
    .map_err(|e| OxyError::DBError(format!("insert workspace: {e}")))?;
    Ok(())
}
