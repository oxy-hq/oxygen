//! A document — one row for both kinds the product ships.
//!
//! A Knowledge-base chapter and a Compliance upload differ in where their bytes
//! live and in nothing else: same folder, same visibility rule, same versions,
//! same trash, same search. [`super::document_versions`] holds the difference.
//!
//! # The two columns that decide who can read it
//!
//! `visibility` and `location_id` are the whole access story, and neither is an
//! authorization ring. Reads are a QUERY FILTER, because a frontline worker
//! holds no `org_members` row by design and the readable set is unbounded per
//! user — the same call `/work` and the notifications inbox made. `oxy-authz`
//! gates WRITES (`Action::ManageDocuments`); it never decides which rows come
//! back.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "documents")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub org_id: Uuid,
    #[sea_orm(indexed)]
    pub folder_id: Option<Uuid>,
    pub title: String,
    /// `chapter` — markdown authored in the app. `file` — an upload in S3.
    pub kind: String,
    /// `draft` | `published`. A draft is the "Drafts" tab: its author and
    /// document managers, nobody else.
    pub status: String,
    /// `org` | `hq`. Overrides the folder's default rather than inheriting it,
    /// so moving a document between folders never changes who can read it.
    pub visibility: String,
    /// NULL = every location. Set = this store only.
    ///
    /// The column has NO ACTION on delete, which is worth knowing before you
    /// write code that deletes a location: one that still holds documents
    /// cannot be deleted at all. Both alternatives were worse — SET NULL
    /// silently WIDENS a store's document to the whole org, and CASCADE
    /// destroys a closed store's permits. Archive the location instead;
    /// `locations.status` carries the lifecycle for it.
    #[sea_orm(indexed)]
    pub location_id: Option<Uuid>,
    /// When an officer put this on the org's shelf, and who.
    ///
    /// **`pinned_at` alone decides whether it is pinned.** `pinned_by` is
    /// `ON DELETE SET NULL`, so a pin outlives its author and reads as
    /// "pinned, by somebody who has left" — a real state, and the reason the
    /// CHECK that would have tied the two together is deliberately absent (see
    /// `m20260904_000002_document_favorites_and_pins.rs`: it would have made
    /// deleting that officer fail rather than making the pin invalid).
    ///
    /// A pin is org-wide by design. The per-viewer bookmark is
    /// [`super::document_favorites`], and the two tabs exist precisely because
    /// they are different acts by different people.
    pub pinned_at: Option<DateTimeWithTimeZone>,
    pub pinned_by: Option<Uuid>,
    /// The tab this appears under in Compliance. NULL is "uncategorised",
    /// which is a state the screen renders rather than an error — deleting a
    /// category leaves its documents here.
    #[sea_orm(indexed)]
    pub category_id: Option<Uuid>,
    /// `in_review` | `approved` | `rejected`, or NULL for "nobody is asking".
    ///
    /// A separate axis from [`Self::status`], not more values in it. `status`
    /// decides whether anybody can read the document; this decides whether
    /// somebody has signed it off. A Compliance document is routinely both
    /// published and undecided, and one column cannot say that.
    pub review_status: Option<String>,
    pub reviewed_by: Option<Uuid>,
    /// Set exactly when `review_status` is a decision. `in_review` and NULL both
    /// mean undecided and both carry no timestamp — enforced by
    /// `documents_review_decision_is_whole`, so code may rely on it.
    pub reviewed_at: Option<DateTimeWithTimeZone>,
    pub review_note: Option<String>,
    /// The Compliance hook. Nothing reads it on the serving path yet — the
    /// sweeper that turns an approaching expiry into a `work_items` row
    /// ("document expiry" is already listed there as a source kind) is later,
    /// and needs no schema change when it arrives.
    pub expires_at: Option<DateTimeWithTimeZone>,
    /// The version a reader gets. NULL only while a document is still a draft
    /// with nothing uploaded — `documents_published_has_a_version` makes the
    /// published-and-empty combination unrepresentable.
    pub current_version_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(
        belongs_to,
        from = "folder_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "SetNull"
    )]
    #[serde(skip)]
    pub folders: BelongsTo<Option<super::folders::Entity>>,
    #[sea_orm(has_many)]
    #[serde(skip)]
    pub document_versions: HasMany<super::document_versions::Entity>,
}

impl Model {
    /// Not in the trash.
    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }

    pub fn is_published(&self) -> bool {
        self.status == "published"
    }

    /// Head-office only — never served to a frontline worker.
    pub fn is_hq_only(&self) -> bool {
        self.visibility == "hq"
    }

    /// Scoped to one store, rather than to the whole org.
    pub fn is_location_scoped(&self) -> bool {
        self.location_id.is_some()
    }

    /// On the org's shelf. What the "Pinned" tab lists.
    pub fn is_pinned(&self) -> bool {
        self.pinned_at.is_some()
    }

    /// Waiting on somebody. What the "Review submissions" queue counts.
    pub fn awaits_review(&self) -> bool {
        self.review_status.as_deref() == Some("in_review")
    }

    /// Signed off. Callers use this rather than comparing the string, so a
    /// fourth review state is one match arm rather than a grep.
    pub fn is_approved(&self) -> bool {
        self.review_status.as_deref() == Some("approved")
    }

    /// Expiring at or before a caller-supplied instant.
    ///
    /// The clock is a parameter rather than read here, so this stays pure: an
    /// expiry calculation that reads the wall clock can only be tested by
    /// waiting.
    pub fn expires_by(&self, horizon: DateTimeWithTimeZone) -> bool {
        self.expires_at.is_some_and(|e| e <= horizon)
    }
}

impl ActiveModelBehavior for ActiveModel {}
