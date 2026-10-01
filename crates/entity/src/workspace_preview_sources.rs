//! `workspace_preview_sources` — per-pipeline sandbox overrides for an Airway
//! sample run of a rotate-on-use source (QuickBooks runs against the
//! customer's sandbox company). Variable names and realm ids, never a secret;
//! at most one pipeline per workspace rotates a given refresh-token variable.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_preview_sources")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub pipeline_name: String,
    /// Always `sandbox`.
    pub environment: String,
    pub overrides: Json,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
