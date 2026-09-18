//! `airway_workspace_pipeline_state` — incremental ingest state, per workspace.
//!
//! One row per `(workspace_id, pipeline_name)`: the serialized
//! `PipelineState`, the `Schema`, and a monotonic `version` for optimistic
//! concurrency on save. The same key the single-flight lease uses, so a run's
//! lease and its cursor can never name different things.
//!
//! Replaces [`super::pipeline_state`], which is keyed by `pipeline_name`
//! alone and is therefore shared by every workspace running a pipeline of that
//! name. The store reads this table and adopts the legacy row once when a
//! workspace has none yet; see `crate::state_store`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "airway_workspace_pipeline_state")]
pub struct Model {
    /// The workspace the run belongs to (`Uuid::nil()` in local mode and for
    /// `oxy airway run`, matching what the run row and the lease carry).
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    /// Pipeline name (`AirwayPipelineSpec.name`).
    #[sea_orm(primary_key, auto_increment = false)]
    pub pipeline_name: String,
    /// Serialized `airway::PipelineState`.
    pub state: Json,
    /// Serialized `airway::Schema`. `None` means no schema is provisioned —
    /// either nothing has run yet, or a reset left a tombstone here so the
    /// legacy row is not adopted back.
    pub schema_json: Option<Json>,
    /// Monotonic version used for optimistic concurrency on save.
    pub version: i64,
    pub updated_at: ChronoDateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
