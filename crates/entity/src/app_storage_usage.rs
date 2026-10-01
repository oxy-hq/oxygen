use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Current measured size of one custom app's asset silo
/// (`customer-app-storage/<app_id>/`). One row per app, refreshed by the
/// storage sweeper. See
/// `internal-docs/customer-apps-functions.md` ("Quotas, usage & the admin Storage tab").
///
/// Deliberately a **rollup, not a per-object index** — S3 stays authoritative
/// for objects, and this is recomputed from it, so a presigned upload that oxy
/// never observed is still counted.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_storage_usage")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub app_id: Uuid,
    /// Denormalized `apps.org_id` — quotas are org-level, and this keeps the
    /// join off the upload hot path.
    pub org_id: Uuid,
    pub bytes: i64,
    pub object_count: i64,
    /// Bytes with no `oxy-ttl` tag: growth nothing will ever reclaim.
    pub untagged_bytes: i64,
    pub untagged_object_count: i64,
    /// `{ "<top-level-prefix>/": { "bytes": n, "objects": n } }`, captured
    /// during the same walk rather than recomputed per page load.
    pub prefix_breakdown: Option<Json>,
    pub measured_at: DateTimeWithTimeZone,
    /// `ok` | `partial` | `failed`. A partial walk must stay visible: silently
    /// recording a smaller number would make a quota fail open exactly when the
    /// object store is unhealthy.
    pub measure_status: String,
    pub measure_detail: Option<String>,
    #[sea_orm(
        belongs_to,
        from = "app_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub apps: BelongsTo<super::apps::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

/// The three values `measure_status` may take. Strings on the wire (matching
/// the rest of this schema's conventions), but named here so a typo is a
/// compile error rather than a row nothing ever matches.
pub mod measure_status {
    /// The walk completed and the numbers are exact as of `measured_at`.
    pub const OK: &str = "ok";
    /// The walk was cut short (page cap, timeout); the numbers are a FLOOR.
    pub const PARTIAL: &str = "partial";
    /// The walk failed; the numbers are whatever the previous run recorded.
    pub const FAILED: &str = "failed";
}
