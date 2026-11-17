//! `simulation_run_periods` — what the policy did and what it earned, once per
//! decision period. The profit race is a query over this table.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// No `Eq`: the model carries f64 profits and spends.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "simulation_run_periods")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub period: i32,
    pub mean_spend: f64,
    pub realized_profit: f64,
    pub cumulative_profit: f64,
    /// Per-entity spend, so a `machine+explore` run can be asked how much
    /// variation its jitter actually left behind — the question that arm exists
    /// to answer, and one a mean cannot carry.
    pub actions: Json,
    #[sea_orm(
        belongs_to,
        from = "run_id",
        to = "run_id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub simulation_runs: BelongsTo<super::simulation_runs::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
