use sea_orm::entity::prelude::*;

/// One address's copy of one usage report. The primary key is the claim: a
/// row means the send is under way or done. A failed send deletes its row.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "custom_app_usage_report_deliveries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub report_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub email: String,
    pub claimed_at: DateTimeWithTimeZone,
    /// Set when the provider accepted the message. NULL is a send in flight,
    /// or one whose process stopped before it could say.
    pub sent_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
