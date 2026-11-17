//! `compiled_sources` — a compiled file's text, stored once per workspace by
//! the SHA-256 of that text.
//!
//! Only the kinds whose text a reader parses are kept (semantic views and
//! topics); `revision_manifest` names the hash of every compiled file either
//! way. Rows are shared by every revision that carries the same text, so they
//! are not deleted by revision retention: `oxy_compile::writer::
//! prune_unreferenced_sources` deletes the ones no manifest names any more.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "compiled_sources")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    /// Lowercase hex SHA-256 of `content`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub content_sha256: String,
    pub content: String,
    pub byte_size: i32,
    /// Touched by every compile that carries this text; the sweep only deletes
    /// rows older than its grace period.
    pub last_seen_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
