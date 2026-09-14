//! Search, and the one property that matters more than relevance.
//!
//! A search endpoint that answers from its own index is the classic way a
//! permission model is bypassed without anybody editing the permission model.
//! So the assertions here are in two halves: that search finds things at all
//! (a seam whose default returns nothing is worse than no seam, because it
//! looks like an empty library), and that everything it finds has been through
//! the same `visible_documents` filter the listing uses.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(document_search)'`

use chrono::Utc;
use entity::{document_versions, documents};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use uuid::Uuid;

use oxy_app::server::api::documents::search::{DocumentSearch, MatchMode, PostgresSearch};
use oxy_app::server::api::documents::visibility::{ReadStanding, resolve_standing};

use crate::common::{Schema, fresh_db};
use crate::documents::{doc, folder, seed_tenant};

/// Give a document a chapter body and make it current, so the body index has
/// something to match and the "current version only" rule has something to
/// exclude.
async fn set_body(db: &sea_orm::DatabaseConnection, id: Uuid, author: Uuid, text: &str) {
    let now = Utc::now().fixed_offset();
    let existing = documents::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let next = existing.current_version_id.map_or(2, |_| 2);
    let version = Uuid::new_v4();
    document_versions::ActiveModel {
        id: ActiveValue::Set(version),
        document_id: ActiveValue::Set(id),
        version_no: ActiveValue::Set(next),
        author_id: ActiveValue::Set(Some(author)),
        body: ActiveValue::Set(Some(text.to_string())),
        object_key: ActiveValue::Set(None),
        content_type: ActiveValue::Set(Some("text/markdown".into())),
        size_bytes: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed version");

    let mut am: documents::ActiveModel = existing.into();
    am.current_version_id = ActiveValue::Set(Some(version));
    am.update(db).await.expect("point at the new version");
}

async fn hits(db: &sea_orm::DatabaseConnection, org: Uuid, caller: Uuid, text: &str) -> Vec<Uuid> {
    hits_in(db, org, caller, text, MatchMode::All).await
}

async fn hits_in(
    db: &sea_orm::DatabaseConnection,
    org: Uuid,
    caller: Uuid,
    text: &str,
    mode: MatchMode,
) -> Vec<Uuid> {
    let standing = resolve_standing(db, caller, org).await.expect("standing");
    PostgresSearch
        .query(db, org, caller, &standing, text, mode, 50)
        .await
        .expect("search")
        .into_iter()
        .map(|d| d.id)
        .collect()
}

/// The two modes, on the same document, with the same words.
///
/// This shipped wrong. `POST /documents/ask` used `All` and returned zero
/// sources for a question whose answer was the top hit of the same words typed
/// into the search box — `plainto_tsquery` ANDs its lexemes, so a ten-word
/// question needs a document containing all ten.
///
/// Both directions are asserted, because either alone would pass for the wrong
/// reason: `Any` finding it proves nothing if `All` also does, and `All`
/// missing it proves nothing if the document was never findable.
#[tokio::test]
async fn a_question_finds_what_an_all_words_match_misses() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let sop = doc(
        &db,
        t.org,
        "Sanitizer Log Procedure",
        "org",
        None,
        true,
        t.officer,
    )
    .await;
    set_body(
        &db,
        sop,
        t.officer,
        "Remake the bucket when the strips read under 200 ppm.",
    )
    .await;

    // The words somebody types into the box.
    assert!(
        hits(&db, t.org, t.officer, "sanitizer")
            .await
            .contains(&sop),
        "the document has to be findable at all, or the rest of this test is \
         asserting nothing"
    );

    // The same thing, asked as a question. `dilute` appears nowhere in it.
    let question = "how do I dilute the sanitizer";
    assert!(
        !hits(&db, t.org, t.officer, question).await.contains(&sop),
        "`All` matched a question containing a word the document does not have \
         — if this ever passes, the mode has stopped meaning anything and the \
         assertion below is no longer evidence"
    );
    assert!(
        hits_in(&db, t.org, t.officer, question, MatchMode::Any)
            .await
            .contains(&sop),
        "a question found nothing, which is the bug this mode exists for: the \
         reader gets an answer written from no documents, or no answer at all"
    );
}

/// `Any` widens what matches; it must not widen who may see it.
///
/// The visibility filter is composed onto the text match rather than applied
/// after it, so this is the assertion that the composition survived a change to
/// the half next to it.
#[tokio::test]
async fn asking_a_question_does_not_reach_past_the_visibility_gate() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let board = doc(
        &db,
        t.org,
        "Sanitizer board paper",
        "hq",
        None,
        true,
        t.officer,
    )
    .await;

    assert!(
        hits_in(
            &db,
            t.org,
            t.officer,
            "what does the sanitizer paper say",
            MatchMode::Any
        )
        .await
        .contains(&board),
        "an officer may read it, so it must match — otherwise the next \
         assertion passes because nothing matched at all"
    );
    assert!(
        !hits_in(
            &db,
            t.org,
            t.worker_a,
            "what does the sanitizer paper say",
            MatchMode::Any
        )
        .await
        .contains(&board),
        "the wider match mode reached an hq document for a frontline caller"
    );
}

