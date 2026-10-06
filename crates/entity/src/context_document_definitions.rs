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
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::revisions::Entity",
        from = "Column::RevisionId",
        to = "super::revisions::Column::RevisionId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Revisions,
}

impl Related<super::revisions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Revisions.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
