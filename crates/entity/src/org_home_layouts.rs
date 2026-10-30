//! `org_home_layouts` — how an org's Oxygen home is laid out: which sections it
//! draws, in what order, and which app widgets at what size.
//!
//! One row per org, the layout document as written (`layout`, checked against
//! the org's widgets on every write by `oxy_app_core::widgets::layout::check`).
//! No row means the default home. Configuration of Oxy's own interface, with no
//! org content in it, so it lives in Oxy's database (control plane).
//!
//! `revision` counts writes: a writer sends back the revision it read, and a
//! stale one is refused, so two agents never overwrite each other unseen.
//! `updated_by` is audit, never authority. The foreign keys live in the
//! migration (`m20261029_000001_org_home_layouts`): deleting the org deletes the
//! layout; deleting the writer only forgets who wrote it.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "org_home_layouts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: Uuid,
    #[sea_orm(column_type = "JsonBinary")]
    pub layout: Json,
    pub revision: i64,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
