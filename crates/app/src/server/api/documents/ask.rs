//! Asking the library a question.
//!
//! # Why this is not a flag on `/documents/search`
//!
//! It started as one. `GET /documents/search?ask=true` is a smaller diff and a
//! smaller client, and it is wrong: producing an answer means resolving an
//! agent's config out of the workspace working copy, which pins the handler to
//! the ide singleton. Search reads Postgres and must keep running on every
//! replica, because a library nobody can search while the ide restarts is a
//! library nobody trusts. One route cannot be both, so there are two.
//!
//! The visible consequence is the honest one: search never gets slower or more
//! expensive because answering exists, and answering can be down while search
//! is up.
//!
//! # What a session is
//!
//! A list of past turns the client sends back, not a row this server keeps.
//! Nothing here is stored, so there is nothing to leak later and nothing to
//! garbage-collect — and, more usefully, no way for a session to become a
//! second copy of the library with its own permission history.
//!
//! **Every turn re-searches.** The candidates are resolved fresh from
//! [`super::visibility::visible_documents`] for the caller of THIS request, so
//! a document that stops being visible between turn one and turn four stops
//! being an input at turn four. A session that carried its retrieved text
//! forward would answer from material the asker is no longer allowed to read,
//! and would do it silently. That is why [`super::search::Turn`] carries ids
//! and prose but never bodies.

use axum::Json;
use axum::http::StatusCode;
use entity::documents;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseConnection, DbErr};
use serde::Deserialize;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use super::ask_agent::DocumentAgent;
use super::ask_sessions;
use super::dto::DocumentSummary;
use super::handlers::db_err;
use super::hydrate;
use super::search::{Answer, DocumentSearch, MatchMode, PostgresSearch, Turn};
use super::visibility::{ReadStanding, resolve_standing};

/// How many ranked matches an answer may read.
///
/// Stated here rather than discovered in a bill. A chapter may be a megabyte;
/// ten of them is a context window, a wait and an invoice for a question whose
/// answer is almost always in the first two or three matches. The engine
/// ranked them, and this trusts that ranking rather than paying to
/// second-guess it.
pub const ASK_CANDIDATES: u64 = 4;

/// How much of a session is carried into the next question.
///
/// A session that grows without limit turns a cheap fourth question into an
/// expensive fortieth one, and the turns that matter to a follow-up are the
/// recent ones. The oldest are dropped, not summarised: a summary of a
/// transcript is another model call to pay for and another thing to be wrong.
const HISTORY_TURNS: usize = 8;

#[derive(Debug, Deserialize)]
pub struct AskRequest {
    pub org_id: Uuid,
    /// The question, in the asker's own words.
    ///
    /// Named `q` to match `GET /documents/search?q=`, because it is the same
    /// words typed into the same box. A body field could afford the longer
    /// name; two names for one input could not.
    pub q: String,
    /// A stored session to continue, from `POST /documents/ask/sessions`.
    ///
    /// When present the history comes from the store and [`Self::history`] is
    /// ignored — one source of truth, so a client that falls behind cannot
    /// replay a turn the server has since dropped. The turn is appended
    /// afterwards.
    ///
    /// Absent is the one-shot ask, and it stores nothing: a list of sessions
    /// should be a list of conversations, not a log of every question anybody
    /// typed once.
    #[serde(default)]
    pub session_id: Option<Uuid>,
    /// The session so far, oldest first. Absent for a one-shot ask, and
    /// ignored entirely when [`Self::session_id`] is set.
    ///
    /// The server trusts this for CONTEXT only: it never becomes a source, and
    /// it can never put a document back in front of somebody, because the
    /// candidates are retrieved fresh from the caller's standing on every turn.
    #[serde(default)]
    pub history: Vec<TurnBody>,
}

