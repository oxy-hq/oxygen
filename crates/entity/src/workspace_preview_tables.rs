//! `workspace_preview_tables` — a preview's shadow map: which live tables it
//! holds a copy of in its own schemas, so reads resolve to the copy. Names and
//! states only; the copies live in the workspace's Airhouse. See
//! `crates/app/src/server/previews/registry.rs`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_preview_tables")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub preview_key: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub live_schema: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub table_name: String,
    /// `shadow` | `partial` | `sample` | `dropped` (airhouse `ShadowState`).
    pub state: String,
    pub last_run_id: String,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
