//! The document model's read gates.
//!
//! `visible_documents` is the only thing standing between a store's staff and
//! the rest of the tenant's library, and it is SQL — so the proof has to be a
//! real query against real rows, not a unit test of a predicate that resembles
//! it. Each case below resolves standing the way the handlers do and runs the
//! condition they run.
//!
//! The failure class these exist for is the one the assignment graph and chat
//! both shipped once: a filter that is correct for the caller it was written
//! against and open for the caller it was not.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(documents)'`

use chrono::Utc;
use entity::{
    admin_assume_sessions, app_admins, document_versions, documents, folders, locations,
    org_frontline_members, org_members, org_role_members, org_roles, organizations, users,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use oxy_app::server::api::documents::hydrate;
use oxy_app::server::api::documents::manage::{gate_refs, is_descendant_of, owned_folder_scoped};
use oxy_app::server::api::documents::visibility::{
    ReadStanding, Trash, resolve_standing, visible_documents, visible_documents_scoped,
    visible_folders, visible_folders_scoped,
};

use crate::common::{Schema, fresh_db};

/// One tenant: two stores, an officer, a plain member, and a worker rostered at
/// the first store only.
pub(crate) struct Tenant {
    pub org: Uuid,
    pub store_a: Uuid,
    pub store_b: Uuid,
    pub officer: Uuid,
    #[allow(dead_code)]
    pub member: Uuid,
    /// Enrolled by PIN and rostered at `store_a`. Holds no `org_members` row,
    /// which is the entire point of the fixture.
    pub worker_a: Uuid,
    /// Enrolled, but rostered nowhere.
    #[allow(dead_code)]
    pub worker_unplaced: Uuid,
}

async fn user(db: &DatabaseConnection, label: &str) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("{label}-{id}@example.com"))),
        name: ActiveValue::Set(label.to_string()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

async fn location(db: &DatabaseConnection, org: Uuid, name: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    locations::ActiveModel {
        // `kind` and `parent_id` arrived with main's operating graph. Left
        // NULL: a document's visibility joins `locations` on identity alone,
        // so a store's place in that hierarchy is not a fact these tests are
        // about, and inventing one would be seeding a claim they do not test.
        kind: ActiveValue::Set(None),
        parent_id: ActiveValue::Set(None),
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        name: ActiveValue::Set(format!("{name}-{id}")),
        status: ActiveValue::Set("open".into()),
        timezone: ActiveValue::Set("UTC".into()),
        external_id: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed location");
    id
}

async fn enrol(db: &DatabaseConnection, org: Uuid, u: Uuid, at: Option<Uuid>, role: Uuid) {
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(u),
        status: ActiveValue::Set("active".into()),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed frontline enrolment");

    if let Some(loc) = at {
        org_role_members::ActiveModel {
            // Also from the operating graph. A roster row here exists to give a
            // worker standing at a store; who supervises them is a different
            // question and no read filter consults it.
            supervisor_id: ActiveValue::Set(None),
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(org),
            role_id: ActiveValue::Set(role),
            user_id: ActiveValue::Set(u),
            location_id: ActiveValue::Set(Some(loc)),
            created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        }
        .insert(db)
        .await
        .expect("seed roster");
    }
}

pub(crate) async fn seed_tenant(db: &DatabaseConnection, label: &str) -> Tenant {
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set(format!("{label} Co")),
        slug: ActiveValue::Set(format!("{label}-{org}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");

    let now = Utc::now().fixed_offset();
    let role = Uuid::new_v4();
    org_roles::ActiveModel {
        id: ActiveValue::Set(role),
        org_id: ActiveValue::Set(org),
        name: ActiveValue::Set(format!("Shift Lead {role}")),
        scope: ActiveValue::Set("location".into()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed role");

    let officer = user(db, "officer").await;
    let member = user(db, "member").await;
    for (u, r) in [
        (officer, org_members::OrgRole::Admin),
        (member, org_members::OrgRole::Member),
    ] {
        org_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(org),
            user_id: ActiveValue::Set(u),
            role: ActiveValue::Set(r),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("seed membership");
    }

    let store_a = location(db, org, "store-a").await;
    let store_b = location(db, org, "store-b").await;

    let worker_a = user(db, "worker-a").await;
    enrol(db, org, worker_a, Some(store_a), role).await;
    let worker_unplaced = user(db, "worker-unplaced").await;
    enrol(db, org, worker_unplaced, None, role).await;

    Tenant {
        org,
        store_a,
        store_b,
        officer,
        member,
        worker_a,
        worker_unplaced,
    }
}

/// A published document. Publishing needs a version — the schema refuses the
/// combination otherwise, which is itself worth having exercised here.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn doc(
    db: &DatabaseConnection,
    org: Uuid,
    title: &str,
    visibility: &str,
    at: Option<Uuid>,
    published: bool,
    author: Uuid,
) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    documents::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        folder_id: ActiveValue::Set(None),
        title: ActiveValue::Set(title.to_string()),
        kind: ActiveValue::Set("chapter".into()),
        status: ActiveValue::Set("draft".into()),
        visibility: ActiveValue::Set(visibility.to_string()),
        location_id: ActiveValue::Set(at),
        expires_at: ActiveValue::Set(None),
        current_version_id: ActiveValue::Set(None),
        created_by: ActiveValue::Set(Some(author)),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        deleted_at: ActiveValue::Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed document");

    if published {
        let version = Uuid::new_v4();
        document_versions::ActiveModel {
            id: ActiveValue::Set(version),
            document_id: ActiveValue::Set(id),
            version_no: ActiveValue::Set(1),
            author_id: ActiveValue::Set(Some(author)),
            body: ActiveValue::Set(Some(format!("# {title}"))),
            object_key: ActiveValue::Set(None),
            content_type: ActiveValue::Set(Some("text/markdown".into())),
            size_bytes: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now),
        }
        .insert(db)
        .await
        .expect("seed version");

        let mut am: documents::ActiveModel = documents::Entity::find_by_id(id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .into();
        am.status = ActiveValue::Set("published".into());
        am.current_version_id = ActiveValue::Set(Some(version));
        am.update(db).await.expect("publish");
    }
    id
}

/// Exactly what a handler does: resolve standing, build the filter, run it.
/// Going through the real condition is the point — a test that re-implemented
/// the rule in Rust would pass while the SQL leaked.
pub(crate) async fn visible_to(db: &DatabaseConnection, org: Uuid, caller: Uuid) -> Vec<Uuid> {
    let standing = resolve_standing(db, caller, org).await.expect("standing");
    let Some(filter) = visible_documents(org, caller, &standing) else {
        return vec![];
    };
    documents::Entity::find()
        .filter(filter)
        .all(db)
        .await
        .expect("query")
        .into_iter()
        .map(|d| d.id)
        .collect()
}

/// The baseline. Without it every refusal below could be passing because the
/// filter returns nothing to anybody.
#[tokio::test]
async fn a_member_sees_the_whole_published_library() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let org_wide = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;
    let at_a = doc(
        &db,
        t.org,
        "Store A permit",
        "org",
        Some(t.store_a),
        true,
        t.officer,
    )
    .await;
    let hq = doc(&db, t.org, "Board pack", "hq", None, true, t.officer).await;

    let seen = visible_to(&db, t.org, t.member).await;
    for id in [org_wide, at_a, hq] {
        assert!(
            seen.contains(&id),
            "a member should see every published row"
        );
    }
}

/// The case the whole model exists for. A worker rostered at one store must not
/// read another store's documents — and the refusal has to come from the query,
/// because nothing above it knows which stores this person works at.
#[tokio::test]
async fn a_worker_at_one_store_cannot_read_another_stores_documents() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let at_a = doc(
        &db,
        t.org,
        "A permit",
        "org",
        Some(t.store_a),
        true,
        t.officer,
    )
    .await;
    let at_b = doc(
        &db,
        t.org,
        "B permit",
        "org",
        Some(t.store_b),
        true,
        t.officer,
    )
    .await;
    let org_wide = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;

    let seen = visible_to(&db, t.org, t.worker_a).await;
    assert!(seen.contains(&at_a), "their own store");
    assert!(seen.contains(&org_wide), "the org-wide handbook");
    assert!(
        !seen.contains(&at_b),
        "a worker rostered at store A read store B's document"
    );
}

/// `hq` is the column's entire reason to exist: head-office material a store's
/// staff must not see, even when it is org-wide in every other sense.
#[tokio::test]
async fn a_frontline_worker_never_sees_an_hq_document() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let hq = doc(&db, t.org, "Board pack", "hq", None, true, t.officer).await;
    let ordinary = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;

    let seen = visible_to(&db, t.org, t.worker_a).await;
    assert!(seen.contains(&ordinary));
    assert!(!seen.contains(&hq), "a worker read an hq document");
}

