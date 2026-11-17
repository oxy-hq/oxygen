//! `schema_migration_definitions` — compiled `schemas/*.sql` files.
//!
//! One row per migration file per revision. Ordering is by `file_path`, which
//! is why the convention is a zero-padded numeric prefix (`0001_orders.sql`):
//! lexical order is the apply order, with no separate sequence to keep in sync.
//!
//! Plain SQL rather than a schema DSL on purpose — these are authored by Oxy
//! engineers in a vibe-coding flow, and every model writes Postgres DDL
//! fluently while none has seen a bespoke DSL.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "schema_migration_definitions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub revision_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub file_path: String,
    /// Content hash. The applier compares this against the ledger so an edited
    /// already-applied migration is caught rather than silently ignored.
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
