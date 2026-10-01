//! `workspace_preview_schemas` — the registry of Airhouse schemas a workspace
//! preview may create, `preview_<preview_key>__<live_schema>`, and when each
//! expires. A row is written before its schema exists; the TTL sweeper drops
//! only registered schemas. See `crates/app/src/server/previews/registry.rs`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_preview_schemas")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    /// `preview_<preview_key>__<live_schema>`; a CHECK holds it to that.
    #[sea_orm(primary_key, auto_increment = false)]
    pub schema_name: String,
    pub preview_key: String,
    /// The live schema this one stands in for, lowercase.
    pub live_schema: String,
    pub created_by_run_id: String,
    pub created_at: DateTimeWithTimeZone,
    pub last_written_at: DateTimeWithTimeZone,
    pub expires_at: DateTimeWithTimeZone,
    /// When the preview's own strict `CREATE SCHEMA` succeeded. `None`: the
    /// preview never created this schema, so it is never dropped.
    pub schema_created_at: Option<DateTimeWithTimeZone>,
    /// The `preview_schema_drop` run that claimed this schema, while it runs.
    pub drop_run_id: Option<String>,
    pub drop_claimed_at: Option<DateTimeWithTimeZone>,
    /// Drops claimed since the schema was last (re)created; the sweep stops
    /// at `previews::maintenance::MAX_DROP_ATTEMPTS`.
    pub drop_attempts: i32,
    pub dropped_at: Option<DateTimeWithTimeZone>,
    /// The schema is not the preview's to touch (it already existed, or it
    /// holds relations the preview did not create): never written or dropped.
    pub refused_at: Option<DateTimeWithTimeZone>,
    pub refused_reason: Option<String>,
}

impl ActiveModelBehavior for ActiveModel {}