/// The engine answers, and says which engine it was. A seam whose default
/// returns nothing is worse than no seam — it looks like an empty library.
#[tokio::test]
async fn search_finds_a_document_by_its_title() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let sop = doc(
        &db,
        t.org,
        "Sanitizer Log Procedure",
        "org",
        None,
        true,
        t.officer,
    )
    .await;
    doc(
        &db,
        t.org,
        "Onigiri Wrapping Standard",
        "org",
        None,
        true,
        t.officer,
    )
    .await;

    assert_eq!(PostgresSearch.name(), "postgres-fts");
    let found = hits(&db, t.org, t.officer, "sanitizer").await;
    assert_eq!(found, vec![sop]);
}

/// The half a title index alone would miss, and the half the product's own
/// search box promises: "search chapters, forms, SOPs".
#[tokio::test]
async fn search_finds_a_document_by_its_chapter_text() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let onigiri = doc(
        &db,
        t.org,
        "Onigiri Wrapping Standard",
        "org",
        None,
        true,
        t.officer,
    )
    .await;
    set_body(
        &db,
        onigiri,
        t.officer,
        "Remake the bucket when the strips read under 200 ppm.",
    )
    .await;

    assert_eq!(
        hits(&db, t.org, t.officer, "bucket strips").await,
        vec![onigiri]
    );
    // Stemming, which is the difference between a search box and a `LIKE`.
    assert_eq!(hits(&db, t.org, t.officer, "remaking").await, vec![onigiri]);
    assert!(hits(&db, t.org, t.officer, "forklift").await.is_empty());
}

/// The load-bearing case. A worker searching must not find what a worker
/// listing cannot see — the same filter, or the endpoint is a way around it.
#[tokio::test]
async fn search_cannot_find_what_the_listing_hides() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    let hq = doc(
        &db,
        t.org,
        "Board pack allergen review",
        "hq",
        None,
        true,
        t.officer,
    )
    .await;
    let other_store = doc(
        &db,
        t.org,
        "Allergen chart store B",
        "org",
        Some(t.store_b),
        true,
        t.officer,
    )
    .await;
    let theirs = doc(
        &db,
        t.org,
        "Allergen chart store A",
        "org",
        Some(t.store_a),
        true,
        t.officer,
    )
    .await;

    // An officer finds all three: the term really does match every one of them,
    // so the worker's misses below are the filter and not the query.
    let officer_hits = hits(&db, t.org, t.officer, "allergen").await;
    for id in [hq, other_store, theirs] {
        assert!(officer_hits.contains(&id));
    }

    let worker_hits = hits(&db, t.org, t.worker_a, "allergen").await;
    assert_eq!(
        worker_hits,
        vec![theirs],
        "search returned a document the listing hides"
    );
}

/// A stranger to the org gets nothing, and gets it without a query being built
/// at all — `visible_documents` returns `None` and the engine returns early.
#[tokio::test]
async fn search_returns_nothing_to_a_stranger() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let a = seed_tenant(&db, "acme").await;
    let b = seed_tenant(&db, "beta").await;
    doc(
        &db,
        a.org,
        "Sanitizer Log Procedure",
        "org",
        None,
        true,
        a.officer,
    )
    .await;

    assert!(hits(&db, a.org, b.officer, "sanitizer").await.is_empty());
}

/// Only the CURRENT version is searchable. Older ones stay indexed so an audit
/// can ask which version said something, but a hit on text somebody deleted two
/// revisions ago answers a question nobody asked.
#[tokio::test]
async fn search_matches_the_current_version_and_not_a_superseded_one() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let d = doc(&db, t.org, "Closing routine", "org", None, true, t.officer).await;
    set_body(
        &db,
        d,
        t.officer,
        "Sweep the walk-in and log the temperature.",
    )
    .await;

    assert_eq!(hits(&db, t.org, t.officer, "walk-in").await, vec![d]);

    // Version 1, seeded by `doc`, carried the title as its body. Superseded now.
    assert!(
        hits(&db, t.org, t.officer, "Closing routine")
            .await
            .contains(&d),
        "the title still matches, which is the title index doing its job"
    );
}

/// Folders match by name, and a search that found the document but not its
/// folder would leave the reader somewhere they cannot navigate back from.
#[tokio::test]
async fn search_finds_a_folder_by_name() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let boh = folder(&db, t.org, "Back of House", "org", t.officer).await;
    folder(&db, t.org, "Marketing Essentials", "org", t.officer).await;

    let standing = resolve_standing(&db, t.officer, t.org).await.unwrap();
    let hits = PostgresSearch
        .query_folders(&db, t.org, &standing, "house", 50)
        .await
        .expect("folder search");

    assert_eq!(hits.iter().map(|f| f.id).collect::<Vec<_>>(), vec![boh]);
}

