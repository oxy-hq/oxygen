use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// What the relay sweep knows about one org's environment outbox without
/// connecting to the org's store: when a publish last went into it, when a
/// relay pass last left it empty, and since when work has waited there.
/// Waiting rows are possible while `published_at` is after `drained_at`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_event_outbox_marks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub environment: String,
    pub published_at: DateTimeWithTimeZone,
    pub drained_at: Option<DateTimeWithTimeZone>,
    /// When the mark last went from new or drained to due; kept while it
    /// stays due, cleared when a pass drains it. The stale alarm's clock.
    pub pending_since: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
