//! Favorites and Pinned.
//!
//! The two tabs exist because they are different acts by different people, and
//! these are the assertions that they actually behave that way: a favorite is
//! private to whoever made it, a pin is the same for everybody, and neither one
//! is a second way to decide who reads a document.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(document_shelf)'`

use chrono::Utc;
use entity::{document_favorites, documents, users};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use oxy_app::server::api::documents::hydrate;
use oxy_app::server::api::documents::shelf::{add_favorite, favorited_by};
use oxy_app::server::api::documents::visibility::{
    Trash, resolve_standing, visible_documents_scoped,
};

use crate::common::{Schema, fresh_db};
use crate::documents::{doc, seed_tenant};

async fn bookmark(db: &DatabaseConnection, user: Uuid, doc_id: Uuid) {
    document_favorites::ActiveModel {
        user_id: ActiveValue::Set(user),
        document_id: ActiveValue::Set(doc_id),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed favorite");
}

/// One row, hydrated as a given caller sees it.
async fn seen_by(
    db: &DatabaseConnection,
    caller: Uuid,
    id: Uuid,
) -> oxy_app::server::api::documents::dto::DocumentSummary {
    let rows = documents::Entity::find_by_id(id).all(db).await.unwrap();
    hydrate::summaries(db, caller, rows)
        .await
        .expect("hydrate")
        .pop()
        .expect("one row")
}

/// A bookmark belongs to the person who made it and to nobody else. If this
/// ever leaked, the Favorites tab would read as somebody else's reading list.
#[tokio::test]
async fn a_favorite_is_private_to_the_person_who_made_it() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let handbook = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;

    bookmark(&db, t.member, handbook).await;

    assert!(seen_by(&db, t.member, handbook).await.is_favorite);
    assert!(
        !seen_by(&db, t.officer, handbook).await.is_favorite,
        "one member's bookmark showed on another person's row"
    );
    assert!(!seen_by(&db, t.worker_a, handbook).await.is_favorite);
}

/// A pin is the org's shelf: set once by an officer, identical for every
/// viewer. That is the whole difference from a favorite.
#[tokio::test]
async fn a_pin_is_the_same_for_everyone() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let handbook = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;

    let mut am: documents::ActiveModel = documents::Entity::find_by_id(handbook)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.pinned_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.pinned_by = ActiveValue::Set(Some(t.officer));
    let pinned = am.update(&db).await.expect("pin");
    assert!(pinned.is_pinned());

    for who in [t.officer, t.member, t.worker_a] {
        assert!(
            seen_by(&db, who, handbook).await.pinned_at.is_some(),
            "the org's shelf looked different to somebody"
        );
    }
}

/// Deleting the officer who pinned something must delete the officer.
///
/// This replaces a test that asserted the opposite — that a pin with no author
/// is refused, by a `documents_pin_is_whole` CHECK. That CHECK was a trap this
/// repo had already documented and avoided once: `pinned_by` is
/// `ON DELETE SET NULL`, so the delete nulls it, `pinned_at` stays set, and the
/// CHECK fires. It does not reject the pin. It rejects the DELETE — removing a
/// departed employee would fail with a constraint error naming `documents`,
/// a table nobody was touching.
///
/// So the invariant is the other way round, and this is the case that proves
/// it: the person goes, the pin stays, and the shelf still renders.
#[tokio::test]
async fn deleting_the_officer_who_pinned_something_leaves_the_pin() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;

    let mut am: documents::ActiveModel = documents::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.pinned_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    am.pinned_by = ActiveValue::Set(Some(t.officer));
    am.update(&db).await.expect("pin it");

    // The instant as STORED, read back, not the one just constructed.
    // `timestamptz` keeps microseconds and `Utc::now()` carries nanoseconds, so
    // comparing the in-memory value against a row read out of Postgres asserts
    // the precision of the clock rather than the survival of the pin. It failed
    // in CI on exactly that: `…314063` back from the database against
    // `…314063302` in memory.
    let at = documents::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .pinned_at;
    assert!(at.is_some(), "the pin was not stored");

    users::Entity::delete_by_id(t.officer)
        .exec(&db)
        .await
        .expect("the officer who pinned a document must still be deletable");

    let after = documents::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .expect("the document outlives the officer");
    assert_eq!(
        after.pinned_at, at,
        "the pin was lost when its author was deleted"
    );
    assert_eq!(
        after.pinned_by, None,
        "`ON DELETE SET NULL` did not clear the author"
    );
}

/// The load-bearing one. A favorite is a bookmark, never a grant: filtering to
/// Favorites must still be intersected with what the caller may see, or a stale
/// bookmark becomes a way back into a document that moved out of reach.
#[tokio::test]
async fn the_favorites_filter_cannot_widen_what_a_worker_sees() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let open_doc = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;
    let hq_doc = doc(&db, t.org, "Board pack", "hq", None, true, t.officer).await;

    // Both bookmarked by the worker — the second one seeded directly, standing
    // in for a document that was org-visible when they favorited it and was
    // moved to `hq` afterwards.
    bookmark(&db, t.worker_a, open_doc).await;
    bookmark(&db, t.worker_a, hq_doc).await;

    assert_eq!(
        document_favorites::Entity::find()
            .filter(document_favorites::Column::UserId.eq(t.worker_a))
            .all(&db)
            .await
            .unwrap()
            .len(),
        2,
        "both bookmarks exist"
    );

    // The predicate the handler composes, not a second copy of it.
    let standing = resolve_standing(&db, t.worker_a, t.org).await.unwrap();
    let filter = visible_documents_scoped(t.org, t.worker_a, &standing, Trash::Excluded)
        .expect("standing")
        .add(favorited_by(t.worker_a));
    let visible: Vec<Uuid> = documents::Entity::find()
        .filter(filter)
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect();

    assert_eq!(
        visible,
        vec![open_doc],
        "a stale bookmark handed a worker an hq document"
    );
}

/// Favoriting the same document twice is a no-op, not an error.
///
/// The doc comment on the handler has promised this from the start. The first
/// implementation was `find_by_id` then a conditional `insert`, which promises
/// it and does not deliver — two taps that both read "absent" both insert, and
/// the loser gets a unique violation the handler renders as `500`. On a shared
/// tablet a double tap is the normal case.
///
/// The second implementation used `Insert::do_nothing`, which is deprecated in
/// sea-orm 2.0 for the reason this test exists: with `ON CONFLICT DO NOTHING`
/// and a plain `exec`, an insert that inserted nothing answers
/// `DbErr::RecordNotInserted` — an error, from the clause added to stop one.
/// `on_conflict_do_nothing_on` returns a `TryInsert`, which does not.
///
/// Asserted at the database rather than through HTTP, because what is under
/// test is which `DbErr` comes back, and a route test would only see the `500`.
#[tokio::test]
async fn favoriting_twice_is_not_an_error() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(&db, t.org, "Handbook", "org", None, true, t.officer).await;

    // `add_favorite` is the statement the handler runs. Written inline here the
    // first time, which made the test unable to notice a regression in the
    // thing it guards — the anti-pattern `shelf.rs` documents beside it.
    let tap = || {
        let db = &db;
        async move { add_favorite(db, t.officer, id).await }
    };

    tap().await.expect("the first tap");
    tap()
        .await
        .expect("the second tap must be a no-op, not the 500 this replaces");

    assert_eq!(
        document_favorites::Entity::find()
            .filter(document_favorites::Column::UserId.eq(t.officer))
            .all(&db)
            .await
            .unwrap()
            .len(),
        1,
        "the second tap must not have written a second row either"
    );
}
