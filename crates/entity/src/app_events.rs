use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A relayed app event: its routing metadata. `id` is the UUIDv5 the app
/// computed; no database default. The body is not here: it stays in the
/// org's OLTP outbox log, `oxy_events.outbox`, under `(writer_role, id)`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub environment: String,
    pub source_app_id: Option<Uuid>,
    pub r#type: String,
    pub key: String,
    /// The body's size, as `jsonb` text in the org's store.
    pub data_bytes: i32,
    /// The OLTP writer role that published the event: with `id`, the key of
    /// the body in the org's outbox log.
    pub writer_role: String,
    pub schema_revision_id: Option<Uuid>,
    pub actor: Option<String>,
    pub causation_id: Option<Uuid>,
    pub root_id: Uuid,
    pub depth: i32,
    pub invocation_id: Option<Uuid>,
    pub delay_seconds: i32,
    pub published_at: DateTimeWithTimeZone,
    pub relayed_at: DateTimeWithTimeZone,
    pub relay_error: Option<String>,
    /// When the event was copied into the workspace's Airhouse `oxy_event_log`.
    pub mirrored_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