/// Enrolled but rostered nowhere still sees the org-wide handbook, and only
/// that. The empty-roster case is easy to write as "sees nothing", which would
/// make a newly enrolled worker's first screen blank.
#[tokio::test]
async fn an_unplaced_worker_sees_org_wide_documents_only() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let org_wide = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;
    let at_a = doc(
        &db,
        t.org,
        "A permit",
        "org",
        Some(t.store_a),
        true,
        t.officer,
    )
    .await;

    let seen = visible_to(&db, t.org, t.worker_unplaced).await;
    assert!(seen.contains(&org_wide));
    assert!(!seen.contains(&at_a));
}

/// A draft belongs to whoever is writing it, and to the officers who run the
/// library. Not to the org, and never to a worker.
#[tokio::test]
async fn a_draft_is_visible_to_its_author_and_to_officers_and_to_nobody_else() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let members_draft = doc(&db, t.org, "Half-written", "org", None, false, t.member).await;

    assert!(
        visible_to(&db, t.org, t.member)
            .await
            .contains(&members_draft)
    );
    assert!(
        visible_to(&db, t.org, t.officer)
            .await
            .contains(&members_draft),
        "an officer runs the Drafts tab"
    );
    assert!(
        !visible_to(&db, t.org, t.worker_a)
            .await
            .contains(&members_draft),
        "a worker read an unpublished draft"
    );

    let officers_draft = doc(&db, t.org, "Officer note", "org", None, false, t.officer).await;
    assert!(
        !visible_to(&db, t.org, t.member)
            .await
            .contains(&officers_draft),
        "a plain member read somebody else's draft"
    );
}

