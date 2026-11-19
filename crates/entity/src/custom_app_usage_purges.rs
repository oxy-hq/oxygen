//! A deleted custom app's usage facts that its workspace's Airhouse still owes
//! a delete (`custom_apps_usage_purge::ledger`).
//!
//! One row per deleted app, written in the transaction that deletes it. Unlike
//! the queue row of the purge task, queue retention never prunes it while the
//! purge is owed. No foreign key to `apps`: the app is gone by design.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "custom_app_usage_purges")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub app_id: Uuid,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    /// Failed runs so far.
    pub attempts: i32,
    /// When the sweep may queue the next run.
    pub next_attempt_at: DateTimeWithTimeZone,
    /// The last failure, truncated.
    pub last_error: Option<String>,
    /// Set when Airhouse deleted the facts (or the workspace has none).
    pub completed_at: Option<DateTimeWithTimeZone>,
    /// Set when the retries ran out. The debt stays visible; clearing this and
    /// resetting `attempts` retries it.
    pub abandoned_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
