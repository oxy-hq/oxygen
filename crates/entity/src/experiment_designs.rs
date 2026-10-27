//! `experiment_designs` — an experiment's one design: the whole
//! `oxy_experiment::ExperimentConfig`, edited in place while a draft and frozen
//! by Register. `frozen_at` is the server's registration instant; a BEFORE
//! UPDATE OR DELETE trigger refuses any change to a frozen row, so the design a
//! run is pinned to never moves under it.
//!
//! `from_proposal_id` is the proposal whose output keys were last written onto
//! the draft. `content_hash` is the sha256 of the canonical (key-sorted) JSON of
//! `config`: an editor's optimistic-concurrency token, and what a proposal
//! remembers it priced.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "experiment_designs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub experiment_id: Uuid,
    pub from_proposal_id: Option<Uuid>,
    pub config: Json,
    pub content_hash: String,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub updated_by: Uuid,
    pub updated_at: DateTimeWithTimeZone,
    /// `None` while a draft; set once, by Register.
    pub frozen_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::experiments::Entity",
        from = "Column::ExperimentId",
        to = "super::experiments::Column::Id"
    )]
    Experiment,
}

impl Related<super::experiments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Experiment.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
