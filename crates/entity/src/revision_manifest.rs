//! `revision_manifest` — what a revision is made of: one row per compiled file,
//! its kind, and the SHA-256 of its text. The git tree of a revision.
//!
//! A revision compiled before this table existed has no rows here; readers
//! treat that as "no manifest", never as "no files".

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "revision_manifest")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub revision_id: Uuid,
    /// Workspace-relative, `/`-separated.
    #[sea_orm(primary_key, auto_increment = false)]
    pub file_path: String,
    /// The compiled kind (`view`, `topic`, `agent`, …):
    /// `oxy_compile::manifest::kind_label`.
    pub kind: String,
    pub workspace_id: Uuid,
    pub content_sha256: String,
    #[sea_orm(
        belongs_to,
        from = "revision_id",
        to = "revision_id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub revisions: BelongsTo<super::revisions::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
