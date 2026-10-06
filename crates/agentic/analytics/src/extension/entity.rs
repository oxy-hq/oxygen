use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "analytics_run_extensions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: String,
    pub agent_id: String,
    pub spec_hint: Option<Json>,
    pub thinking_mode: Option<String>,
    /// When an attempt began executing the run; `None` until one does. See
    /// `super::execution`.
    pub execution_started_at: Option<DateTimeWithTimeZone>,
    /// The executing attempt's newest proof of life.
    pub execution_heartbeat_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
