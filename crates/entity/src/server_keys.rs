//! `SeaORM` Entity for `server_keys` — secrets a deployment generates for
//! itself and every one of its instances must agree on. One row per key:
//! `session`, the root of the browser-session signing key
//! (`oxy_auth::session_key`); `encryption_check`, a fixed value sealed with
//! the master key that every boot must be able to open
//! (`oxy_platform::secrets::key_check`); and `master_key`, the master key
//! itself sealed by KMS when `OXY_ENCRYPTION_KMS_KEY_ID` is set
//! (`oxy_platform::secrets::master_key`).

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "server_keys")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
    /// Random bytes (`session`) or a sealed value (`encryption_check`,
    /// `master_key`). Never
    /// logged, never returned by an API.
    pub secret: Vec<u8>,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