/// The cross-tenant case. Standing is resolved per org, so a full officer of
/// one tenant has none in another and the filter is never even built.
#[tokio::test]
async fn a_stranger_to_the_org_reaches_nothing_in_it() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_tenant(&db, "acme").await;
    let b = seed_tenant(&db, "beta").await;

    doc(&db, a.org, "Acme handbook", "org", None, true, a.officer).await;

    assert_eq!(
        resolve_standing(&db, b.officer, a.org).await.unwrap(),
        ReadStanding::None
    );
    assert!(visible_to(&db, a.org, b.officer).await.is_empty());
    assert!(visible_folders(a.org, &ReadStanding::None).is_none());
}

/// Trashed rows leave every listing. The "Deleted" tab reads them back by
/// asking for them explicitly, which is a different query from this one.
#[tokio::test]
async fn a_trashed_document_leaves_every_listing() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;

    let mut am: documents::ActiveModel = documents::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.deleted_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();

    assert!(!visible_to(&db, t.org, t.officer).await.contains(&id));
    assert!(!visible_to(&db, t.org, t.worker_a).await.contains(&id));
}

/// The write side. `OrgAdmin` proves the caller runs the org on the path and
/// nothing about the ids they sent with it — the hole the assignment graph
/// shipped, in the same shape.
#[tokio::test]
async fn a_write_cannot_borrow_another_tenants_folder_or_location() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_tenant(&db, "acme").await;
    let b = seed_tenant(&db, "beta").await;

    let now = Utc::now().fixed_offset();
    let b_folder = Uuid::new_v4();
    folders::ActiveModel {
        id: ActiveValue::Set(b_folder),
        org_id: ActiveValue::Set(b.org),
        parent_id: ActiveValue::Set(None),
        name: ActiveValue::Set("Beta SOPs".into()),
        visibility: ActiveValue::Set("org".into()),
        created_by: ActiveValue::Set(Some(b.officer)),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        deleted_at: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed folder");

    // Wholly inside one org: accepted. Without this the two refusals could be
    // passing because the gate refuses everything.
    assert!(
        gate_refs(&db, a.org, None, Some(a.store_a), None)
            .await
            .is_ok()
    );

    // The CODE, not the status. Every refusal in this gate is a `400`, so
    // asserting the status would pass for the wrong rule — and the code is what
    // the client branches on: it re-reads its folder tree on `folder_missing`
    // and does not on the others.
    assert_eq!(
        refused(gate_refs(&db, a.org, Some(b_folder), None, None).await),
        Some("folder_missing"),
        "a document was filed into another tenant's folder"
    );
    assert_eq!(
        refused(gate_refs(&db, a.org, None, Some(b.store_b), None).await),
        Some("location_missing"),
        "a document was scoped to another tenant's store"
    );

    // The third id, added late and for a bad reason: `category_id` was missing
    // from the create DTO entirely, so nothing could reach this gate because
    // nothing could set a category at all. Serde dropped the field, the write
    // answered 201, and the document came back uncategorised — silently.
    let b_cat = Uuid::new_v4();
    entity::document_categories::ActiveModel {
        id: ActiveValue::Set(b_cat),
        org_id: ActiveValue::Set(b.org),
        name: ActiveValue::Set("Beta Permits".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed category");

    assert_eq!(
        refused(gate_refs(&db, a.org, None, None, Some(b_cat)).await),
        Some("category_missing"),
        "a document was filed under another tenant's category"
    );
}

/// Which rule a refusal names, or `None` when it was accepted.
///
/// A helper rather than a method, so a refusal that stops carrying a code shows
/// up here as `Some(..) != None` rather than as a passing assertion about a
/// status every one of them shares.
fn refused(
    r: Result<(), oxy_app::server::api::documents::manage::Refusal>,
) -> Option<&'static str> {
    match r {
        Ok(()) => None,
        Err(e) => {
            assert_eq!(
                e.status(),
                axum::http::StatusCode::BAD_REQUEST,
                "a cross-tenant reference must be a 400, not a 404 — the caller \
                 is an officer of the org on the path and can see it exists"
            );
            Some(e.code().expect("every refusal in this gate names its rule"))
        }
    }
}

/// The trash opens for officers and for nobody else.
///
/// The restore endpoint has existed since the first PR and nothing could reach
/// it, because every read filter hid deleted rows. This is the tab that reaches
/// it, and the assertion that it is not a second way into the library.
#[tokio::test]
async fn only_an_officer_sees_the_trash() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(&db, t.org, "Old handbook", "org", None, true, t.officer).await;

    let mut am: documents::ActiveModel = documents::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.deleted_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();

    let trashed = |caller: Uuid| {
        let db = &db;
        let org = t.org;
        async move {
            let standing = resolve_standing(db, caller, org).await.expect("standing");
            let filter = visible_documents_scoped(org, caller, &standing, Trash::Only)
                .expect("a filter for a caller with standing");
            documents::Entity::find()
                .filter(filter)
                .all(db)
                .await
                .expect("query")
                .into_iter()
                .map(|d| d.id)
                .collect::<Vec<_>>()
        }
    };

    assert!(trashed(t.officer).await.contains(&id), "the Deleted tab");
    assert!(
        !trashed(t.member).await.contains(&id),
        "a plain member reached the trash"
    );
    assert!(
        !trashed(t.worker_a).await.contains(&id),
        "a frontline worker reached the trash"
    );
    // And the live listing still hides it, so the two tabs are not the same
    // query wearing different labels.
    assert!(!visible_to(&db, t.org, t.officer).await.contains(&id));
}

