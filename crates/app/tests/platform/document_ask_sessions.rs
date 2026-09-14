//! What a stored conversation may hand back, and to whom.
//!
//! A session is the one place in this surface where content outlives the
//! permission check that produced it. The prose was written from documents the
//! reader could open at the time and it is kept as written — a transcript is a
//! record of what somebody was told, and unsaying it later would be a different
//! product. What must NOT survive is the reach: a citation they can no longer
//! open, and a turn replayed into a new prompt after its documents left them.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(document_ask_sessions)'`

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use oxy_app::server::api::documents::ask_sessions::{self, OrgQuery};
use oxy_app::server::api::documents::visibility::resolve_standing;

use crate::common::test_db;
use crate::documents::{doc, seed_tenant};

fn as_user(id: Uuid) -> AuthenticatedUserExtractor {
    AuthenticatedUserExtractor(AuthenticatedUser {
        id,
        email: None,
        name: "tester".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
    })
}

async fn open_session(org: Uuid, user: Uuid) -> Uuid {
    let (_, Json(session)) = ask_sessions::create(as_user(user), Query(OrgQuery { org_id: org }))
        .await
        .expect("open a session");
    session.id
}

/// Make a document invisible to a frontline caller without deleting it.
async fn mark_hq(db: &DatabaseConnection, id: Uuid) {
    let row = entity::documents::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::documents::ActiveModel = row.into();
    am.visibility = ActiveValue::Set("hq".into());
    am.update(db).await.expect("mark it head-office only");
}

/// The ownership rule, which has no override anywhere in this file.
#[tokio::test]
async fn another_persons_session_does_not_exist() {
    let db = test_db().await;
    let t = seed_tenant(&db, "acme").await;
    let session = open_session(t.org, t.officer).await;

    // Falsify first: the owner can read it, so the assertion below is about
    // who is asking and not about the session being missing.
    assert!(
        ask_sessions::read(as_user(t.officer), Path(session))
            .await
            .is_ok(),
        "the owner cannot read their own session"
    );

    // An OFFICER of the same org — the strongest standing this model has.
    let err = ask_sessions::read(as_user(t.worker_a), Path(session))
        .await
        .expect_err("somebody else read a session");
    assert_eq!(
        err,
        StatusCode::NOT_FOUND,
        "`404`, not `403`: whether a stranger's conversation exists is not a \
         fact this endpoint should confirm"
    );
}

/// A citation the reader may no longer open is absent, not rendered.
#[tokio::test]
async fn a_citation_that_became_unreadable_leaves_the_transcript() {
    let db = test_db().await;
    let t = seed_tenant(&db, "acme").await;
    let sop = doc(
        &db,
        t.org,
        "Sanitiser log procedure",
        "org",
        None,
        true,
        t.officer,
    )
    .await;

    let session = open_session(t.org, t.worker_a).await;
    ask_sessions::record(
        &db,
        session,
        t.worker_a,
        "when do I remake the bucket",
        Some("Under 200 ppm. [1]"),
        &[sop],
        &[sop],
    )
    .await
    .expect("record the turn");

    let Json(before) = ask_sessions::read(as_user(t.worker_a), Path(session))
        .await
        .expect("read it back");
    assert_eq!(
        before.turns[0].cited,
        vec![sop],
        "the citation is missing before the permission even changes"
    );
    assert_eq!(before.documents.len(), 1, "and its document is hydrated");

    mark_hq(&db, sop).await;

    let Json(after) = ask_sessions::read(as_user(t.worker_a), Path(session))
        .await
        .expect("read it back");
    assert!(
        after.turns[0].cited.is_empty(),
        "a citation the reader can no longer open was still offered — clicking \
         it 404s, and its presence discloses that the document exists"
    );
    assert!(
        after.documents.is_empty(),
        "and its title came back in `documents`, which is the same disclosure \
         by a different field"
    );
    assert_eq!(
        after.turns[0].answer.as_deref(),
        Some("Under 200 ppm. [1]"),
        "the prose is kept as written — it is a record of what this person was \
         already told, not a live read"
    );
}

