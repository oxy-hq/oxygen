//! Request and response shapes for the document surface.

use chrono::{DateTime, FixedOffset};
use entity::documents;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One row in a listing.
///
/// Deliberately does NOT carry a chapter's body. A folder of thirty SOPs would
/// otherwise ship thirty markdown documents to render thirty titles, and the
/// list is the screen a worker opens first on a tablet.
#[derive(Debug, Serialize)]
pub struct DocumentSummary {
    pub id: Uuid,
    pub title: String,
    /// `file` | `chapter`.
    pub kind: String,
    /// `draft` | `published`.
    pub status: String,
    /// `org` | `hq`.
    pub visibility: String,
    pub folder_id: Option<Uuid>,
    /// NULL = every location.
    pub location_id: Option<Uuid>,
    pub expires_at: Option<DateTime<FixedOffset>>,
    pub updated_at: DateTime<FixedOffset>,
    /// Filled in by `hydrate` on the read path, absent on a manage response —
    /// the caller of a create already knows what they just sent.
    pub author_name: Option<String>,
    pub location_name: Option<String>,
    /// The current version's number, absent while a draft has nothing in it.
    pub version_no: Option<i32>,
    /// On the org's shelf, and when. Org-wide: every viewer sees the same pin.
    pub pinned_at: Option<DateTime<FixedOffset>>,
    /// This viewer's own bookmark. Per-viewer, unlike `pinned_at`.
    pub is_favorite: bool,
    pub category_id: Option<Uuid>,
    pub category_name: Option<String>,
    /// `in_review` | `approved` | `rejected`, or absent when nobody is asking.
    pub review_status: Option<String>,
    pub reviewed_at: Option<DateTime<FixedOffset>>,
    /// Who decided, and why.
    ///
    /// Both are written by `review::decide` and neither came back from any
    /// route, so a rejection reason was stored and unreadable: the officer
    /// typed "missing signature" and the submitter saw `rejected`. "Who
    /// approved this" is the field a compliance audit actually asks for.
    ///
    /// A name rather than the id, because every other person on this struct is
    /// a name. Filled by `hydrate::summary`; absent on the bare conversion,
    /// like the rest of them.
    pub reviewed_by_name: Option<String>,
    pub review_note: Option<String>,
    /// The current version's content type. What a screen labels "SOP", "PDF" or
    /// "Chapter" is a rendering of this plus `kind`, not a column of its own —
    /// three display names for two facts, and the mapping belongs to whoever is
    /// drawing the badge.
    pub content_type: Option<String>,
}

impl From<documents::Model> for DocumentSummary {
    /// The one place a row becomes a listing entry. Both the read handlers and
    /// the manage handlers return summaries, and two conversions is how a field
    /// ends up present on a create response and missing from a list.
    fn from(d: documents::Model) -> Self {
        Self {
            id: d.id,
            title: d.title,
            kind: d.kind,
            status: d.status,
            visibility: d.visibility,
            folder_id: d.folder_id,
            location_id: d.location_id,
            expires_at: d.expires_at,
            updated_at: d.updated_at,
            pinned_at: d.pinned_at,
            // NOT a fact — this conversion has no caller to ask. Every listing
            // goes through `hydrate::summary`, which fills it in; the write
            // handlers that return a bare row must set it themselves rather
            // than let this default ship, because "false" on a document the
            // caller has starred is a lie the star renders.
            is_favorite: false,
            category_id: d.category_id,
            category_name: None,
            review_status: d.review_status,
            reviewed_at: d.reviewed_at,
            reviewed_by_name: None,
            review_note: d.review_note,
            author_name: None,
            location_name: None,
            version_no: None,
            content_type: None,
        }
    }
}

/// One document, opened.
#[derive(Debug, Serialize)]
pub struct DocumentDetail {
    #[serde(flatten)]
    pub summary: DocumentSummary,
    /// A chapter's body. Always absent for a `file` — its bytes come from
    /// the download redirect, never inline.
    pub body: Option<String>,
}