/// One earlier exchange, as the client holds it.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct TurnBody {
    pub question: String,
    pub answer: String,
    #[serde(default)]
    pub cited: Vec<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct AnswerBody {
    pub text: String,
    /// Document ids in the order the answer first cited them, so `[1]` in the
    /// text and `sources[0]` in the response are the same document.
    pub cited: Vec<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct AskResponse {
    /// Which engine answered. `postgres-fts` here means no answer was possible
    /// — see [`answering_engine`] — which is a different ticket from a bad one.
    pub engine: &'static str,
    /// `None` when no engine could write one. The client renders the sources
    /// alone in that case, which is still worth more than an error.
    pub answer: Option<AnswerBody>,
    /// Exactly the documents the engine was given, in the order it got them.
    ///
    /// Returned so a citation is something the reader can open. An answer whose
    /// footnotes go nowhere asks to be taken on faith, which is the one thing a
    /// generated paragraph over somebody's SOPs must never ask for.
    pub sources: Vec<DocumentSummary>,
}

/// `POST /api/documents/ask`
///
/// Mounted `route_ide`: [`DocumentAgent`] resolves an agent config through the
/// workspace working copy. See the module docs for why that is not a reason to
/// move search with it.
#[instrument(skip_all, fields(org = %req.org_id, session = ?req.session_id))]
pub async fn ask(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(req): Json<AskRequest>,
) -> Result<Json<AskResponse>, StatusCode> {
    let question = req.q.trim();
    if question.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let standing = resolve_standing(&db, user.id, req.org_id)
        .await
        .map_err(db_err)?;

    // `404` before anything runs, exactly as every other read on this surface
    // answers it. A caller with no standing must be a branch, not a filter
    // trusted to come back empty — and here the filter guards a model call.
    if matches!(standing, ReadStanding::None) {
        return Err(StatusCode::NOT_FOUND);
    }

    let history: Vec<Turn> = match req.session_id {
        // A stored session is the authority on its own past. Reading it here
        // rather than trusting what the client sent is also what applies the
        // drop rule in `turns_of`: a turn whose documents the caller can no
        // longer open is not replayed into a new prompt.
        Some(id) => ask_sessions::turns_of(&db, id, req.org_id, user.id, &standing)
            .await
            .map_err(db_err)?,
        None => req
            .history
            .iter()
            .rev()
            .take(HISTORY_TURNS)
            .rev()
            .map(|t| Turn {
                question: t.question.clone(),
                answer: t.answer.clone(),
                cited: t.cited.clone(),
            })
            .collect(),
    };

    let engine = answering_engine(&db, req.org_id, user.id).await;
    let (rows, answer) = retrieve_and_answer(
        &db,
        engine.as_ref(),
        req.org_id,
        user.id,
        &standing,
        question,
        &history,
    )
    .await
    .map_err(db_err)?;

    info!(
        engine = engine.name(),
        sources = rows.len(),
        answered = answer.is_some(),
        "document ask"
    );

    // Recorded after the answer, and never in front of it. A failure to write
    // down what somebody was told must not turn a good answer into a `500`, so
    // this logs and carries on — the reader still gets the answer, and the
    // operator gets the reason the transcript is short.
    if let Some(id) = req.session_id {
        let cited = answer.as_ref().map(|a| a.cited.clone()).unwrap_or_default();
        // `sources` in the order the engine got them, because that is what the
        // `[1]`, `[2]` markers in the prose index into. Recorded before
        // hydration, so the numbering a reader sees later is the numbering the
        // answer was written against.
        let source_ids: Vec<Uuid> = rows.iter().map(|d| d.id).collect();
        if let Err(e) = ask_sessions::record(
            &db,
            id,
            user.id,
            question,
            answer.as_ref().map(|a| a.text.as_str()),
            &source_ids,
            &cited,
        )
        .await
        {
            warn!(session = %id, error = %e, "could not record the turn");
        }
    }

    let sources = hydrate::summaries(&db, user.id, rows)
        .await
        .map_err(db_err)?;
    Ok(Json(AskResponse {
        engine: engine.name(),
        answer: answer.map(|a| AnswerBody {
            text: a.text,
            cited: a.cited,
        }),
        sources,
    }))
}

/// Retrieve, then answer from what was retrieved.
///
/// A free function rather than four lines inside the handler because it is the
/// property the gate tests assert: that what `query` returns is exactly what
/// `answer` receives. A test that re-implemented this composition would pass
/// while the shipped one leaked, so `tests/platform/document_ask.rs` calls
/// this.
pub async fn retrieve_and_answer(
    db: &DatabaseConnection,
    engine: &dyn DocumentSearch,
    org_id: Uuid,
    caller: Uuid,
    standing: &ReadStanding,
    question: &str,
    history: &[Turn],
) -> Result<(Vec<documents::Model>, Option<Answer>), DbErr> {
    // `Any`, not `All`. A question is eight words, and requiring all of them
    // of one document matches nothing — this route shipped with `All` and
    // answered a question with zero sources whose answer was the top hit for
    // the same words typed into the search box.
    let mut rows = engine
        .query(
            db,
            org_id,
            caller,
            standing,
            question,
            MatchMode::Any,
            ASK_CANDIDATES,
        )
        .await?;

    // A follow-up is usually elliptical, and a search cannot read a pronoun.
    //
    // Observed in a three-turn session on the rig: "how often should that be
    // checked?" retrieved nothing useful, and the agent answered that the
    // documents say nothing about frequency — one turn after citing the
    // document whose first line is "test the bucket at open, at every shift
    // change and at close". The words that would have found it were in the
    // PREVIOUS question.
    //
    // So a thin result on a session turn is retried with the previous
    // question's words appended. Only a thin one: a follow-up that already
    // fills the candidate list stands on its own, and widening it would let an
    // old subject outrank the current one.
    //
    // This borrows WORDS, never rows. The retry is the same `query`, with the
    // same standing, through the same visibility filter — so a document that
    // became `hq` between turns is still absent, which is the property the
    // per-turn re-search exists for. Re-fetching the previous turn's cited ids
    // directly would have been the obvious alternative and is exactly the
    // second retrieval path that could drift from the gate.
    if rows.len() < ASK_CANDIDATES as usize
        && let Some(previous) = history.last()
    {
        let widened = format!("{} {}", previous.question, question);
        let more = engine
            .query(
                db,
                org_id,
                caller,
                standing,
                &widened,
                MatchMode::Any,
                ASK_CANDIDATES,
            )
            .await?;
        // Appended, not prepended: what the current question found on its own
        // is the better match for the current question, and stays first.
        for doc in more {
            if rows.len() >= ASK_CANDIDATES as usize {
                break;
            }
            if !rows.iter().any(|r| r.id == doc.id) {
                rows.push(doc);
            }
        }
    }

    if rows.is_empty() {
        // Nothing survived the filter, so there is nothing to answer FROM. An
        // engine called here would hold only the question, and a model with a
        // question and no material is a model that invents one.
        return Ok((rows, None));
    }

    let bodies = hydrate::bodies_of(db, &rows).await?;
    let answer = match engine.answer(question, &rows, &bodies, history).await {
        Ok(a) => a,
        Err(e) => {
            // The answer is the optional half of this route. Returning `500`
            // because a model failed would throw away the sources too, and the
            // sources are what the reader can act on. Logged so the failure is
            // silent only to the reader, never to an operator.
            warn!(engine = engine.name(), error = %e, "the engine failed to answer");
            None
        }
    };
    Ok((rows, answer))
}

/// The engine this deployment answers with, or the one that admits it cannot.
///
/// Resolution can fail for ordinary reasons — an org with no workspace, a
/// workspace with no document agent in it — and each of them means the same
/// thing to a reader: sources, no written answer. So a failure falls back to
/// [`PostgresSearch`], whose `name()` is what an operator reads to tell
/// "answering is not configured here" from "the answer was bad".
async fn answering_engine(
    db: &DatabaseConnection,
    org_id: Uuid,
    caller: Uuid,
) -> Box<dyn DocumentSearch> {
    match DocumentAgent::resolve(db, org_id, caller).await {
        Some(agent) => Box::new(agent),
        None => Box::new(PostgresSearch),
    }
}
