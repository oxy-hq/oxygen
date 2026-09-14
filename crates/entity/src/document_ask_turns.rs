//! One question and the answer it got.
//!
//! Append-only. Nothing here is edited after it is written, because it is a
//! record of what somebody was told rather than a document about it.
//!
//! [`Model::cited`] holds document ids as JSON rather than rows in a join
//! table. A citation records what an answer used at a moment in time; a
//! foreign key with `ON DELETE CASCADE` would let deleting a document rewrite
//! a transcript that was true when it was written. The reader hydrates these
//! through `visible_documents`, so an id whose document is gone — or which the
//! reader may no longer open — renders as nothing rather than as a title.
//!
//! There is deliberately no column for document TEXT. See the migration.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "document_ask_turns")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub session_id: Uuid,
    /// 1-based. The order of a conversation is a fact about it, not something
    /// to rediscover from timestamps that can collide inside one millisecond.
    pub seq: i32,
    pub question: String,
    /// `None` when no engine wrote one. A turn the library had nothing to say
    /// to is still a turn that happened, and dropping it would make the
    /// transcript disagree with what the reader saw.
    pub answer: Option<String>,
    /// Every document the engine was given, in the order it was given them.
    ///
    /// What the `[1]`, `[2]` markers inside [`Self::answer`] index into. Stored
    /// because the numbering is by candidate position, not by citation order —
    /// a transcript holding only [`Self::cited`] renders prose whose numbers
    /// point at nothing.
    pub sources: Json,
    /// The subset the answer actually used, in the order it first used them.
    pub cited: Json,
    pub created_at: DateTimeWithTimeZone,
}

impl Model {
    /// The cited ids, or none when the column holds anything else.
    ///
    /// Lenient rather than fallible: a transcript that will not render because
    /// one row's JSON is malformed is worse than one citation going missing,
    /// and the citations are the optional half of a turn.
    pub fn cited_ids(&self) -> Vec<Uuid> {
        serde_json::from_value(self.cited.clone()).unwrap_or_default()
    }

    /// The documents the engine was given, in marker order. Lenient for the
    /// same reason as [`Self::cited_ids`].
    pub fn source_ids(&self) -> Vec<Uuid> {
        serde_json::from_value(self.sources.clone()).unwrap_or_default()
    }
}

impl ActiveModelBehavior for ActiveModel {}
