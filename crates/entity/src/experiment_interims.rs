//! `experiment_interims` — the latest interim reading of a run still in progress.
//!
//! One row per run (`run_id` is the key), overwritten by each reading: the registered
//! estimator over the days read so far, relabelled `ReportStatus::Interim`. Never the
//! sealed result, never a coefficient patch. `task_id` is the job in flight, cleared when
//! it ends; `error` is why the last attempt produced no reading, and the previous reading
//! (if any) stays beside it.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "experiment_interims")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: Uuid,
    pub task_id: Option<Uuid>,
    pub requested_at: Option<DateTimeWithTimeZone>,
    pub computed_at: Option<DateTimeWithTimeZone>,
    /// The last day read, inclusive.
    pub through: Option<Date>,
    pub report: Option<Json>,
    pub effect: Option<Json>,
    pub compliance: Option<Json>,
    pub series: Option<Json>,
    pub error: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
