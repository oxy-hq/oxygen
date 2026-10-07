use sea_orm::entity::prelude::*;

/// One weekly custom-app usage report. `period_start` is unique: the insert
/// that lands is the report for that week, on every node.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "custom_app_usage_reports")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub period_start: DateTimeWithTimeZone,
    pub period_end: DateTimeWithTimeZone,
    /// Counts per org and app, with their names. Never app content.
    pub snapshot: Json,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
