use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One named environment of a custom app: `production`, `staging`, or a
/// `dev-<handle>` slot. `build_id` names the build that environment serves. For a
/// dev slot it is the base build its function overlays sit on.
///
/// Phase 1a: production and staging rows mirror `apps.published_build_id` and
/// `apps.draft_build_id`, written in the same transaction by
/// `custom_apps_environments::record_move`. Naming rules:
/// `oxy_app_core::custom_app_environment`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_environments")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub app_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
    /// `production` | `staging` | `dev` (DB CHECK).
    pub kind: String,
    /// Set for `dev` slots only (DB CHECK).
    pub owner_user_id: Option<Uuid>,
    pub build_id: Option<Uuid>,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTimeWithTimeZone,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
