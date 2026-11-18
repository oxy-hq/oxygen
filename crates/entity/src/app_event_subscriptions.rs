use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A listener function's subscription to an event type in one environment.
/// Unique on `(environment, app_id, function_name, event_type)`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_event_subscriptions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub environment: String,
    pub app_id: Uuid,
    pub function_name: String,
    pub event_type: String,
    pub build_id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// The circuit breaker paused the subscription at this time: its
    /// deliveries are held until `oxyc events resync`.
    pub paused_at: Option<DateTimeWithTimeZone>,
    pub paused_reason: Option<String>,
    /// When `oxyc events resync` last resumed it: the breaker counts only
    /// deliveries of events published after.
    pub resumed_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
