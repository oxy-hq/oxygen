//! One person's bookmark on one document.
//!
//! Per-viewer, which is the whole difference from a pin: a pin lives on the
//! document because an officer set it for everybody, and a favorite lives here
//! because it is nobody's business but the person who made it.
//!
//! Composite primary key, so favoriting twice is a no-op the database enforces
//! rather than a duplicate row the handler has to remember to check for.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "document_favorites")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub document_id: Uuid,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
