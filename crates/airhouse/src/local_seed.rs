//! Local-mode seeding for Airhouse provisioning.
//!
//! Local mode has no organization picker — there's a single implicit org at
//! the nil UUID, with the local guest user as Owner. We seed both rows once
//! at startup so the existing per-org provision flow works untouched.
//!
//! Idempotent: safe to call on every boot. Uses `INSERT ... ON CONFLICT DO
//! NOTHING` semantics by checking existence before inserting.

use chrono::Utc;
use entity::org_members::{self, OrgRole};
use entity::organizations;
use entity::prelude::{OrgMembers, Organizations, Workspaces};
use entity::workspaces::{self, WorkspaceStatus};
use oxy_auth::types::Identity;
use oxy_auth::user::{LOCAL_GUEST_EMAIL, UserService};
use oxy_platform::db::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::config::LOCAL_ORG_ID;

/// Ensure the local guest user, the nil-UUID org, the local-mode workspace,
/// and the Owner membership row all exist. Called once on local-mode startup
/// before the HTTP server accepts requests.
///
/// `local_workspace_id` is the well-known nil-UUID workspace id used by the
/// host's serve-mode (mirrors `LOCAL_ORG_ID`). Passed in so this crate does
/// not depend on the host's serve-mode constants.
pub async fn ensure_local_org_seeded(local_workspace_id: Uuid) -> Result<(), OxyError> {
    // 1. Get-or-create the local guest user.
    let user = UserService::get_or_create_user(&Identity {
        // A seed path: this must be able to mint the row, so no id.
        user_id: None,
        email: LOCAL_GUEST_EMAIL.to_string(),
        name: Some("Local User".to_string()),
        picture: None,
    })
    .await?;

    let conn = establish_connection().await?;

    // 2. Insert the nil-UUID organization if missing.
    if Organizations::find_by_id(LOCAL_ORG_ID)
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(format!("query local org: {e}")))?
        .is_none()
    {
        let now = Utc::now().fixed_offset();
        organizations::ActiveModel {
            id: ActiveValue::Set(LOCAL_ORG_ID),
            name: ActiveValue::Set("Local".to_string()),
            slug: ActiveValue::Set("local".to_string()),
            logo: ActiveValue::NotSet,
            logo_content_type: ActiveValue::NotSet,
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(format!("insert local org: {e}")))?;
        tracing::info!("seeded local-mode organization at nil UUID");
    }

    // 3. Insert the nil-UUID workspace if missing. The airhouse_tenants and
    //    airhouse_users FK constraints reference workspaces(id), so the local
    //    workspace must exist in the DB when Airhouse provision is called.
    if Workspaces::find_by_id(local_workspace_id)
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(format!("query local workspace: {e}")))?
        .is_none()
    {
        let now = Utc::now().fixed_offset();
        workspaces::ActiveModel {
            id: ActiveValue::Set(local_workspace_id),
            name: ActiveValue::Set("Local".to_string()),
            git_namespace_id: ActiveValue::Set(None),
            git_remote_url: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
            path: ActiveValue::Set(None),
            last_opened_at: ActiveValue::Set(None),
            created_by: ActiveValue::Set(None),
            org_id: ActiveValue::Set(Some(LOCAL_ORG_ID)),
            status: ActiveValue::Set(WorkspaceStatus::Ready),
            error: ActiveValue::Set(None),
            monthly_vlm_budget_micros: ActiveValue::Set(None),
            current_revision_id: ActiveValue::Set(None),
            default_branch: ActiveValue::Set(None),
            repo_subdir: ActiveValue::Set(None),
        }
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(format!("insert local workspace: {e}")))?;
        tracing::info!("seeded local-mode workspace at nil UUID");
    }

    // 4. Ensure the local user is an Owner of the local org.
    let membership = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(LOCAL_ORG_ID))
        .filter(org_members::Column::UserId.eq(user.id))
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(format!("query local membership: {e}")))?;
    if membership.is_none() {
        let now = Utc::now().fixed_offset();
        org_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(LOCAL_ORG_ID),
            user_id: ActiveValue::Set(user.id),
            role: ActiveValue::Set(OrgRole::Owner),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(format!("insert local membership: {e}")))?;
        tracing::info!(user_id = %user.id, "seeded local guest user as Owner of local org");
    }

    Ok(())
}
