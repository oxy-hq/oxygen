//! One conversation with the library.
//!
//! Owned by the person who opened it and by nobody else — there is no officer
//! override on this table, deliberately. Reading what a worker asked is
//! surveillance, not administration.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "document_ask_sessions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub org_id: Uuid,
    #[sea_orm(indexed)]
    pub user_id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    /// Bumped by each turn. The list sorts on this, so a conversation somebody
    /// came back to rises rather than sinking under newer, shorter ones.
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
