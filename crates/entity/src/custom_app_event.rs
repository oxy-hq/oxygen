use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Engineer-tagged custom event fired from inside a custom-app bundle
/// via the SDK's `useTrackEvent("export-clicked", {...})` hook.
///
/// Free-form: `event_name` is engineer-chosen (validated against
/// `^[a-z][a-z0-9-]{0,63}$` at insert time so the namespace stays
/// clean and the regex prevents accidental PII in the name itself).
/// `payload` is whatever JSON the engineer passes, ≤ 4 KiB serialized.
///
/// The Activity tab in AppDetail groups these by `event_name` for the
/// counts row + lets the operator drill into recent occurrences with
/// their payloads.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "custom_app_event")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub app_id: Uuid,
    pub user_id: Uuid,
    pub user_email: String,
    /// Inherits from the view-event session cookie so an operator can
    /// reconstruct "this user clicked export then refreshed and
    /// re-exported, all in one session."
    pub session_id: Uuid,
    pub event_name: String,
    pub payload: Json,
    pub occurred_at: DateTimeWithTimeZone,
    /// The app environment of the page that recorded it (`production`,
    /// `staging`, `dev-<handle>`). The Activity tab counts production only.
    /// `DEFAULT 'production'`, which every earlier row was.
    pub environment: String,
    #[sea_orm(
        belongs_to,
        from = "app_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub apps: BelongsTo<super::apps::Entity>,
    #[sea_orm(
        belongs_to,
        from = "user_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub users: BelongsTo<super::users::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