/// A listing carries the names a screen prints, not the uuids a table stores.
#[tokio::test]
async fn a_listing_carries_the_names_a_screen_shows() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(
        &db,
        t.org,
        "Store A permit",
        "org",
        Some(t.store_a),
        true,
        t.officer,
    )
    .await;

    let rows = documents::Entity::find_by_id(id).all(&db).await.unwrap();
    let out = hydrate::summaries(&db, t.officer, rows)
        .await
        .expect("hydrate");
    let s = out.first().expect("one row");

    assert_eq!(s.author_name.as_deref(), Some("officer"));
    assert!(s.location_name.is_some(), "the store's name is missing");
    assert_eq!(s.version_no, Some(1), "the current version's number");
    assert_eq!(s.content_type.as_deref(), Some("text/markdown"));
}

/// Folder counts skip the trash, and are the same number for everyone.
#[tokio::test]
async fn folder_counts_skip_the_trash() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let officer_standing = resolve_standing(&db, t.officer, t.org).await.unwrap();

    let now = Utc::now().fixed_offset();
    let folder = Uuid::new_v4();
    folders::ActiveModel {
        id: ActiveValue::Set(folder),
        org_id: ActiveValue::Set(t.org),
        parent_id: ActiveValue::Set(None),
        name: ActiveValue::Set("Back of House".into()),
        visibility: ActiveValue::Set("org".into()),
        created_by: ActiveValue::Set(Some(t.officer)),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        deleted_at: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed folder");

    let mut ids = vec![];
    for n in 0..3 {
        let d = doc(
            &db,
            t.org,
            &format!("SOP {n}"),
            "org",
            None,
            true,
            t.officer,
        )
        .await;
        let mut am: documents::ActiveModel = documents::Entity::find_by_id(d)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .into();
        am.folder_id = ActiveValue::Set(Some(folder));
        am.update(&db).await.unwrap();
        ids.push(d);
    }

    assert_eq!(
        hydrate::folder_counts(&db, t.org, t.officer, &officer_standing)
            .await
            .unwrap()
            .get(&folder),
        Some(&3)
    );

    // Trash one. The card must stop counting it, or the folder claims contents
    // nobody can open.
    let mut am: documents::ActiveModel = documents::Entity::find_by_id(ids[0])
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.deleted_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();

    assert_eq!(
        hydrate::folder_counts(&db, t.org, t.officer, &officer_standing)
            .await
            .unwrap()
            .get(&folder),
        Some(&2)
    );
}

/// Trashing a folder leaves its documents readable — and pointing at a folder
/// nobody can list any more.
///
/// The first half is the deliberate part: a compliance library must not vanish
/// because somebody tidied a folder. The second half is the cost of it, and the
/// reason this test exists rather than a comment — a reader that filters "in
/// this folder" and "filed nowhere" finds such a document in neither, and it
/// renders under nothing. One was lost that way in the Store Ops app, which now
/// treats a document whose folder it cannot see as top-level.
///
/// If this ever starts failing because `trash_folder` clears `folder_id`, the
/// thing to check is `restore_folder`: keeping the id is the only reason a
/// restore can put the documents back where they were.
#[tokio::test]
async fn trashing_a_folder_keeps_its_documents_and_keeps_their_filing() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let f = folder(&db, t.org, "Back of House", "org", t.officer).await;

    let id = doc(
        &db,
        t.org,
        "Opening Checklist",
        "org",
        None,
        true,
        t.officer,
    )
    .await;
    let mut am: documents::ActiveModel = documents::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.folder_id = ActiveValue::Set(Some(f));
    am.update(&db).await.unwrap();

    // Trash the folder, the way the handler does.
    let mut am: folders::ActiveModel = folders::Entity::find_by_id(f)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.deleted_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();

    let standing = resolve_standing(&db, t.officer, t.org).await.unwrap();
    let rows = documents::Entity::find()
        .filter(
            visible_documents_scoped(t.org, t.officer, &standing, Trash::Excluded)
                .expect("standing"),
        )
        .all(&db)
        .await
        .unwrap();

    let found = rows
        .iter()
        .find(|d| d.id == id)
        .expect("trashing a folder took its documents with it");

    // Still filed, and filed into a folder the tree no longer shows. Both
    // halves matter: the first is what makes a restore lossless, the second is
    // what a client has to handle.
    assert_eq!(found.folder_id, Some(f));
    let listable = folders::Entity::find()
        .filter(folders::Column::OrgId.eq(t.org))
        .filter(folders::Column::DeletedAt.is_null())
        .all(&db)
        .await
        .unwrap();
    assert!(
        !listable.iter().any(|x| x.id == f),
        "the trashed folder is still in the listing, so nothing is stranded and this test proves nothing"
    );
}

