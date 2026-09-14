//! A folder in the document tree.
//!
//! `visibility` here is a DEFAULT for what lands inside, not a binding rule.
//! The twelve folders this is modelled on are authored per team — "Back of
//! House", "Marketing Essentials SOP" — and the exceptions live at the item, so
//! a folder that dictated visibility would be wrong for exactly the documents
//! anybody stops to think about. [`super::documents::Model::visibility`] is
//! what a read actually consults.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "folders")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub org_id: Uuid,
    /// NULL at the root. Deliberately NOT declared as a sea-orm relation: a
    /// self-join needs a hand-written `Linked`, and every caller so far walks
    /// the tree in one query and assembles it in memory instead.
    pub parent_id: Option<Uuid>,
    pub name: String,
    /// `org` | `hq`. See [`Model::is_hq_only`] rather than testing the string.
    pub visibility: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// Set = in the trash. This IS the product's "Deleted" tab, not a hedge
    /// against a hard delete — restoring is the feature.
    pub deleted_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(has_many)]
    #[serde(skip)]
    pub documents: HasMany<super::documents::Entity>,
}

impl Model {
    /// Head-office only: no frontline worker may see what is filed here by
    /// default. Callers use this rather than comparing the string, so a third
    /// visibility becomes one match arm rather than a grep.
    pub fn is_hq_only(&self) -> bool {
        self.visibility == "hq"
    }

    /// Not in the trash.
    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }
}

impl ActiveModelBehavior for ActiveModel {}
