//! `oltp_roles` — one row per **writer** (a custom app or an Airway pipeline).
//!
//! Each writer owns exactly one schema and holds exactly one role. Storing the
//! sealed password is a deliberate divergence from airhouse: provider roles are
//! durable, not ephemeral, so rotation is an explicit operation rather than a
//! TTL expiry.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
#[serde(rename_all = "snake_case")]
pub enum WriterKind {
    /// A custom app → schema `app_<name>`.
    #[sea_orm(string_value = "app")]
    App,
    /// An Airway pipeline → schema `raw_<name>`.
    #[sea_orm(string_value = "pipeline")]
    Pipeline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(8))")]
#[serde(rename_all = "snake_case")]
pub enum GrantLevel {
    #[sea_orm(string_value = "rw")]
    ReadWrite,
    #[sea_orm(string_value = "ro")]
    ReadOnly,
}

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "oltp_roles")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tenant_row_id: Uuid,

    pub writer_kind: WriterKind,
    /// Caller-supplied name (app slug or Airway source), pre-validation.
    pub writer_name: String,
    /// Derived: `app_<name>` or `raw_<name>`.
    pub schema_name: String,
    /// Derived: `<schema_name>_rw` / `_ro`.
    pub role_name: String,
    pub grant_level: GrantLevel,

    /// AES-GCM-sealed role password. Disclosed once by the provider.
    pub password_ciphertext: Vec<u8>,

    /// Workspace that owns this schema namespace.
    ///
    /// An OLTP database is per org, but schema definitions compile per
    /// workspace — so two workspaces in one org could both declare
    /// `app_bookings` and interleave DDL into the same schema. The claim makes
    /// the second one a compile conflict instead.
    ///
    /// `None` is an unclaimed legacy row; the next `ensure_writer` adopts it.
    pub claimed_by_workspace_id: Option<Uuid>,
    /// Whether the analyst may read this schema. `None` = never chosen, so the
    /// reader falls back to the writer kind's default.
    pub analytics_visible: Option<bool>,

    pub created_at: DateTimeWithTimeZone,
    /// Last explicit rotation. `None` means never rotated since creation.
    pub rotated_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(
        belongs_to,
        from = "tenant_row_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub tenants: BelongsTo<super::tenants::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
