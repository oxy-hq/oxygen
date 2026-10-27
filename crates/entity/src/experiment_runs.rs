//! `experiment_runs` — one row per registered experiment (unique
//! `experiment_id`), pinned to the design version it was registered with
//! (`design_id`). Design versions are insert-only, so the run re-reads the same
//! design years later. A retrospective run (`retrospective`) is the same row made
//! after the fact from past data: it starts `closing` and moves no lever.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "experiment_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub experiment_id: Uuid,
    /// The `experiment_designs` version this run executes; never changes.
    pub design_id: Uuid,
    /// Who registered it, and when — the platform's own clock, strictly before
    /// the registration deadline.
    pub registered_by: Uuid,
    pub registered_at: DateTimeWithTimeZone,
    /// `registered` | `live` | `closing` | `reported` | `aborted`.
    pub state: String,
    pub next_check_at: DateTimeWithTimeZone,
    /// When the run entered `closing`, local to `design.timezone`.
    pub closing_since: Option<DateTime>,
    pub abort_reason: Option<String>,
    /// `off` | `keep`: what the owner asked the abort to do with the levers, then what
    /// it did once the run is `aborted` (a guardrail always turns them off). `None` on a
    /// run aborted before this was recorded: it turned them off.
    pub abort_levers: Option<String>,
    /// The day (design timezone) the run became `aborted`.
    pub aborted_on: Option<Date>,
    /// The pool the first check pinned: the controls, or a switchback's fleet.
    pub control_pool: Option<Json>,
    /// Under `assign_by`: `{group: [member, …]}`, pinned with `control_pool`.
    pub group_membership: Option<Json>,
    /// A switchback's fleet, pinned once by the first check.
    pub fleet: Option<Json>,
    pub guardrail_breach: Option<String>,
    pub observations: Option<Json>,
    pub check_enqueued_at: Option<DateTimeWithTimeZone>,
    pub estimate_task_id: Option<Uuid>,
    /// Created from a design whose dates were all past: never checked, no lever
    /// moved, estimated at once and reported as not pre-registered.
    pub retrospective: bool,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::experiment_flips::Entity")]
    Flips,
}

impl Related<super::experiment_flips::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Flips.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
