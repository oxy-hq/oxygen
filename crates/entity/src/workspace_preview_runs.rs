//! `workspace_preview_runs` — work staff start on a previewed branch: the
//! Airway change check (`analyze`), and later procedure dry runs, transform
//! builds, Airway samples and compares. Ids, names and states only; a run's
//! result rides its `agentic_runs` row, which shares `run_id`. See
//! `crates/app/src/server/previews/analyze.rs`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_preview_runs")]
pub struct Model {
    /// Also the id of the run's `agentic_runs` row.
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: String,
    pub workspace_id: Uuid,
    pub branch: String,
    pub preview_key: String,
    /// The staging revision the run reads.
    pub revision_id: Uuid,
    /// `analyze` | `procedure` | `transform_build` | `airway_sample` | `compare`.
    pub kind: String,
    pub target_ref: Option<String>,
    pub parent_run_id: Option<String>,
    pub options: Json,
    /// `queued` | `running` | `finished`.
    pub state: String,
    pub requested_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub started_at: Option<DateTimeWithTimeZone>,
    pub finished_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
