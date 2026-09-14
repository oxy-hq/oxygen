//! What an answering engine is allowed to see.
//!
//! Not whether an answer is good — that is a nondeterministic property of a
//! model, and a test asserting it fails on a Tuesday for no reason. These
//! assert the engine's INPUTS, which are not nondeterministic at all: what an
//! `hq` board paper is doing in a frontline worker's candidate list has exactly
//! one right answer.
//!
//! The engine here records its arguments and answers nothing. That is the whole
//! fixture — no model, no key, no network — and it is worth more than any
//! number of prompt tests, because the property being defended is that certain
//! rows never reach the call at all.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(document_ask)'`

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use entity::documents;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, DbErr, EntityTrait};
use uuid::Uuid;

use oxy_app::server::api::documents::ask::retrieve_and_answer;
use oxy_app::server::api::documents::search::{
    Answer, DocumentSearch, MatchMode, PostgresSearch, Turn,
};
use oxy_app::server::api::documents::visibility::resolve_standing;
use oxy_shared::errors::OxyError;

use crate::common::{Schema, fresh_db};
use crate::documents::{doc, seed_tenant};

/// An engine that answers nothing and remembers what it was asked with.
///
/// Retrieval is delegated to the real one, because the property under test is
/// the COMPOSITION — that what `query` returns is what `answer` receives — and
/// a fake retriever would let this pass while the shipped one leaked.
struct Recorder {
    inner: PostgresSearch,
    seen: Arc<Mutex<Vec<Uuid>>>,
    calls: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl DocumentSearch for Recorder {
    fn name(&self) -> &'static str {
        "recorder"
    }

    async fn query(
        &self,
        db: &DatabaseConnection,
        org_id: Uuid,
        caller: Uuid,
        standing: &oxy_app::server::api::documents::visibility::ReadStanding,
        text: &str,
        mode: MatchMode,
        limit: u64,
    ) -> Result<Vec<documents::Model>, DbErr> {
        self.inner
            .query(db, org_id, caller, standing, text, mode, limit)
            .await
    }

    async fn query_folders(
        &self,
        db: &DatabaseConnection,
        org_id: Uuid,
        standing: &oxy_app::server::api::documents::visibility::ReadStanding,
        text: &str,
        limit: u64,
    ) -> Result<Vec<entity::folders::Model>, DbErr> {
        self.inner
            .query_folders(db, org_id, standing, text, limit)
            .await
    }

    async fn answer(
        &self,
        _question: &str,
        candidates: &[documents::Model],
        _bodies: &HashMap<Uuid, String>,
        _history: &[Turn],
    ) -> Result<Option<Answer>, OxyError> {
        *self.calls.lock().unwrap() += 1;
        *self.seen.lock().unwrap() = candidates.iter().map(|d| d.id).collect();
        Ok(None)
    }
}

impl Recorder {
    fn new() -> Self {
        Self {
            inner: PostgresSearch,
            seen: Arc::new(Mutex::new(vec![])),
            calls: Arc::new(Mutex::new(0)),
        }
    }

    /// Ask through the SHIPPED composition rather than a copy of it.
    ///
    /// `retrieve_and_answer` is the function the handler calls, so a change
    /// that widened what reaches `answer` — retrieving before the standing
    /// check, reusing an earlier turn's rows, hydrating the whole library
    /// instead of the candidates — fails here. An `ask` that re-implemented
    /// those four lines would keep passing while the route leaked.
    async fn ask(&self, db: &DatabaseConnection, org: Uuid, caller: Uuid, q: &str) -> Vec<Uuid> {
        // Cleared first, because the shipped composition does NOT call the
        // engine when nothing was retrieved. Without this reset, "the engine
        // was never called" reads as "the engine saw what it saw last time" —
        // and two of the assertions below passed for that reason before the
        // handler stopped asking a model to answer from an empty set.
        // `calls()` is what tells the two states apart.
        self.seen.lock().unwrap().clear();
        let standing = resolve_standing(db, caller, org).await.unwrap();
        retrieve_and_answer(db, self, org, caller, &standing, q, &[])
            .await
            .unwrap();
        self.seen.lock().unwrap().clone()
    }

    /// As [`Self::ask`], carrying a session.
    async fn ask_in_session(
        &self,
        db: &DatabaseConnection,
        org: Uuid,
        caller: Uuid,
        q: &str,
        history: &[Turn],
    ) -> Vec<Uuid> {
        self.seen.lock().unwrap().clear();
        let standing = resolve_standing(db, caller, org).await.unwrap();
        retrieve_and_answer(db, self, org, caller, &standing, q, history)
            .await
            .unwrap();
        self.seen.lock().unwrap().clone()
    }

    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

/// The load-bearing one: an engine never sees what its caller may not read.
#[tokio::test]
async fn an_hq_document_never_reaches_the_engine_for_a_frontline_caller() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;

    // Both mention the word, so ranking cannot be what separates them.
    let board = doc(
        &db,
        t.org,
        "Sanitiser board paper",
        "hq",
        None,
        true,
        t.officer,
    )
    .await;
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

    let engine = Recorder::new();

    let officer_saw = engine.ask(&db, t.org, t.officer, "sanitiser").await;
    assert!(
        officer_saw.contains(&board) && officer_saw.contains(&sop),
        "an officer may read both, so both are candidates — otherwise the next \
         assertion would pass for the wrong reason"
    );

