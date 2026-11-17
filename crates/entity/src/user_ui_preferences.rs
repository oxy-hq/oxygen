//! `user_ui_preferences` — a person's own choices about how Oxy draws its
//! interface, across every org: today the light/dark `color_mode` that Oxygen
//! and every custom app follow.
//!
//! One row per person (`user_id` is the key), written lazily: no row, or a null
//! `color_mode`, means the person has not chosen and Oxygen's default applies.
//! `color_mode` is `light`, `dark` or `system` (a `CHECK` in the migration),
//! read and written by the `oxy-api-custom-apps` appearance store. Per person
//! rather than per org (unlike `home_user_preferences`): one person sees one
//! Oxygen, whichever org's app they open. Configuration of Oxy's own interface,
//! with no org content in it, so it lives in Oxy's database (control plane).
//! The foreign key lives in the migration
//! (`m20261116_000001_user_ui_preferences`): deleting the user deletes the row.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "user_ui_preferences")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    pub color_mode: Option<String>,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
