use sea_orm::entity::prelude::*;

/// How far copying a workspace's custom-app usage history into its Airhouse
/// has got (`oxy_custom_apps::usage_backfill`). One row per workspace. A row
/// with `finished_at` set makes the Activity tab and the weekly usage report
/// read that workspace's Airhouse (`oxy_custom_apps::usage_reads`). Control
/// plane only: timestamps, a count and a resume position.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "custom_app_usage_backfill")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    pub started_at: DateTimeWithTimeZone,
    pub finished_at: Option<DateTimeWithTimeZone>,
    /// Rows read from Postgres and handed to the sink's idempotent insert.
    pub rows_copied: i64,
    /// `view` or `event`: the table the last row handed over came from.
    pub cursor_kind: Option<String>,
    pub cursor_app_id: Option<Uuid>,
    pub cursor_at: Option<DateTimeWithTimeZone>,
    pub cursor_id: Option<Uuid>,
    pub updated_at: DateTimeWithTimeZone,
    /// Set on a finished workspace when an Airhouse append failed and the
    /// drain wrote the rows to Postgres instead: the oldest such row's time.
    /// A backfill pass copies them over and clears it.
    pub parked_since: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