/// Suspending a worker takes their access, even when they also hold a role.
///
/// `worker_a` is the shape that broke it: enrolled by PIN *and* on a roster.
/// The old resolver filtered the enrolment query on `status = 'active'` and
/// then asked `roles.is_empty() && !enrolled` — so suspension only made the row
/// absent, absent is what a never-enrolled roleholder looks like, and the role
/// carried them straight past the guard. They kept the handbook after somebody
/// had deliberately taken it away.
///
/// Asserted on both sides so the test cannot pass by denying everyone.
#[tokio::test]
async fn suspending_a_worker_outranks_the_role_they_hold() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;

    assert!(
        visible_to(&db, t.org, t.worker_a).await.contains(&id),
        "an active worker reads the handbook — otherwise this test proves nothing"
    );

    // Through main's own writer, not by hand. The merge asserted that
    // `set_worker_standing`'s vocabulary and this filter's `!= "active"` branch
    // agree; writing `"suspended"` into the row myself would have tested my
    // spelling against my spelling and held the two apart forever.
    oxy_auth::frontline::set_worker_standing(&db, t.org, t.worker_a, false)
        .await
        .expect("suspend through the shipped route");

    // The roster row is deliberately left in place: it is the thing that used
    // to rescue them.
    assert!(
        !org_role_members::Entity::find()
            .filter(org_role_members::Column::OrgId.eq(t.org))
            .filter(org_role_members::Column::UserId.eq(t.worker_a))
            .all(&db)
            .await
            .unwrap()
            .is_empty(),
        "the role must still be there, or the fix is not what is under test"
    );

    assert!(
        matches!(
            resolve_standing(&db, t.worker_a, t.org).await.unwrap(),
            ReadStanding::None
        ),
        "a suspended worker still had standing"
    );
    assert!(
        visible_to(&db, t.org, t.worker_a).await.is_empty(),
        "a suspended worker still read the library"
    );

    // And reinstating gives it back — the half nothing covered, so a
    // suspension that could not be undone would have shipped unnoticed.
    oxy_auth::frontline::set_worker_standing(&db, t.org, t.worker_a, true)
        .await
        .expect("reinstate through the shipped route");
    assert!(
        visible_to(&db, t.org, t.worker_a).await.contains(&id),
        "a reinstated worker did not get the library back"
    );
}

/// A trashed folder is listable by an officer and by nobody else.
///
/// Without this the folder trash was write-only: the app offered "move this
/// folder to the trash" and `restore_folder` had no way to be reached, because
/// nothing could list what was in there. The policy matches documents exactly —
/// a non-officer asking for the trash gets their ordinary tree rather than a
/// refusal, so the request cannot be used to learn that a trash exists.
#[tokio::test]
async fn a_trashed_folder_is_listable_by_an_officer_and_by_nobody_else() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let live = folder(&db, t.org, "Front of House", "org", t.officer).await;
    let gone = folder(&db, t.org, "Retired", "org", t.officer).await;

    let mut am: folders::ActiveModel = folders::Entity::find_by_id(gone)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.deleted_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();

    let listed = |caller: Uuid, trash: Trash| {
        let db = &db;
        let org = t.org;
        async move {
            let standing = resolve_standing(db, caller, org).await.unwrap();
            let Some(filter) = visible_folders_scoped(org, &standing, trash) else {
                return vec![];
            };
            folders::Entity::find()
                .filter(filter)
                .all(db)
                .await
                .unwrap()
                .into_iter()
                .map(|f| f.id)
                .collect::<Vec<_>>()
        }
    };

    assert_eq!(
        listed(t.officer, Trash::Only).await,
        vec![gone],
        "an officer cannot see what is in the folder trash"
    );
    assert_eq!(
        listed(t.officer, Trash::Excluded).await,
        vec![live],
        "the ordinary tree must not include the trash"
    );
    assert_eq!(
        listed(t.worker_a, Trash::Only).await,
        vec![live],
        "a worker asking for the trash must get their ordinary tree, not a refusal and not the trash"
    );
}

