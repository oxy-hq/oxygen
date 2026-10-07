//! `SeaORM` Entity for `server_keys` — secrets a deployment generates for
//! itself and every one of its instances must agree on. One row per key; today
//! only `session`, the root of the browser-session signing key
//! (`oxy_auth::session_key`).

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "server_keys")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
    /// Random bytes. Never logged, never returned by an API.
    pub secret: Vec<u8>,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
