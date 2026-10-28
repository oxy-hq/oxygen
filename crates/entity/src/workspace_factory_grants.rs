//! `workspace_factory_grants` — which members an Oxy operator let into the Factory
//! (the IDE and back office) of one workspace.
//!
//! The Factory is for Oxy staff, the org's owners and admins, and the people an
//! operator names, per workspace; a plain org membership or a workspace role does
//! not mean it. This row IS the grant: its
//! presence for `(workspace_id, user_id)`, with the user still a member of the
//! workspace's org, is what `oxy_authz::Ring::Factory` reads. Written and removed only
//! by the staff console (`/admin/workspaces/{id}/factory-access/{user_id}`).
//!
//! `granted_by` / `granted_at` are the audit trail, never authority. Foreign keys
//! live in the migration (`m20261026_000001_workspace_factory_grants`): deleting the
//! workspace or the user deletes the grant, and deleting the granting operator only
//! forgets who granted it.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_factory_grants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    /// The operator who granted it. NULL only if that user was deleted.
    pub granted_by: Option<Uuid>,
    pub granted_at: DateTimeWithTimeZone,
    #[sea_orm(
        belongs_to,
        from = "workspace_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub workspaces: BelongsTo<super::workspaces::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
