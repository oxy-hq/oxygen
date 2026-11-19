//! `SeaORM` Entity for `app_service_accounts` — the one service account a
//! custom app acts as when its functions use the platform actions an org admin
//! approved for it (`internal-docs/proposals/app-platform-access.md`).
//!
//! The account is an ordinary `service_accounts` row; this links it to its app.
//! Foreign keys live in the migration (`m20261118_000050_app_platform_access`).

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "app_service_accounts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub app_id: Uuid,
    /// The account's `users` row, which is `service_accounts.user_id`. Unique:
    /// an account belongs to at most one app.
    pub service_account_user_id: Uuid,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