/// A folder cannot be moved under its own descendant.
///
/// The handler only refused `parent == self` and left deeper cycles to "the
/// tree is assembled" — code that does not exist anywhere in the repo. So
/// `A.parent = B; B.parent = A` was a `200`, and both folders and everything
/// filed in them dropped off every client at once, because they all walk down
/// from `parent_id IS NULL`.
///
/// Tested through `is_descendant_of` rather than the handler, which takes
/// extractors this suite cannot build; it is the whole of the new decision.
#[tokio::test]
async fn a_folder_cannot_be_moved_under_its_own_descendant() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let a = folder(&db, t.org, "Operations", "org", t.officer).await;
    let b = folder(&db, t.org, "Chillers", "org", t.officer).await;
    let c = folder(&db, t.org, "Deep", "org", t.officer).await;
    reparent(&db, b, Some(a)).await; // b under a
    reparent(&db, c, Some(b)).await; // c under b

    // The cycle the handler used to accept: a under its own grandchild.
    assert!(
        is_descendant_of(&db, t.org, c, a).await.unwrap(),
        "c is below a, so moving a under c is a cycle"
    );
    assert!(is_descendant_of(&db, t.org, b, a).await.unwrap());

    // The moves that are fine, so this cannot pass by refusing everything.
    assert!(
        !is_descendant_of(&db, t.org, a, c).await.unwrap(),
        "moving c under a is ordinary nesting"
    );
    let sibling = folder(&db, t.org, "Front", "org", t.officer).await;
    assert!(!is_descendant_of(&db, t.org, sibling, a).await.unwrap());

    // And it must terminate on data that is ALREADY cyclic — this shipped
    // accepting cycles, so a tenant can have one, and a check that hangs on the
    // input it exists to detect is worse than no check.
    reparent(&db, a, Some(c)).await;
    let done = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        is_descendant_of(&db, t.org, sibling, a),
    )
    .await;
    assert!(done.is_ok(), "the walk did not terminate on a cyclic tree");
}

/// Move a folder under a parent, bypassing the handler's guards. Used to build
/// the shapes the guards are supposed to refuse.
async fn reparent(db: &DatabaseConnection, id: Uuid, parent: Option<Uuid>) {
    let mut am: folders::ActiveModel = folders::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.parent_id = ActiveValue::Set(parent);
    am.update(db).await.expect("reparent");
}

/// A document cannot be filed into a folder that is in the trash.
///
/// `gate_refs` checked the folder belongs to the org and not that it still
/// exists, so a write naming a trashed folder was accepted. The document then
/// lands somewhere no tree shows it, and only the reader-side "treat an
/// unseeable folder as top-level" rule stops it disappearing outright — a
/// state the API was creating on input that looks entirely ordinary.
#[tokio::test]
async fn a_document_cannot_be_filed_into_a_trashed_folder() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let live = folder(&db, t.org, "Front of House", "org", t.officer).await;
    let gone = folder(&db, t.org, "Retired", "org", t.officer).await;

    // The live one is accepted, so this cannot pass by refusing everything.
    assert!(gate_refs(&db, t.org, Some(live), None, None).await.is_ok());

    let mut am: folders::ActiveModel = folders::Entity::find_by_id(gone)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.deleted_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();

    assert!(
        gate_refs(&db, t.org, Some(gone), None, None).await.is_err(),
        "a trashed folder was accepted as a filing destination"
    );
}

/// An operator inside an assume-role session can READ what they may write.
///
/// `OrgAdmin` lets Oxy staff and partners into a tenant through a synthesised
/// Owner membership that `org_context` mints for a live, audited session — the
/// only sanctioned way staff reach a tenant at all. `resolve_standing` began by
/// requiring a real `org_members` row, which such a caller by definition does
/// not have, so every write was accepted and every read answered `404`: a
/// support engineer could create, publish and trash a tenant's documents and
/// could not list or open one, including the one they had just written.
///
/// The negative half is the load-bearing one. Being staff is not enough — the
/// session has to be live — so the same user with no session, and with an
/// EXPIRED session, must still see nothing.
#[tokio::test]
async fn an_operator_in_a_live_assume_session_can_read_what_they_may_write() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(&db, t.org, "Health permit", "org", None, true, t.officer).await;

    // Staff: a real user with no membership in this org at all. `app_admins` is
    // keyed by EMAIL, so the grant has to name the address the user row carries.
    let staff = user(&db, "oxy-staff").await;
    let staff_email = users::Entity::find_by_id(staff)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .email
        .expect("the seeded user has an address");
    app_admins::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(staff_email.clone()),
        role: ActiveValue::Set("global_admin".into()),
        scope_all: ActiveValue::Set(true),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        updated_at: ActiveValue::Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed staff");

    let sees = |caller: Uuid| {
        let db = &db;
        let org = t.org;
        async move {
            let standing = resolve_standing(db, caller, org).await.unwrap();
            match visible_documents(org, caller, &standing) {
                None => vec![],
                Some(f) => documents::Entity::find()
                    .filter(f)
                    .all(db)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|d| d.id)
                    .collect(),
            }
        }
    };

    assert!(
        sees(staff).await.is_empty(),
        "staff with NO session must see nothing — being staff is not standing"
    );

    let session = |ends_in: i64| {
        let db = &db;
        let org = t.org;
        let staff_email = staff_email.clone();
        async move {
            admin_assume_sessions::Entity::delete_many()
                .exec(db)
                .await
                .unwrap();
            let now = Utc::now();
            admin_assume_sessions::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                actor_user_id: ActiveValue::Set(staff),
                actor_email: ActiveValue::Set(staff_email.clone()),
                org_id: ActiveValue::Set(org),
                reason: ActiveValue::Set("support".into()),
                started_at: ActiveValue::Set(now.fixed_offset()),
                expires_at: ActiveValue::Set(
                    (now + chrono::Duration::minutes(ends_in)).fixed_offset(),
                ),
                ended_at: ActiveValue::Set(None),
                ..Default::default()
            }
            .insert(db)
            .await
            .expect("seed session");
        }
    };

    session(60).await;
    assert!(
        sees(staff).await.contains(&id),
        "an operator in a live session still could not read the tenant's documents"
    );

    session(-1).await;
    assert!(
        sees(staff).await.is_empty(),
        "an EXPIRED session still granted reads — liveness is the whole gate"
    );

    // A REAL MEMBER who also holds a session keeps their real standing.
    //
    // `org_context` consults assume only when there is no membership, so
    // `OrgAdmin` judges such a person on their real role. Consulting assume
    // first here — which the first version of this fix did — made reads
    // disagree with writes in the worst direction: a plain Member was denied
    // the write on their real role and granted officer READS, which is every
    // draft and the trash. `assume::start` does not refuse a real member, so
    // this is reachable rather than theoretical.
    let inside = user(&db, "member-with-a-session").await;
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(t.org),
        user_id: ActiveValue::Set(inside),
        role: ActiveValue::Set(org_members::OrgRole::Member),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        updated_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(&db)
    .await
    .expect("seed a plain member");

    let member_email = users::Entity::find_by_id(inside)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .email
        .unwrap();
    app_admins::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(member_email.clone()),
        role: ActiveValue::Set("global_admin".into()),
        scope_all: ActiveValue::Set(true),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        updated_at: ActiveValue::Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("make them staff too");
    admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(inside),
        actor_email: ActiveValue::Set(member_email),
        org_id: ActiveValue::Set(t.org),
        reason: ActiveValue::Set("support".into()),
        started_at: ActiveValue::Set(Utc::now().fixed_offset()),
        expires_at: ActiveValue::Set((Utc::now() + chrono::Duration::minutes(60)).fixed_offset()),
        ended_at: ActiveValue::Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("and give them a live session");

    assert!(
        matches!(
            resolve_standing(&db, inside, t.org).await.unwrap(),
            ReadStanding::Member { is_officer: false }
        ),
        "a live session elevated a plain member to officer reads — every draft and the trash"
    );
}

