//! A tenant-defined document category.
//!
//! Same shape and same reasoning as [`super::org_roles`]: the eight names one
//! operator files paperwork under — "Certificate of Insurance", "Design
//! Ext/Int", "EIN", "Lease" — are theirs, and an enum would mean a migration
//! every time somebody invents a ninth.
//!
//! **Not a visibility control.** A category is a label and a tab on a screen.
//! Who may read a document is decided by `visibility` and `location_id` and
//! nothing else, which is why a category can be deleted out from under its
//! documents (they become uncategorised) where a location cannot.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "document_categories")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub org_id: Uuid,
    /// Unique within the org — two tabs with the same name is a support ticket.
    pub name: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
