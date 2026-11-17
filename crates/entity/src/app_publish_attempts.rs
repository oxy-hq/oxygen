//! One custom-app publish that reached the pipeline, whatever became of it.
//!
//! `app_builds` holds one row per publish that **landed** a build; a publish
//! that failed part way is undone (`custom_apps_publish::orphan`) and leaves
//! no build row. This table is the other half: every attempt, success or
//! failure, with the stage it failed at, the error it failed with, and how long
//! each stage took (`custom_apps_publish_attempts`). Bounded per app by a
//! keep-newest-N per outcome, pruned after each attempt, as builds are GC'd
//! after each publish.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// `outcome` of an attempt that landed its build.
pub const OUTCOME_SUCCESS: &str = "success";
/// `outcome` of an attempt that failed at `failed_stage`.
pub const OUTCOME_FAILED: &str = "failed";

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_publish_attempts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub app_id: Uuid,
    /// The build row (`app_builds.id`) a successful attempt recorded. NULL for
    /// a failed attempt (its row, if it got that far, was undone) and once
    /// retention deleted the build (`ON DELETE SET NULL`).
    pub app_build_id: Option<Uuid>,
    /// The engineer-facing build id the publish carried.
    pub build_id: String,
    /// The environment the publish moved: `staging`, `production` (a
    /// promoting publish), or the sandbox `dev-<handle>` it named.
    pub environment: String,
    /// Who published, recorded as the build row records it: a user, the
    /// verified workflow of a trusted (OIDC) or `ci` publish, or the sandbox
    /// agent token. Loose — no foreign keys.
    pub published_by: Option<Uuid>,
    pub published_via: Option<String>,
    pub published_token_id: Option<Uuid>,
    pub started_at: DateTimeWithTimeZone,
    pub finished_at: DateTimeWithTimeZone,
    /// [`OUTCOME_SUCCESS`] or [`OUTCOME_FAILED`].
    pub outcome: String,
    /// The pipeline stage a failed attempt failed at (`admit`, `inflate`,
    /// `store`, `record`, `promote`, `point`). NULL on success.
    pub failed_stage: Option<String>,
    /// The refusal a failed attempt answered, truncated. NULL on success.
    pub error: Option<String>,
    /// `{stage: ms}` for every stage that ran.
    pub stage_ms: Json,
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
