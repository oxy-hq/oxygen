//! One immutable version of a document.
//!
//! Never UPDATEd and never DELETEd on its own: a version is the record of what
//! a document said when somebody signed off on it, and Compliance is the reason
//! that has to keep being true. Editing a document appends a row and moves
//! `documents.current_version_id`; it does not rewrite history.
//!
//! Exactly one of `body` / `object_key` is set, enforced by
//! `document_versions_body_xor_object_key` in the schema rather than by a
//! convention here. Read it through [`Model::is_chapter`] / [`Model::is_file`].

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "document_versions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub document_id: Uuid,
    /// 1-based and dense per document. The unique `(document_id, version_no)`
    /// index is what makes "read the max, insert max + 1" safe: a concurrent
    /// second writer loses on the constraint instead of silently taking the
    /// same number.
    pub version_no: i32,
    /// NULL when the author's user row is gone. The version stays — a document
    /// that loses its history when somebody leaves the company is unreadable
    /// exactly when it is being audited.
    pub author_id: Option<Uuid>,
    /// Markdown, for a chapter. Mutually exclusive with `object_key`.
    pub body: Option<String>,
    /// The S3 key under `org-documents/{org}/{doc}/{version}`, for a file.
    ///
    /// Platform-owned, deliberately not a per-app storage silo: documents are
    /// org assets that several apps and the analytics agent read, and parking
    /// them inside one app's silo assigns the wrong owner and makes every other
    /// reader impossible.
    pub object_key: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub created_at: DateTimeWithTimeZone,
    #[sea_orm(
        belongs_to,
        from = "document_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub documents: BelongsTo<super::documents::Entity>,
}

impl Model {
    /// Authored in the app: the text is in this row.
    pub fn is_chapter(&self) -> bool {
        self.body.is_some()
    }

    /// Uploaded: the bytes are in S3 and this row holds the key.
    pub fn is_file(&self) -> bool {
        self.object_key.is_some()
    }
}

impl ActiveModelBehavior for ActiveModel {}
