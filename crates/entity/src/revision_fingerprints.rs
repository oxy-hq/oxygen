//! `revision_fingerprints` — one hash per compiled kind per revision, over the
//! kind's sorted `(file_path, content_sha256)` manifest entries, so "did this
//! kind change between two revisions" is a comparison of two strings.
//!
//! `validated` is the compile's verdict on the semantic model, on the
//! `semantic` row only: `Some(true)` it built, `Some(false)` it did not, `None`
//! nothing checked it. The next compile's semantic gate ratchets against the
//! served revision's verdict (`oxy_compile::compile`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "revision_fingerprints")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub revision_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    /// Lowercase hex SHA-256.
    pub fingerprint: String,
    pub file_count: i32,
    pub validated: Option<bool>,
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