/// The half that matters more: a turn does not carry its content into a NEW
/// prompt after its documents left the caller's reach.
#[tokio::test]
async fn a_turn_whose_documents_left_is_not_replayed_into_the_next_prompt() {
    let db = test_db().await;
    let t = seed_tenant(&db, "acme").await;
    let sop = doc(
        &db,
        t.org,
        "Sanitiser log procedure",
        "org",
        None,
        true,
        t.officer,
    )
    .await;

    let session = open_session(t.org, t.worker_a).await;
    ask_sessions::record(
        &db,
        session,
        t.worker_a,
        "when do I remake the bucket",
        Some("Under 200 ppm. [1]"),
        &[sop],
        &[sop],
    )
    .await
    .expect("record the turn");

    let standing = resolve_standing(&db, t.worker_a, t.org).await.unwrap();
    let before = ask_sessions::turns_of(&db, session, t.org, t.worker_a, &standing)
        .await
        .expect("replay");
    assert_eq!(
        before.len(),
        1,
        "the turn is not replayed even while it is still readable"
    );

    mark_hq(&db, sop).await;

    let standing = resolve_standing(&db, t.worker_a, t.org).await.unwrap();
    let after = ask_sessions::turns_of(&db, session, t.org, t.worker_a, &standing)
        .await
        .expect("replay");
    assert!(
        after.is_empty(),
        "a turn quoting a document the caller has since lost was replayed into \
         the next prompt — the per-turn re-search correctly drops the document \
         and the transcript hands its contents over anyway, which is the exact \
         back door the re-search exists to close"
    );
}

/// A marker in stored prose still points at the document it pointed at.
///
/// The numbering is by candidate position, not citation order, so a transcript
/// holding only the citations renders `[2]` with nothing in slot 2. And a slot
/// the reader has since lost comes back as a hole rather than being removed —
/// removing it would renumber every marker after it and repoint the prose at
/// the wrong document, which is worse than an unresolvable marker.
#[tokio::test]
async fn a_marker_keeps_its_slot_even_when_its_document_leaves() {
    let db = test_db().await;
    let t = seed_tenant(&db, "acme").await;
    let board = doc(&db, t.org, "Board paper", "org", None, true, t.officer).await;
    let sop = doc(&db, t.org, "Sanitiser log", "org", None, true, t.officer).await;

    let session = open_session(t.org, t.worker_a).await;
    ask_sessions::record(
        &db,
        session,
        t.worker_a,
        "when do I remake the bucket",
        // The engine was given both and cited the SECOND one.
        Some("Under 200 ppm. [2]"),
        &[board, sop],
        &[sop],
    )
    .await
    .expect("record the turn");

    let Json(before) = ask_sessions::read(as_user(t.worker_a), Path(session))
        .await
        .expect("read");
    assert_eq!(
        before.turns[0].sources,
        vec![Some(board), Some(sop)],
        "the marker ordering was not kept, so `[2]` in the prose resolves to \
         nothing a reader can open"
    );

    mark_hq(&db, board).await;

    let Json(after) = ask_sessions::read(as_user(t.worker_a), Path(session))
        .await
        .expect("read");
    assert_eq!(
        after.turns[0].sources,
        vec![None, Some(sop)],
        "slot 1 was removed rather than emptied — every later marker shifts \
         down one, and `[2]` in the prose now names a different document than \
         the sentence it supports"
    );
    assert_eq!(
        after.turns[0].cited,
        vec![sop],
        "and the document the answer actually used is still citable"
    );
}

/// A turn that cited nothing has nothing a permission change could take away.
#[tokio::test]
async fn a_turn_that_cited_nothing_is_still_replayed() {
    let db = test_db().await;
    let t = seed_tenant(&db, "acme").await;
    let session = open_session(t.org, t.worker_a).await;
    ask_sessions::record(
        &db,
        session,
        t.worker_a,
        "do we have a dress code",
        Some("The documents do not say."),
        &[],
        &[],
    )
    .await
    .expect("record the turn");

    let standing = resolve_standing(&db, t.worker_a, t.org).await.unwrap();
    let replayed = ask_sessions::turns_of(&db, session, t.org, t.worker_a, &standing)
        .await
        .expect("replay");
    assert_eq!(
        replayed.len(),
        1,
        "a turn with no citations was dropped — the drop rule is about \
         documents that left, and this one never had any"
    );
}

/// Turns come back oldest first, and a session belongs to one person to write
/// as well as to read.
#[tokio::test]
async fn turns_are_ordered_and_only_the_owner_may_append() {
    let db = test_db().await;
    let t = seed_tenant(&db, "acme").await;
    let session = open_session(t.org, t.officer).await;

    for (i, q) in ["first", "second", "third"].iter().enumerate() {
        ask_sessions::record(&db, session, t.officer, q, Some(&format!("a{i}")), &[], &[])
            .await
            .expect("record");
    }
    // Somebody else's write is a no-op, not an error and not an append.
    ask_sessions::record(&db, session, t.worker_a, "intruder", Some("x"), &[], &[])
        .await
        .expect("silently ignored");

    let Json(detail) = ask_sessions::read(as_user(t.officer), Path(session))
        .await
        .expect("read");
    let asked: Vec<&str> = detail.turns.iter().map(|t| t.question.as_str()).collect();
    assert_eq!(
        asked,
        vec!["first", "second", "third"],
        "a conversation came back out of order, or somebody else wrote into it"
    );
}
