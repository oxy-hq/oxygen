//! `SeaORM` Entity for `service_accounts` — an org-owned machine principal
//! (API-tokens design §3.3, §7).
//!
//! The row is the account's whole standing: a `users` row with no email names
//! it, and this says which org it belongs to and how high it may act there.
//! It is **not** an `org_members` row. Foreign keys and the `org_role` CHECK
//! live in the migration (`m20261002_000001_service_accounts`).

use sea_orm::entity::prelude::*;

/// `service_accounts.org_role`: a plain member of its org.
pub const ROLE_MEMBER: &str = "member";
/// `service_accounts.org_role`: an admin of its org. There is no `owner`.
pub const ROLE_ADMIN: &str = "admin";

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "service_accounts")]
pub struct Model {
    /// The account's `users` row — what a token's `principal_user_id` names.
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    pub org_id: Uuid,
    /// `member | admin`. Never `owner`.
    pub org_role: String,
    /// A slug, unique per org.
    pub name: String,
    pub description: Option<String>,
    /// Audit only; never authority.
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    /// Set = every token of the account is refused.
    pub disabled_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
