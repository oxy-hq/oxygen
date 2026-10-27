//! `experiment_results` — the sealed report of one closed run.
//!
//! One row per run (`run_id` is unique), written once by the `experiment_estimate`
//! job and never updated: a re-read years later renders the same page. `effect`
//! and `ratio` are the engine's own serialisation (infinite bounds are `null`);
//! `report` is the `oxy_experiment::report::ReportView` the page renders;
//! `coefficient_patch` is a JSON-encoded `CoefficientEdit` — **not a diff**,
//! because the compiled views carry no source text — and is `NULL` whenever the
//! write-back was refused, the reason living in `report.write_back`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "experiment_results")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub run_id: Uuid,
    pub effect: Json,
    pub ratio: Option<Json>,
    /// `report.prediction` — the World Model's prediction against the measurement — or `NULL`.
    pub prediction: Option<Json>,
    /// The target measure day by day across the run (`estimate::series::DailySeries`), from
    /// the panel the estimate read; `NULL` on a row sealed before it existed, or when the
    /// target panel was never read.
    pub series: Option<Json>,
    pub compliance: Json,
    pub report: Json,
    pub coefficient_patch: Option<String>,
    pub aborted: bool,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