/// A node in the folder tree. The tree is assembled by the client from
/// `parent_id`; sending it nested would make paging one impossible later.
#[derive(Debug, Serialize)]
pub struct FolderNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub visibility: String,
    /// Live documents filed directly in this folder that THIS CALLER could
    /// open, not counting subfolders.
    ///
    /// Per-viewer, and it used to say the opposite: the count was org-wide on
    /// the argument that two people disagreeing about it reads as data loss.
    /// That argument lost to `hq` — an `org`-visible folder holding three
    /// head-office documents reported "3 items" to the audience `hq` exists to
    /// exclude, who opened it to an empty list. See `hydrate::folder_counts`.
    pub item_count: i64,
}

/// A Compliance tab, with the number beside its name.
#[derive(Debug, Serialize)]
pub struct CategoryNode {
    pub id: Uuid,
    pub name: String,
    /// Live documents in this category, org-wide — see `categories::counts`.
    pub item_count: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCategory {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateCategory {
    pub name: String,
}

/// Submit for review, or decide. One route rather than three, because the three
/// transitions share every field and differ only in which one they set — and
/// three endpoints would be three places to forget the timestamp rule.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDecision {
    /// `in_review` | `approved` | `rejected`. Clearing a review is not a
    /// transition this exposes: a document that was approved and is now
    /// unreviewed loses the record that it ever was.
    pub status: String,
    /// Why. Most useful on a rejection, and kept for an approval too, because
    /// "approved with a note" is how an operator records a condition.
    pub note: Option<String>,
}

fn default_limit() -> u64 {
    100
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Required. The read routes are not nested under `/orgs/{org_id}`, so the
    /// org arrives here — and is checked, never trusted, by
    /// `visibility::resolve_standing`.
    pub org_id: Uuid,
    pub folder_id: Option<Uuid>,
    /// Compliance's only listing filter: what expires on or before this.
    pub expiring_before: Option<DateTime<FixedOffset>>,
    /// Narrow to one Compliance tab.
    pub category_id: Option<Uuid>,
    /// The review queue: `in_review` for "Review submissions", `approved` for
    /// the settled ones.
    pub review_status: Option<String>,
    /// The "Favorites" tab — this viewer's bookmarks only.
    #[serde(default)]
    pub favorited: bool,
    /// The "Pinned" tab — the org's shelf, same for everyone.
    #[serde(default)]
    pub pinned: bool,
    /// The "Deleted" tab. Honoured for officers and quietly ignored for
    /// everyone else, which fails closed without announcing that a trash
    /// exists — see `visibility::visible_documents_scoped`.
    #[serde(default)]
    pub deleted: bool,
    #[serde(default = "default_limit")]
    pub limit: u64,
    /// Where this page starts. Absent is the first page, which is what every
    /// other adopter of `oxy_app_core::pagination` means by absent.
    ///
    /// Offset rather than a keyset cursor on `(updated_at, id)`, deliberately:
    /// the ordering is stable but the CONTENT is not — a document edited while
    /// somebody pages moves to the front under `updated_at DESC`, and a keyset
    /// would silently skip whatever took its place. Neither shape survives that
    /// perfectly; offset at least degrades the way readers already expect a
    /// list to, and it matches the two conventions already in this codebase.
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Deserialize)]
pub struct FolderQuery {
    pub org_id: Uuid,
    /// The "Deleted" tab, for folders. Honoured for officers and quietly
    /// ignored for everyone else, exactly as `ListQuery::deleted` is.
    #[serde(default)]
    pub deleted: bool,
}

/// `deny_unknown_fields` for the reason `CreateDocument` states: a folder write
/// that silently discards a field it does not know is the same failure, and the
/// two document DTOs having it while these two did not was an oversight rather
/// than a decision.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateFolder {
    pub name: String,
    pub parent_id: Option<Uuid>,
    /// Defaults to `org` — the visible-by-default choice, because a folder
    /// nobody can see is a support ticket and a folder everybody can see is
    /// what the author almost always meant.
    pub visibility: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateFolder {
    pub name: Option<String>,
    /// Doubly wrapped, and through the adapter: explicit null moves the folder
    /// to the root.
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub parent_id: Option<Option<Uuid>>,
    pub visibility: Option<String>,
}

