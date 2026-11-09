//! `workspace_periodic_ticks` — when each workspace is next due to be driven
//! by the periodic global driver, and which compiled revision its schedule
//! rows were last reconciled from. Written by the driver's cloud tick
//! (`oxy-app`'s `server::router::periodic`); `next_tick_at` is its
//! `FOR UPDATE SKIP LOCKED` claim.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_periodic_ticks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    /// When the workspace is next due. The claim moves it one driver interval
    /// out, so the node whose UPDATE returned the row owns this interval.
    pub next_tick_at: DateTimeWithTimeZone,
    pub claimed_at: Option<DateTimeWithTimeZone>,
    /// `workspaces.current_revision_id` at the last health / pre-aggregation /
    /// monitor schedule reconcile; a different value means the reconcile runs.
    pub last_reconciled_revision_id: Option<Uuid>,
    pub last_reconciled_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
