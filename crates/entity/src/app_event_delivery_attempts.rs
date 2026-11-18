use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One try of a delivery. Unique on `(delivery_id, attempt)`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_event_delivery_attempts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub delivery_id: Uuid,
    pub attempt: i32,
    pub started_at: DateTimeWithTimeZone,
    pub finished_at: Option<DateTimeWithTimeZone>,
    /// `"ok"` | `"error"` | `"timeout"` | `"crash"` | `"non_retryable"`.
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub invocation_id: Option<Uuid>,
}

impl ActiveModelBehavior for ActiveModel {}
