//! `experiments` — one row per experiment name per workspace. The identity an
//! experiment's design versions (`experiment_designs`), proposals and run hang
//! off; `hypothesis` and `owner` are as the experiment was created.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "experiments")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub workspace_id: Uuid,
    /// Unique per workspace; the `{name}` in every `/experiments/{name}` route.
    pub name: String,
    pub hypothesis: String,
    pub owner: String,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::experiment_designs::Entity")]
    Designs,
}

impl Related<super::experiment_designs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Designs.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
