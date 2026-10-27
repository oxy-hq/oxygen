//! `experiment_flips` — one row per (run, unit, target state, due moment). The
//! unique key is what makes a repeated tick record nothing; `task_id` going
//! from NULL to set, in the same transaction as the lever's run row, is what
//! makes the lever job exactly-once. Under `per: wave` every row of one
//! `(wave, target_state, due_at)` carries the same `task_id`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "experiment_flips")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub run_id: Uuid,
    pub unit: String,
    /// `on` | `off`.
    pub target_state: String,
    /// When the flip is due, local to `design.timezone` (midnight below hour grain).
    pub due_at: DateTime,
    /// Wave index, or the switchback period's index.
    pub wave: i32,
    pub task_id: Option<Uuid>,
    pub agentic_run_id: Option<Uuid>,
    /// The promoted revision `via` resolved at when the job was enqueued.
    pub lever_revision_id: Option<Uuid>,
    /// `pending` | `dispatched` | `confirmed` | `unconfirmed` | `failed` | `cancelled`.
    pub status: String,
    /// The reading read-back saw the lever land on, local (a midnight below hour grain).
    pub confirmed_on: Option<DateTime>,
    pub reason: Option<String>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::experiment_runs::Entity",
        from = "Column::RunId",
        to = "super::experiment_runs::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Run,
}

impl Related<super::experiment_runs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
