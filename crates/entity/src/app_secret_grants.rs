//! A workspace secret shared with one custom app: the row that says "app X may
//! read workspace secret Y", created by a workspace admin, never by a bundle.
//!
//! `ctx.env` sees an app's own `apps/<app_id>/<KEY>` secrets and, through these
//! rows, the workspace secrets shared with it — read as `ctx.env.<name>`, the
//! workspace secret's own name. One stored value, any number of apps, one
//! rotation. Nothing else reaches a workspace secret from an app: there is no
//! fallback and no manifest field (`internal-docs/customer-apps-functions.md`,
//! "Workspace secrets shared with an app").
//!
//! `secret_id` and `project_id` are loose UUIDs (domain-boundaries P3: the
//! secret is the tenancy aggregate's); `app_id` cascades with its app. A secret
//! is soft-deleted, so a grant of an inactive secret resolves to nothing, and
//! the workspace delete path removes the grants with it.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_secret_grants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// The workspace both the secret and the app belong to.
    #[sea_orm(indexed)]
    pub project_id: Uuid,
    /// The workspace secret (`secrets.id`) being shared.
    pub secret_id: Uuid,
    #[sea_orm(indexed)]
    pub app_id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    /// Who shared it. Loose: NULL-able, and not cleared with the user.
    pub created_by: Option<Uuid>,
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
