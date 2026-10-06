//! `workspace_compile_checks` — when a remote-backed workspace is next due to
//! have its default branch's head compared with the revision it serves, and
//! what the last comparison found. Written by the compile reconcile loop
//! (`oxy-app`'s `server::compile_reconcile`); `next_check_at` is its
//! compare-and-set claim.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_compile_checks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    pub next_check_at: DateTimeWithTimeZone,
    pub last_checked_at: Option<DateTimeWithTimeZone>,
    /// The branch head GitHub reported at the last check that got an answer.
    pub last_head_sha: Option<String>,
    /// A stable label for what the last check did (`up_to_date`, `enqueued`,
    /// `no_token`, `not_found`, …).
    pub last_outcome: Option<String>,
}

impl ActiveModelBehavior for ActiveModel {}
