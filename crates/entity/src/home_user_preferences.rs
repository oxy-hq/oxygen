//! `home_user_preferences` — a person's own choices about Oxygen's home: which
//! widget copies they hid and whether Today shows.
//!
//! One row per person per org (`(user_id, org_id)` is the key), written lazily:
//! no row means nothing hidden and Today shown. `hidden` is an array of copy keys
//! (`<app>/<widget>` or `<app>/<widget>#<id>`), checked on write by the
//! `oxy-api-custom-apps` preferences store. Configuration of Oxy's own
//! interface, with no org content in it, so it lives in Oxy's database (control
//! plane). The foreign keys live in the migration
//! (`m20261101_000001_home_user_preferences`): deleting the user or the org
//! deletes the row.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "home_user_preferences")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: Uuid,
    #[sea_orm(column_type = "JsonBinary")]
    pub hidden: Json,
    pub today_hidden: bool,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