/// A trashed folder is not an editable folder.
///
/// `gate_refs` refuses one as a filing DESTINATION; the subject of an edit was
/// unguarded, so a folder in the bin could still be renamed, re-parented and
/// re-scoped. Restoring it then brought back something other than what was
/// trashed, and re-parenting one under a live folder put a deleted container in
/// the middle of a live tree.
///
/// Asserted through `owned_folder_scoped`, which is the whole of the decision —
/// the handlers around it take extractors this suite cannot build.
#[tokio::test]
async fn a_trashed_folder_cannot_be_edited_but_can_be_restored() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let f = folder(&db, t.org, "Retired", "org", t.officer).await;

    // Live: editable, and not restorable — there is nothing to restore.
    assert!(
        owned_folder_scoped(&db, t.org, f, Trash::Excluded)
            .await
            .is_ok()
    );
    assert!(
        owned_folder_scoped(&db, t.org, f, Trash::Only)
            .await
            .is_err()
    );

    let mut am: folders::ActiveModel = folders::Entity::find_by_id(f)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.deleted_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();

    // Trashed: the two swap. Both directions asserted, so this cannot pass by
    // refusing everything.
    assert!(
        owned_folder_scoped(&db, t.org, f, Trash::Excluded)
            .await
            .is_err(),
        "a trashed folder was still editable"
    );
    assert!(
        owned_folder_scoped(&db, t.org, f, Trash::Only)
            .await
            .is_ok(),
        "a trashed folder could not be restored"
    );
}

/// A document cannot be filed at a place that has stores under it.
///
/// `#3113` made `locations` a tree. `visible_documents` matches `location_id`
/// literally against roster rows, so a document at "Northeast" is invisible to
/// every worker rostered at a Northeast store — a `201` and a document nobody
/// can read, discoverable only by counting.
///
/// The negative half matters as much: a CHILDLESS place with nobody rostered at
/// it must still be accepted, because that is every store on its first day and
/// refusing it would break opening one.
#[tokio::test]
async fn a_document_cannot_be_filed_at_a_place_that_has_children() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let region = location(&db, t.org, "Northeast").await;
    let store = location(&db, t.org, "Encinitas").await;
    let mut am: locations::ActiveModel = locations::Entity::find_by_id(store)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.parent_id = ActiveValue::Set(Some(region));
    am.update(&db).await.unwrap();

    assert!(
        gate_refs(&db, t.org, None, Some(region), None)
            .await
            .is_err(),
        "a place with stores under it and nobody rostered at it was accepted, so the \
         document would be unreadable"
    );

    // A DISTRICT MANAGER rostered at the region. `validate_targets` accepts any
    // location in the org for a location-scoped role — there is no leaf check —
    // so this is a supported assignment, `visible_documents` matches it, and
    // the guard must NOT refuse the write. The first version of this guard did,
    // on the false premise that no roster ever points at a container.
    let role = org_role_members::Entity::find()
        .filter(org_role_members::Column::OrgId.eq(t.org))
        .one(&db)
        .await
        .unwrap()
        .expect("the seeded roster row")
        .role_id;
    org_role_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(t.org),
        role_id: ActiveValue::Set(role),
        user_id: ActiveValue::Set(t.worker_a),
        location_id: ActiveValue::Set(Some(region)),
        supervisor_id: ActiveValue::Set(None),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(&db)
    .await
    .expect("roster somebody at the region");

    assert!(
        gate_refs(&db, t.org, None, Some(region), None)
            .await
            .is_ok(),
        "a place somebody is rostered at was refused — a district manager is a supported \
         assignment and can read a document filed there"
    );
    assert!(
        gate_refs(&db, t.org, None, Some(store), None).await.is_ok(),
        "a leaf store must still be accepted"
    );

    // The case the refusal must not catch: a brand-new store, no children and
    // nobody rostered at it yet.
    let fresh = location(&db, t.org, "Opening Next Month").await;
    assert!(
        gate_refs(&db, t.org, None, Some(fresh), None).await.is_ok(),
        "a store nobody is rostered at yet was refused — that is every store on day one"
    );
}

