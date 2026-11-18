use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Where the table tracker is in one watched table: the last DuckLake
/// snapshot delivered, per workspace and environment. Unique on
/// `(workspace_id, environment, table_ref)`. `table_ref` is the table as the
/// watching function wrote it (`pokehouse:bookkeeping.lines`).
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_event_table_cursors")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub environment: String,
    pub table_ref: String,
    /// The last snapshot delivered; `None` until the first sweep places it.
    pub snapshot_id: Option<i64>,
    /// The newest snapshot the last sweep saw.
    pub head_snapshot: Option<i64>,
    /// The schema the last sweep read the table in (the environment's own
    /// copy, or production's).
    pub watched_schema: Option<String>,
    /// Column name to type, as the last sweep read them: the contract.
    pub table_columns: Option<Json>,
    /// The catalog id of the table when the contract was read: a `CREATE OR
    /// REPLACE` or a drop and create gives the name a new one.
    pub table_id: Option<i64>,
    /// `active`, `behind_horizon` or `broken`.
    pub state: String,
    pub state_detail: Option<String>,
    pub checked_at: Option<DateTimeWithTimeZone>,
    pub advanced_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