    let worker_saw = engine.ask(&db, t.org, t.worker_a, "sanitiser").await;
    assert!(
        worker_saw.contains(&sop),
        "the worker may read the SOP and it matched, so it must be a candidate"
    );
    assert!(
        !worker_saw.contains(&board),
        "an hq document was handed to the engine for a caller who cannot read it — \
         the model could quote it, and no prompt would stop that"
    );
}

/// A draft somebody else is writing is not material for an answer either.
#[tokio::test]
async fn another_authors_draft_is_not_a_candidate() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let draft = doc(
        &db,
        t.org,
        "Sanitiser rewrite",
        "org",
        None,
        false,
        t.officer,
    )
    .await;

    let engine = Recorder::new();
    let worker_saw = engine.ask(&db, t.org, t.worker_a, "sanitiser").await;

    assert!(
        !worker_saw.contains(&draft),
        "an unpublished draft reached the engine — a half-written procedure \
         answered as if it were the standing one"
    );
}

/// Standing is re-resolved per ask, so a permission change lands on the next
/// question rather than the next session.
///
/// This is the assertion the two-mode design exists for. An implementation that
/// retrieved once and reused the result across a conversation passes every test
/// above and fails this one.
#[tokio::test]
async fn marking_a_document_hq_removes_it_from_the_next_ask() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let id = doc(
        &db,
        t.org,
        "Sanitiser log procedure",
        "org",
        None,
        true,
        t.officer,
    )
    .await;

    let engine = Recorder::new();
    let first = engine.ask(&db, t.org, t.worker_a, "sanitiser").await;
    assert!(first.contains(&id), "it is org-visible on the first ask");

    let mut am: documents::ActiveModel = documents::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.visibility = ActiveValue::Set("hq".into());
    am.update(&db).await.expect("mark it head-office only");

    let calls_before = engine.calls();
    let second = engine.ask(&db, t.org, t.worker_a, "sanitiser").await;
    assert!(
        !second.contains(&id),
        "the second ask still handed over a document that had become hq — a \
         session that retrieves once would do exactly this for the rest of the \
         conversation"
    );
    assert_eq!(
        engine.calls(),
        calls_before,
        "nothing was left to answer from, and a model was asked anyway"
    );
}

/// A follow-up that cannot be retrieved on its own borrows the words that can.
///
/// The bug this pins was observed in a three-turn session on the rig: the agent
/// answered that the documents say nothing about check frequency, one turn
/// after citing the document whose first line answers it. "how often should
/// that be checked" shares no word with that document; "remake the sanitizer
/// bucket" — the previous question — shares three.
#[tokio::test]
async fn a_follow_up_reaches_the_document_the_previous_turn_cited() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let sop = doc(
        &db,
        t.org,
        "Sanitizer bucket procedure",
        "org",
        None,
        true,
        t.officer,
    )
    .await;

    let engine = Recorder::new();
    let follow_up = "how often should that be checked";

    // Falsify first: on its own the follow-up finds nothing, so the assertion
    // below is about the history and not about the document being findable.
    assert!(
        engine
            .ask(&db, t.org, t.officer, follow_up)
            .await
            .is_empty(),
        "the follow-up matched on its own, so this test proves nothing"
    );

    let history = [Turn {
        question: "when do I remake the sanitizer bucket".into(),
        answer: "Under 200 ppm.".into(),
        cited: vec![sop],
    }];
    let saw = engine
        .ask_in_session(&db, t.org, t.officer, follow_up, &history)
        .await;
    assert!(
        saw.contains(&sop),
        "a follow-up did not reach the document its own session was about — \
         the reader gets told the library is silent about something it just quoted"
    );
}

/// Borrowing the previous question's words must not borrow its permissions.
#[tokio::test]
async fn a_widened_follow_up_still_stops_at_the_visibility_gate() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    let board = doc(
        &db,
        t.org,
        "Sanitizer bucket board paper",
        "hq",
        None,
        true,
        t.officer,
    )
    .await;

    let engine = Recorder::new();
    let follow_up = "how often should that be checked";
    let history = [Turn {
        question: "when do I remake the sanitizer bucket".into(),
        answer: "Under 200 ppm.".into(),
        cited: vec![board],
    }];

    assert!(
        engine
            .ask_in_session(&db, t.org, t.officer, follow_up, &history)
            .await
            .contains(&board),
        "an officer may read it, so the widened retry must find it — otherwise \
         the next assertion passes because the retry found nothing at all"
    );
    assert!(
        !engine
            .ask_in_session(&db, t.org, t.worker_a, follow_up, &history)
            .await
            .contains(&board),
        "the widened retry handed an hq document to a frontline caller — and it \
         arrived through a history the caller supplied, which is the worst \
         version of this bug"
    );
}

/// No standing, no call. The handler answers 404; the model is not consulted on
/// the way there.
#[tokio::test]
async fn a_stranger_never_reaches_the_engine_at_all() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let t = seed_tenant(&db, "acme").await;
    doc(
        &db,
        t.org,
        "Sanitiser log procedure",
        "org",
        None,
        true,
        t.officer,
    )
    .await;

    let engine = Recorder::new();

    // Falsify first: the same call for somebody with standing does reach it.
    engine.ask(&db, t.org, t.worker_a, "sanitiser").await;
    assert_eq!(engine.calls(), 1, "a member's ask reaches the engine");

    let stranger = Uuid::new_v4();
    let saw = engine.ask(&db, t.org, stranger, "sanitiser").await;

    assert!(saw.is_empty(), "a stranger's ask produced candidates");
    assert_eq!(
        engine.calls(),
        1,
        "a stranger's ask reached the engine — with no documents, which is a \
         model call, a bill and a log line for a caller who is not in this org"
    );
}
