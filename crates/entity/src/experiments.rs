//! `experiments` — one row per experiment name per workspace. The identity an
//! experiment's design versions (`experiment_designs`), proposals and run hang
//! off; `hypothesis` and `owner` are as the experiment was created.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
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
    #[sea_orm(has_many)]
    #[serde(skip)]
    pub experiment_designs: HasMany<super::experiment_designs::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
