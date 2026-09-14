//! Categories and the review queue.
//!
//! Two features that look like plumbing and carry one real hazard between them:
//! a category is a tab, a review state is a workflow, and neither may become a
//! second way to decide who reads a document. The filter tests here are the
//! ones that matter — narrowing by category or by review state must compose
//! with the visibility filter rather than replace it.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(document_compliance)'`

use chrono::Utc;
use entity::{document_categories, documents};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use oxy_app::server::api::documents::categories::is_duplicate_name;
use oxy_app::server::api::documents::visibility::{
    Trash, resolve_standing, visible_documents, visible_documents_scoped,
};

use crate::common::{Schema, fresh_db};
use crate::documents::{doc, seed_tenant};

async fn category(db: &DatabaseConnection, org: Uuid, name: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    document_categories::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        name: ActiveValue::Set(name.to_string()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed category");
    id
}

async fn put_in(db: &DatabaseConnection, doc_id: Uuid, cat: Option<Uuid>) {
    let mut am: documents::ActiveModel = documents::Entity::find_by_id(doc_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.category_id = ActiveValue::Set(cat);
    am.update(db).await.expect("file into a category");
}

/// The name is unique inside an org and free outside it. Two tabs with the same
/// name is a support ticket; two tenants using the word "Lease" is normal.
#[tokio::test]
async fn a_category_name_is_unique_per_org_and_not_beyond() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_tenant(&db, "acme").await;
    let b = seed_tenant(&db, "beta").await;

    category(&db, a.org, "Lease").await;
    category(&db, b.org, "Lease").await; // a different tenant: fine

    let clash = document_categories::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(a.org),
        name: ActiveValue::Set("Lease".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        updated_at: ActiveValue::Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(&db)
    .await;

    let err = clash.expect_err("a duplicate category name was accepted");

    // `is_err()` alone is not enough, and this is the assertion that would have
    // caught the bug it replaces: the handler decides between `409` and `500`
    // by recognising this error, and it recognised it by looking for the
    // SQLSTATE `23505` — which sea-orm's `Display` never emits for an insert.
    // So a duplicate answered `500` and put a raw `POST … → 500` in front of
    // the person who typed the name, while this test stayed green.
    assert!(
        is_duplicate_name(&err),
        "the handler cannot tell this is a duplicate, so it will answer 500: {err}"
    );
}

/// Deleting a category leaves its documents alone and uncategorised. This is
/// the difference from `location_id`, where the same delete is refused because
/// it would widen who can read the document.
#[tokio::test]
async fn deleting_a_category_leaves_its_documents_uncategorised() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let cat = category(&db, t.org, "Insurance").await;
    let id = doc(&db, t.org, "COI 2026", "org", None, true, t.officer).await;
    put_in(&db, id, Some(cat)).await;

    document_categories::Entity::delete_by_id(cat)
        .exec(&db)
        .await
        .expect("a category with documents must still be deletable");

    let after = documents::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .expect("the document survives its category");
    assert_eq!(after.category_id, None);
}

/// A decision records when it was made, or it is not a decision. The schema
/// says so; this is the assertion that the schema is actually saying it.
#[tokio::test]
async fn a_review_decision_must_carry_its_timestamp() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(&db, t.org, "Health permit", "org", None, true, t.officer).await;

    let set = |status: &'static str, at: bool| {
        let db = &db;
        async move {
            let mut am: documents::ActiveModel = documents::Entity::find_by_id(id)
                .one(db)
                .await
                .unwrap()
                .unwrap()
                .into();
            am.review_status = ActiveValue::Set(Some(status.into()));
            am.reviewed_at = ActiveValue::Set(at.then(|| Utc::now().fixed_offset()));
            am.update(db).await
        }
    };

    // Named, not merely `is_err()`. A different constraint failing would
    // satisfy a bare `is_err()` and the test would pass for the wrong reason.
    for (status, at, why) in [
        ("approved", false, "approved with no timestamp"),
        ("in_review", true, "undecided with a timestamp"),
    ] {
        let err = set(status, at).await.expect_err(why).to_string();
        assert!(
            err.contains("documents_review_decision_is_whole"),
            "{why}: refused by something else — {err}"
        );
    }

    // The two shapes that are real.
    assert!(set("in_review", false).await.is_ok());
    let approved = set("approved", true).await.expect("approve");
    assert!(approved.is_approved());
    assert!(!approved.awaits_review());
}

