//! `SeaORM` Entity for `rate_limit_windows` — one row per limiter key per
//! fixed window, counting the calls every replica saw in it. Written and read
//! by `oxy_app_core::rate_limit` with one atomic upsert; rows past
//! `expires_at` are deleted opportunistically by the same module. Keys are
//! `<limiter>:<ids>` and never carry content.

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "rate_limit_windows")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub window_start: DateTimeWithTimeZone,
    pub expires_at: DateTimeWithTimeZone,
    pub count: i64,
}

impl ActiveModelBehavior for ActiveModel {}