/// Paging walks every row exactly once, with no gaps and no repeats.
///
/// The listing capped at `limit` and said nothing, so a client holding the
/// first page could not learn there were more — the app invented a `truncated`
/// heuristic from the row count to cope. `offset` plus `Link: rel="next"` is
/// the answer the rest of this codebase already uses.
///
/// Asserted on the query rather than the handler, which takes extractors this
/// suite cannot build; the over-fetch and the ordering are the whole of it.
#[tokio::test]
async fn paging_the_library_visits_every_document_once() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    for n in 0..7 {
        doc(
            &db,
            t.org,
            &format!("SOP {n:02}"),
            "org",
            None,
            true,
            t.officer,
        )
        .await;
    }

    let standing = resolve_standing(&db, t.officer, t.org).await.unwrap();
    let filter = visible_documents(t.org, t.officer, &standing).expect("standing");

    // Three pages of three over seven rows: 3, 3, 1 — the last one short, which
    // is how the walk knows to stop.
    let mut seen: Vec<Uuid> = vec![];
    let mut offset = 0u64;
    let limit = 3u64;
    loop {
        let mut rows = documents::Entity::find()
            .filter(filter.clone())
            .order_by_desc(documents::Column::UpdatedAt)
            .order_by_desc(documents::Column::Id)
            .offset(offset)
            .limit(limit + 1)
            .all(&db)
            .await
            .unwrap();
        let more = rows.len() as u64 > limit;
        rows.truncate(limit as usize);
        seen.extend(rows.iter().map(|d| d.id));
        if !more {
            break;
        }
        offset += limit;
        assert!(offset < 100, "the walk did not terminate");
    }

    assert_eq!(seen.len(), 7, "the walk did not visit every document");
    let mut unique = seen.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 7, "a document was returned on two pages");
}

/// A folder, for the suites that need one. Shared with `document_search` so
/// both seed the same shape.
pub(crate) async fn folder(
    db: &DatabaseConnection,
    org: Uuid,
    name: &str,
    visibility: &str,
    author: Uuid,
) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    folders::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        parent_id: ActiveValue::Set(None),
        name: ActiveValue::Set(name.to_string()),
        visibility: ActiveValue::Set(visibility.to_string()),
        created_by: ActiveValue::Set(Some(author)),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        deleted_at: ActiveValue::Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed folder");
    id
}

/// A folder does not tell a worker how much head-office material it holds.
///
/// The count used to be org-wide, on the argument that a count is a property of
/// the folder. That argument does not survive `hq`: an `org`-visible folder
/// holding three head-office documents reported "3 items" to the audience `hq`
/// exists to exclude, who then opened it to an empty list. The number both
/// discloses that the material exists and contradicts the screen below it.
#[tokio::test]
async fn a_folder_count_is_what_the_caller_could_actually_open() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let f = folder(&db, t.org, "Operations", "org", t.officer).await;

    // One document everybody may read, two only officers may.
    for (title, vis) in [
        ("Opening checklist", "org"),
        ("Board pack", "hq"),
        ("Budget", "hq"),
    ] {
        let id = doc(&db, t.org, title, vis, None, true, t.officer).await;
        let mut am: documents::ActiveModel = documents::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .into();
        am.folder_id = ActiveValue::Set(Some(f));
        am.update(&db).await.unwrap();
    }

    let count_for = |caller: Uuid| {
        let db = &db;
        let org = t.org;
        async move {
            let standing = resolve_standing(db, caller, org).await.unwrap();
            hydrate::folder_counts(db, org, caller, &standing)
                .await
                .unwrap()
                .get(&f)
                .copied()
                .unwrap_or(0)
        }
    };

    assert_eq!(count_for(t.officer).await, 3, "an officer sees all three");
    assert_eq!(
        count_for(t.worker_a).await,
        1,
        "a worker was told how many hq documents the folder holds"
    );
}
