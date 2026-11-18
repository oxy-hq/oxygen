use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One delivery of an event to one listener function. Unique on
/// `(event_id, app_id, function_name)`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_event_deliveries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub event_id: Uuid,
    /// The listener's app.
    pub app_id: Uuid,
    pub function_name: String,
    pub environment: String,
    /// `"pending"` | `"acked"` | `"dead"` | `"held"` | `"discarded"` |
    /// `"expired"`.
    pub status: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub run_id: Option<String>,
    pub invocation_id: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
