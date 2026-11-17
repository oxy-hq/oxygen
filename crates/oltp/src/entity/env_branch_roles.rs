//! `oltp_env_branch_roles` — the sealed password of one role ON one app
//! environment's branch.
//!
//! The environment-branch twin of [`super::branch_roles`]: a branch copies every
//! role with its parent's password hash, so Oxy re-mints each role on the branch
//! and seals the new password here, one row per `(env branch, role)`. Which
//! roles exist is still `oltp_roles`' fact.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "oltp_env_branch_roles")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub env_branch_row_id: Uuid,
    /// The role's real (qualified) name, as `oltp_roles.role_name` or the
    /// tenant's analyst name records it.
    pub role_name: String,
    /// AES-GCM-sealed password, valid on this environment branch only.
    pub password_ciphertext: Vec<u8>,
    pub created_at: DateTimeWithTimeZone,
    #[sea_orm(
        belongs_to,
        from = "env_branch_row_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub env_branches: BelongsTo<super::env_branches::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