///
/// `deny_unknown_fields`, and that is not decoration. This struct shipped
/// WITHOUT `category_id` while the column, the routes, the counts and the tabs
/// all existed — so a caller sending one got a `201`, a document with no
/// category, and no error anywhere. Serde drops what it does not recognise, and
/// a write API that silently discards half a request is the failure this
/// codebase keeps finding. A typo is now a rejection that names the field —
/// `422`, not `400`: axum renders a `deny_unknown_fields` failure as
/// `JsonRejection::JsonDataError`, which is the body-was-read-but-is-wrong
/// status. Stated exactly because the previous wording said `400` and a client
/// written against it would branch on the wrong number.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDocument {
    pub title: String,
    /// `file` | `chapter`.
    pub kind: String,
    /// The Compliance tab it is filed under. Checked against the org like every
    /// other id a write supplies.
    pub category_id: Option<Uuid>,
    pub folder_id: Option<Uuid>,
    pub visibility: Option<String>,
    pub location_id: Option<Uuid>,
    pub expires_at: Option<DateTime<FixedOffset>>,
}

/// Every field optional; absent means "leave it alone".
///
/// `folder_id` and `location_id` are doubly wrapped so the API can express
/// "move it to the root" / "unlink it from its store" — a single `Option` cannot
/// tell an absent field from an explicit null, and clearing either of those two
/// is a thing somebody has to be able to do.
///
/// The wrapping alone does NOT do that. Serde decides the OUTER `Option` from
/// whether the key is present and then parses `null` into the inner one, so a
/// bare `Option<Option<T>>` deserialises both "absent" and `null` to `None` and
/// `Some(None)` is unreachable — which made every clearing path here dead code
/// under a doc comment promising it worked. `double_option` is the adapter that
/// makes the distinction real; `crates/cameras/src/routes/operator.rs` uses it
/// for the same reason.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDocument {
    pub title: Option<String>,
    /// Doubly wrapped like `folder_id`: absent leaves it alone, explicit null
    /// makes the document uncategorised, which is a state the screen renders.
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub category_id: Option<Option<Uuid>>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub folder_id: Option<Option<Uuid>>,
    pub visibility: Option<String>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub location_id: Option<Option<Uuid>>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub expires_at: Option<Option<DateTime<FixedOffset>>>,
    /// `draft` | `published`. Publishing without a version is refused; the
    /// schema would refuse it anyway, and this turns that into a 400 that says
    /// which.
    pub status: Option<String>,
}

/// One entry in a document's history.
///
/// Carries no body and no key. Shipping every revision of every chapter to draw
/// a list of dates is the same mistake `DocumentSummary` avoids.
///
/// **A record, not a selector.** An earlier version of this comment said the
/// list "is read to choose a version", which no route lets you act on: nothing
/// writes `current_version_id` except creating a version, and `confirm` refuses
/// anything with no `object_key`, so a chapter's history is read-only and a
/// file's is too. Reverting is unbuilt rather than hidden — the way back today
/// is to author or upload a new version carrying the old content, which is a
/// worse answer than a revert route and is the honest description of what
/// exists.
#[derive(Debug, Serialize)]
pub struct VersionSummary {
    pub version_no: i32,
    pub author_name: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub created_at: DateTime<FixedOffset>,
    /// Is this the one readers currently get?
    pub is_current: bool,
}

/// One version, opened.
///
/// The history list has always been [`VersionSummary`] — dates and authors,
/// enough to draw a list and nothing else. This is what that list could not
/// reach: the version itself. A compliance library whose whole claim is an
/// audit trail was showing `v1` and `v2` with no way to read either.
#[derive(Debug, Serialize)]
pub struct VersionContent {
    pub version_no: i32,
    pub author_name: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub created_at: DateTime<FixedOffset>,
    pub is_current: bool,
    /// A chapter's text.
    ///
    /// Absent for a file, whose bytes never pass through this server — the
    /// download route hands out a presigned URL instead. Absent is therefore
    /// "fetch it the other way", not "this version is empty", and the two are
    /// told apart by `content_type` rather than by guessing from a null.
    pub body: Option<String>,
}

