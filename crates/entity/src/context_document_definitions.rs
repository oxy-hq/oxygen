//! `context_document_definitions` — compiled `.md` context documents.
//!
//! One row per markdown file that some `.agentic.yml`'s `context:` globs reach,
//! per revision. Which agent reads which document is not stored: the agent's
//! own patterns answer that at read time, against `file_path`.
//!
//! A revision compiled before this kind existed has no rows here and that is
//! NOT "no documents" — `revisions.schema_version` tells the two apart. See
//! `oxy_compile::context_documents::SINCE_SCHEMA_VERSION`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "context_document_definitions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub revision_id: Uuid,
    /// Workspace-relative, `/`-separated.
    #[sea_orm(primary_key, auto_increment = false)]
    pub file_path: String,
    pub content_sha256: String,
    pub content: String,
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
