//! `simulation_run_fits` — β̂ against β_true, per edge per period.
//!
//! This table *is* the convergence chart and the per-edge truth badge; both are
//! queries over it rather than anything recomputed at render time.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "simulation_run_fits")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub period: i32,
    /// `driver -> target`, matching the edge the metric tree declares.
    #[sea_orm(primary_key, auto_increment = false)]
    pub edge: String,
    /// The basis the fitter chose (`linear`, `log-log`, …). A coefficient is
    /// meaningless without it.
    pub form: String,
    /// `None` exactly when the fit was refused. A stored `0.0` would erase the
    /// distinction the whole outcome taxonomy turns on.
    pub coefficient: Option<f64>,
    pub se: Option<f64>,
    pub t_stat: Option<f64>,
    pub n: i32,
    pub n_panels: i32,
    pub refusal: Option<String>,
    /// The true marginal response at the spend the world actually settled at
    /// this period — not at the anchor the curve was calibrated from. Scoring
    /// against the anchor books a modelling difference as estimator bias.
    pub true_local_slope: f64,
    /// `refused` | `converged` | `confidently_wrong`.
    pub outcome: String,
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
