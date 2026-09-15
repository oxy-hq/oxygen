//! `backfill_ranges` — one row per user-initiated backfill of a `[from, to)`
//! window for a pipeline. The parent of the `backfill_checkpoints` chunks it
//! spawned: every chunk carries `backfill_range_id`, so ranges are kept SEPARATE
//! (no cross-range merge). Re-running an overlapping window is a NEW range with
//! its own chunks — safe because airhouse is merge-on-read (a re-load dedups).
//! The UI lists ranges (a gantt) and drills into a range's chunk coverage.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "backfill_ranges")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Owning workspace (scopes the range in shared multi-tenant Postgres).
    /// `Uuid::nil()` (LOCAL_WORKSPACE_ID) for the single-tenant local/CLI path.
    pub workspace_id: Uuid,
    /// The `*.airway.yml` pipeline this backfill targets.
    pub pipeline_ref: String,
    /// The requested half-open window `[requested_from, requested_to)`.
    pub requested_from: DateTimeWithTimeZone,
    pub requested_to: DateTimeWithTimeZone,
    /// Chunk size the window was split at: `day` | `week` | `month`.
    pub granularity: String,
    /// Max chunks the driver ran concurrently.
    pub concurrency: i32,
    /// Resources this backfill was scoped to, as a JSON array of names.
    ///
    /// `None` means every resource the pipeline declares, which is what ranges
    /// created before the column existed did.
    ///
    /// Stored rather than passed because `/resume-backfill` re-drives a range
    /// knowing only its id: a scope living in the originating request would
    /// apply to the first drive and silently widen on every resume. It matters
    /// because a backfill run is run-scoped, so a SNAPSHOT resource sees empty
    /// state, believes it has never run, and pulls its ordinary daily snapshot
    /// on every chunk — spending report jobs on a period Amazon cannot serve
    /// historically anyway.
    pub resources: Option<Json>,
    /// The user who started this backfill (`None` for CLI/local runs).
    pub created_by: Option<Uuid>,
    /// Rollup over this range's chunks: `running` (any pending/running) |
    /// `done` (all done) | `degraded` (all terminal, some completed_with_errors)
    /// | `failed` (any failed/timed_out/cancelled) | `cancelled`.
    pub status: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