/// The folder tree's `hq` rule holds inside search too. Finding a folder by
/// name that the tree refuses to list would be the same leak by another route.
#[tokio::test]
async fn a_worker_cannot_find_an_hq_folder_by_searching_for_it() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let board = folder(&db, t.org, "Board Papers", "hq", t.officer).await;
    let open = folder(&db, t.org, "Board Cleaning Rota", "org", t.officer).await;

    let officer = resolve_standing(&db, t.officer, t.org).await.unwrap();
    let found: Vec<_> = PostgresSearch
        .query_folders(&db, t.org, &officer, "board", 50)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.id)
        .collect();
    assert!(
        found.contains(&board) && found.contains(&open),
        "both match"
    );

    let worker = resolve_standing(&db, t.worker_a, t.org).await.unwrap();
    let seen: Vec<_> = PostgresSearch
        .query_folders(&db, t.org, &worker, "board", 50)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.id)
        .collect();
    assert_eq!(seen, vec![open], "a worker found an hq folder by searching");
}

/// Ranked, and the ranking survives two custom expressions in one query.
///
/// Two things are under test and only one of them is about relevance.
///
/// The first is that ordering happens at all: both queries applied a `LIMIT` to
/// an unordered scan, so Postgres could return any `limit` of the matches and a
/// different set next time, under a trait doc promising "most relevant first".
///
/// The second is the reason this is an integration test rather than an
/// assertion about SQL text. The filter is an `Expr::cust_with_values` using
/// `$1` and `$2`, and the `ORDER BY` added beside it is another one using `$1`.
/// Whether those placeholders collide is a property of how sea-query binds each
/// fragment, and reading the code is not proof — a collision would either error
/// or, worse, silently rank on the wrong value. Running it is proof.
#[tokio::test]
async fn search_ranks_a_title_match_above_a_passing_mention() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    // The word is in the title of one and buried in the body of the other —
    // and the weaker match is inserted FIRST, deliberately. An unordered scan
    // returns rows in roughly physical order, so seeding the better match first
    // would let the test pass with no `ORDER BY` at all. It did, on the first
    // version of this test, which is the whole argument for making a new check
    // fail once before trusting it.
    let mentioned = doc(
        &db,
        t.org,
        "Opening Checklist",
        "org",
        None,
        true,
        t.officer,
    )
    .await;
    set_body(
        &db,
        mentioned,
        t.officer,
        "Unlock the doors, count the float, check the sanitizer bucket, and brief the team.",
    )
    .await;
    let titled = doc(
        &db,
        t.org,
        "Sanitizer Log Procedure",
        "org",
        None,
        true,
        t.officer,
    )
    .await;

    let found = hits(&db, t.org, t.officer, "sanitizer").await;
    assert_eq!(
        found.len(),
        2,
        "both documents match, or this is not a ranking test"
    );
    assert_eq!(
        found[0], titled,
        "the title match must outrank the passing mention — and the ORDER BY must be reading the query it was given"
    );
    assert_eq!(found[1], mentioned);
}

/// The same query twice returns the same order.
///
/// The property a `LIMIT` without an `ORDER BY` cannot give you, stated as the
/// thing a person actually notices: a list that reshuffles between two
/// identical searches.
#[tokio::test]
async fn the_same_search_twice_returns_the_same_order() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    for n in 0..6 {
        doc(
            &db,
            t.org,
            &format!("Permit renewal {n}"),
            "org",
            None,
            true,
            t.officer,
        )
        .await;
    }
    let first = hits(&db, t.org, t.officer, "permit").await;
    let second = hits(&db, t.org, t.officer, "permit").await;
    assert_eq!(first.len(), 6);
    assert_eq!(first, second);
}

/// A stranger gets the same `404` every other read on this surface gives.
///
/// `search_returns_nothing_to_a_stranger` above asserts the ENGINE returns no
/// rows, which was true and was not the whole story: the handler wrapped that
/// in `200 {"documents":[],"folders":[]}` while `list`, `list_folders` and
/// `categories::list` all answered `404` for the same org — so the one route
/// that broke the pattern was the one that could be used to probe whether a
/// tenant exists. It also reached `hydrate::folder_counts`, an unconditional
/// `GROUP BY` over `documents` keyed on a caller-supplied org id.
///
/// Asserted on the standing rather than on the status code because the handler
/// takes extractors this suite cannot build; `ReadStanding::None` is what the
/// `404` is derived from, and `visible_documents` returning `None` for it is
/// what made the old shape possible.
#[tokio::test]
async fn a_search_in_an_org_you_have_no_standing_in_resolves_to_nothing() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let stranger = seed_tenant(&db, "other").await;

    doc(&db, t.org, "Health permit", "org", None, true, t.officer).await;

    // The officer of another tenant, asking about this one.
    let standing = resolve_standing(&db, stranger.officer, t.org)
        .await
        .expect("standing resolves");
    assert!(
        matches!(standing, ReadStanding::None),
        "an officer of another org must have no standing here"
    );

    // And with no standing there is no query to run — which is the branch the
    // handler now takes before it touches the database at all.
    assert!(
        oxy_app::server::api::documents::visibility::visible_documents(
            t.org,
            stranger.officer,
            &standing
        )
        .is_none()
    );
}
