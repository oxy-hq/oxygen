//! `oltp_branch_roles` — the sealed password of one role ON one branch.
//!
//! A Neon branch copies every role with production's password hash. Oxy
//! re-mints each one on the branch (the analyst and every writer), so a staging
//! credential never opens production and production's never opens staging; the
//! new passwords are sealed here, one row per `(branch, role)`.
//!
//! Which roles exist, and which schema each owns, is still `oltp_roles`' fact —
//! this table only answers "what is that role's password on this branch".
//! Absent entirely on a provider whose branches share the tenant's roles
//! (`LocalProvider`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "oltp_branch_roles")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub branch_row_id: Uuid,
    /// The role's real (qualified) name, as `oltp_roles.role_name` or the
    /// tenant's analyst name records it.
    pub role_name: String,
    /// AES-GCM-sealed password, valid on this branch only.
    pub password_ciphertext: Vec<u8>,
    pub created_at: DateTimeWithTimeZone,
    #[sea_orm(
        belongs_to,
        from = "branch_row_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub branches: BelongsTo<super::branches::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
