use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Append-only history of environment pointer moves. It is both the deploy history
/// and the evidence the promote invariant checks ("production only serves a build
/// staging has served").
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_environment_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub app_id: Uuid,
    /// An `app_environments.name`.
    pub environment: String,
    /// NULL after an `unpublish`, or once `gc_builds` has reaped the build.
    pub build_id: Option<Uuid>,
    /// `publish` | `promote` | `rollback` | `unpublish` | `reset` | `backfill` (DB CHECK).
    pub action: String,
    pub actor: Option<Uuid>,
    pub at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
