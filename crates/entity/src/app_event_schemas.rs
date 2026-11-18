use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One revision of the schema of an app's event type. Unique on
/// `(app_id, event_name, revision)` and on `(app_id, event_name, sha256)`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_event_schemas")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub app_id: Uuid,
    pub event_name: String,
    pub revision: i32,
    pub schema: Json,
    pub sha256: String,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
