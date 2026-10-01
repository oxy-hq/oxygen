//! `workspace_previews` — which branches Oxy staff are previewing. A preview's
//! compile state lives on its staging revision in `revisions`; this row is only
//! the registration. See `crates/app/src/server/previews/mod.rs`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_previews")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub branch: String,
    /// The commit the latest create/refresh asked for.
    pub git_sha: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