/// Start a new version. A chapter carries its text; a file asks for a URL to
/// send bytes to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewVersion {
    /// Chapter only. Mutually exclusive with the two upload fields.
    pub body: Option<String>,
    /// File only.
    pub content_type: Option<String>,
    /// File only. Part of the presigned signature, so the object store rejects
    /// a body that does not match it.
    pub content_length: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct NewVersionResponse {
    pub document_id: Uuid,
    pub version_no: i32,
    /// Present for a file: PUT the bytes here, then confirm. Absent for a
    /// chapter, which is already stored and already current.
    pub upload_url: Option<String>,
    /// Present for a file. Recorded now so the confirm step has nothing to
    /// guess and no caller-supplied key to validate.
    pub object_key: Option<String>,
    /// True when the version is already the document's current one — a chapter
    /// is, a file is not until its upload is confirmed.
    pub is_current: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Query;
    use axum::http::Uri;

    /// Every filter a screen sends, parsed the way axum will parse it.
    ///
    /// These are pure `serde_urlencoded` round trips and they earn their place
    /// because nothing else in the suite touches them: the handler tests call
    /// functions with already-built structs, so a query string the frontend
    /// will really send has never once reached a `Deserialize`. A filter that
    /// 400s is indistinguishable from a filter that finds nothing.
    fn list(q: &str) -> Result<ListQuery, String> {
        let uri: Uri = format!("/api/documents?{q}").parse().unwrap();
        Query::<ListQuery>::try_from_uri(&uri)
            .map(|Query(v)| v)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn the_minimal_listing_query_is_just_an_org() {
        let q = list("org_id=00000000-0000-0000-0000-000000000001").expect("parse");
        assert_eq!(q.limit, 100, "the default page size");
        assert!(!q.deleted && !q.favorited && !q.pinned);
        assert!(q.folder_id.is_none() && q.category_id.is_none());
        assert!(q.expiring_before.is_none());
    }

    /// The one Compliance actually sends. An RFC 3339 timestamp through a query
    /// string is the parse most likely to be wrong, and it is the only filter
    /// that screen has.
    #[test]
    fn compliance_sends_an_rfc3339_horizon() {
        let q = list(
            "org_id=00000000-0000-0000-0000-000000000001\
             &expiring_before=2026-12-31T23%3A59%3A59%2B00%3A00",
        )
        .expect("parse an expiry horizon");
        assert_eq!(
            q.expiring_before.expect("a horizon").to_rfc3339(),
            "2026-12-31T23:59:59+00:00"
        );
    }

    /// The form a browser actually sends. `Date.toISOString()` emits `Z`, and
    /// that matters more than it looks: an RFC 3339 offset carries a literal
    /// `+`, which a query string form-decodes as a space — so the offset form
    /// reaches the server broken unless the caller percent-encodes it. Correct
    /// per the URL spec, invisible from the browser, and a trap for anyone
    /// hand-rolling a request. Found by running the seed script against a real
    /// server, which is the only place it can be found.
    #[test]
    fn the_horizon_a_browser_sends_is_the_z_form() {
        let q = list(
            "org_id=00000000-0000-0000-0000-000000000001&expiring_before=2026-12-31T23:59:59Z",
        )
        .expect("parse the Z form unencoded");
        assert_eq!(
            q.expiring_before.expect("a horizon").to_rfc3339(),
            "2026-12-31T23:59:59+00:00"
        );

        // The same instant written with an unencoded offset does NOT parse, and
        // pinning that keeps the reason above from being rediscovered the hard
        // way a second time.
        assert!(
            list("org_id=00000000-0000-0000-0000-000000000001&expiring_before=2026-12-31T23:59:59+00:00")
                .is_err(),
            "an unencoded + must still be refused; it decodes to a space"
        );
    }

    #[test]
    fn every_tab_and_narrowing_filter_parses() {
        let q = list(
            "org_id=00000000-0000-0000-0000-000000000001\
             &folder_id=00000000-0000-0000-0000-000000000002\
             &category_id=00000000-0000-0000-0000-000000000003\
             &review_status=in_review&deleted=true&favorited=true&pinned=true&limit=25",
        )
        .expect("parse every filter at once");
        assert!(q.deleted && q.favorited && q.pinned);
        assert_eq!(q.review_status.as_deref(), Some("in_review"));
        assert_eq!(q.limit, 25);
        assert!(q.folder_id.is_some() && q.category_id.is_some());
    }

    /// The org is not optional, and the refusal has to happen at the boundary.
    /// A default here would let a listing run against a nil uuid and answer an
    /// empty library rather than a bad request.
    #[test]
    fn a_listing_without_an_org_is_refused() {
        assert!(list("folder_id=00000000-0000-0000-0000-000000000002").is_err());
    }

    #[test]
    fn the_search_and_folder_queries_parse_too() {
        let uri: Uri =
            "/api/documents/search?org_id=00000000-0000-0000-0000-000000000001&q=sanitizer"
                .parse()
                .unwrap();
        let Query(s) =
            Query::<crate::server::api::documents::search::SearchQuery>::try_from_uri(&uri)
                .expect("search query");
        assert_eq!(s.q, "sanitizer");
        assert_eq!(s.limit, 50);

        let uri: Uri = "/api/document-folders?org_id=00000000-0000-0000-0000-000000000001"
            .parse()
            .unwrap();
        assert!(Query::<FolderQuery>::try_from_uri(&uri).is_ok());
    }

    /// Absent, explicit null and a value must be three different things.
    ///
    /// The bug this pins needed no database and no HTTP: `Option<Option<T>>`
    /// without the adapter parses absent and `null` to the same `None`, so
    /// `Some(None)` never occurs and every clearing path downstream is dead.
    /// It shipped under a doc comment describing the behaviour it did not have,
    /// which is exactly the kind of claim a two-line serde test settles.
    #[test]
    fn absent_and_explicit_null_are_different_requests() {
        let absent: UpdateDocument = serde_json::from_str(r#"{"title":"Handbook"}"#).unwrap();
        assert_eq!(absent.folder_id, None, "absent must mean leave it alone");

        let cleared: UpdateDocument = serde_json::from_str(r#"{"folder_id":null}"#).unwrap();
        assert_eq!(
            cleared.folder_id,
            Some(None),
            "explicit null must mean move it to the root"
        );

        let moved: UpdateDocument =
            serde_json::from_str(r#"{"folder_id":"00000000-0000-0000-0000-000000000001"}"#)
                .unwrap();
        assert!(matches!(moved.folder_id, Some(Some(_))));

        // The same three for every other doubly-wrapped field, since each one
        // needs its own attribute and forgetting one is silent.
        let cleared: UpdateDocument =
            serde_json::from_str(r#"{"category_id":null,"location_id":null,"expires_at":null}"#)
                .unwrap();
        assert_eq!(cleared.category_id, Some(None));
        assert_eq!(cleared.location_id, Some(None));
        assert_eq!(cleared.expires_at, Some(None));

        let cleared: UpdateFolder = serde_json::from_str(r#"{"parent_id":null}"#).unwrap();
        assert_eq!(cleared.parent_id, Some(None));
    }

    /// A field nobody declared is a rejected request, not a silent drop. The
    /// document DTOs learned this the expensive way; the folder ones inherit it.
    #[test]
    fn an_undeclared_field_is_refused_rather_than_dropped() {
        assert!(serde_json::from_str::<CreateFolder>(r#"{"name":"Ops","colour":"red"}"#).is_err());
        assert!(serde_json::from_str::<UpdateFolder>(r#"{"colour":"red"}"#).is_err());
        assert!(
            serde_json::from_str::<CreateDocument>(
                r#"{"title":"a","kind":"file","categoryId":null}"#
            )
            .is_err(),
            "camelCase is the typo that started this"
        );
    }
}