/// The load-bearing one. Narrowing by category or by review state must compose
/// with the visibility filter, never replace it — otherwise `?category_id=` is
/// a way to read an `hq` document by asking for it under a tab.
#[tokio::test]
async fn a_compliance_filter_narrows_the_visible_set_and_cannot_widen_it() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let cat = category(&db, t.org, "Insurance").await;

    let hq_doc = doc(&db, t.org, "Board COI", "hq", None, true, t.officer).await;
    let open_doc = doc(&db, t.org, "Store COI", "org", None, true, t.officer).await;
    for d in [hq_doc, open_doc] {
        put_in(&db, d, Some(cat)).await;
        let mut am: documents::ActiveModel = documents::Entity::find_by_id(d)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .into();
        am.review_status = ActiveValue::Set(Some("in_review".into()));
        am.update(&db).await.unwrap();
    }

    let in_category = |caller: Uuid| {
        let db = &db;
        let org = t.org;
        async move {
            let standing = resolve_standing(db, caller, org).await.unwrap();
            let filter = visible_documents_scoped(org, caller, &standing, Trash::Excluded)
                .expect("standing")
                .add(documents::Column::CategoryId.eq(cat))
                .add(documents::Column::ReviewStatus.eq("in_review"));
            documents::Entity::find()
                .filter(filter)
                .all(db)
                .await
                .unwrap()
                .into_iter()
                .map(|d| d.id)
                .collect::<Vec<_>>()
        }
    };

    let officer = in_category(t.officer).await;
    assert!(
        officer.contains(&hq_doc) && officer.contains(&open_doc),
        "both are in the category and in review, so the filter itself matches both"
    );

    let worker = in_category(t.worker_a).await;
    assert_eq!(
        worker,
        vec![open_doc],
        "a compliance filter handed a worker an hq document"
    );
}

/// A category tab does not tell a worker how much hq material it holds.
///
/// The folder-count fix landed and this one did not, under a comment still
/// citing the rationale that fix removed. A category is the same disclosure
/// with the same shape: an `hq` memo, another store's SOP and an unpublished
/// draft all counted toward a tab a frontline worker reads, who then opens it
/// and finds one row.
#[tokio::test]
async fn a_category_count_is_what_the_caller_could_actually_open() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let cat = category(&db, t.org, "Litigation").await;

    // Four documents, one of which this worker may read.
    for (title, vis, published) in [
        ("Board memo", "hq", true),
        ("Store SOP", "org", true),
        ("Unpublished", "org", false),
        ("Second memo", "hq", true),
    ] {
        let id = doc(&db, t.org, title, vis, None, published, t.officer).await;
        put_in(&db, id, Some(cat)).await;
    }

    let count_for = |caller: Uuid| {
        let db = &db;
        let org = t.org;
        async move {
            let standing = resolve_standing(db, caller, org).await.unwrap();
            let Some(visible) = visible_documents(org, caller, &standing) else {
                return 0usize;
            };
            documents::Entity::find()
                .filter(visible)
                .filter(documents::Column::CategoryId.eq(cat))
                .all(db)
                .await
                .unwrap()
                .len()
        }
    };

    assert_eq!(count_for(t.officer).await, 4, "an officer sees all four");
    assert_eq!(
        count_for(t.worker_a).await,
        1,
        "the tab told a worker how many hq documents and drafts it holds"
    );
}
