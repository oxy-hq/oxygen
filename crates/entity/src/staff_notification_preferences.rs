use sea_orm::entity::prelude::*;

/// A staff member's answer to "email me this?" for one platform notification.
/// Keyed by lowercased address, as `app_admins` is — a Global Owner has no
/// grant row to carry it. No row means the notification's default.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "staff_notification_preferences")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub email: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub notification: String,
    pub enabled: bool,
    pub updated_at: DateTimeWithTimeZone,
    /// The address that last changed it — the person themselves, or an admin
    /// who changed it for them.
    pub updated_by: Option<String>,
}

impl ActiveModelBehavior for ActiveModel {}
