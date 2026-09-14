//! Searching the library.
//!
//! # A seam with an engine already in it
//!
//! The notifications work shipped a `Push` trait whose default implementation
//! logged and did nothing, and that was right: nobody mistakes a log line for a
//! phone ringing. Search is the opposite shape. A search box that returns
//! nothing is indistinguishable from a library with nothing in it, and the
//! person who draws the second conclusion stops opening it. So this seam ships
//! with a working engine behind it — Postgres full text over titles and chapter
//! markdown — and a real one (PDF contents, Ask AI) replaces it later without
//! the callers changing.
//!
//! [`DocumentSearch::name`] exists so an operator can tell which engine
//! answered. "Search is bad" and "search is the fallback" are different tickets.
//!
//! # Why answering is a different route
//!
//! Writing an answer means running a model, and running one means resolving an agent's
//! config from the workspace working copy — which pins that handler to the ide
//! pod. This one reads Postgres and nothing else, so it stays on the fleet.
//!
//! The split is not squeamishness about cost. It is that a search must survive
//! the ide restarting and an answer cannot: pinning a READ to the singleton is
//! the HA bug the fleet design exists to prevent, and sharing a route would
//! have done exactly that to every search in the product.
//!
//! # The property that matters more than relevance
//!
//! A search result is a read, so it goes through
//! [`super::visibility::visible_documents`] like every other read. It is
//! composed here rather than reimplemented, and the gate tests assert that a
//! worker cannot find by searching what they cannot see by listing. A search
//! endpoint that answers from its own index is the classic way a permission
//! model is bypassed without anybody editing the permission model.

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use entity::{documents, folders};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_shared::errors::OxyError;
use sea_orm::sea_query::Expr;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;
use std::collections::HashMap;
use tracing::{info, instrument};
use uuid::Uuid;

use super::dto::{DocumentSummary, FolderNode};
use super::handlers::db_err;
use super::hydrate;
use super::visibility::{ReadStanding, resolve_standing, visible_documents, visible_folders};

/// What a search engine has to be able to do.
///
/// Deliberately narrow. Anything richer — facets, highlighting, an "ask" mode —
/// belongs to whatever replaces the default, and putting it here now would be
/// designing an interface around an implementation nobody has written.
#[async_trait::async_trait]
pub trait DocumentSearch: Send + Sync {
    /// Which engine answered. Surfaced so an operator can tell a bad result
    /// from a fallback result.
    fn name(&self) -> &'static str;

    /// Matching documents the caller may see, most relevant first.
    ///
    /// Takes the standing rather than resolving it, so an implementation
    /// cannot decide to skip the visibility filter — it never holds the inputs
    /// needed to build a different one.
    async fn query(
        &self,
        db: &DatabaseConnection,
        org_id: Uuid,
        caller: Uuid,
        standing: &ReadStanding,
        text: &str,
        mode: MatchMode,
        limit: u64,
    ) -> Result<Vec<documents::Model>, DbErr>;

    /// A written answer, when this engine can produce one.
    ///
    /// Required rather than defaulted, for the same reason `query_folders` is:
    /// an engine that cannot answer says so in its own file instead of
    /// inheriting silence. `PostgresSearch` is a text index and returns `None`.
    ///
    /// # Why this takes rows and not a query
    ///
    /// `candidates` are documents that already passed
    /// [`super::visibility::visible_documents`]. The engine is handed the ROWS
    /// rather than the means to fetch them, and that is the whole security
    /// argument for putting a model here at all: it cannot ask for a different
    /// set, because it holds nothing to ask with. No connection, no query, no
    /// org id, no standing — none of them are in this signature, which is
    /// stronger than a rule saying not to use them.
    ///
    /// The default engine ignores every argument. That is not waste: the
    /// signature is what the NEXT engine is held to.
    ///
    /// # Why this one method does not fail with a `DbErr`
    ///
    /// Every other method here is a query and fails the way a query fails.
    /// This one runs a model, and a model fails for reasons a database has no
    /// word for — no key configured, a rate limit, an agent config that will
    /// not parse. Reporting those as database errors would send an operator to
    /// read Postgres logs about something Postgres never did.
    async fn answer(
        &self,
        question: &str,
        candidates: &[documents::Model],
        bodies: &HashMap<Uuid, String>,
        history: &[Turn],
    ) -> Result<Option<Answer>, OxyError>;

    /// Matching FOLDERS the caller may see.
    ///
    /// A required method rather than one defaulting to empty, so an engine that
    /// does not index folders has to say so instead of silently dropping half
    /// the answer. A search that finds the SOP but not the folder it lives in
    /// sends the reader somewhere they cannot navigate back from.
    async fn query_folders(
        &self,
        db: &DatabaseConnection,
        org_id: Uuid,
        standing: &ReadStanding,
        text: &str,
        limit: u64,
    ) -> Result<Vec<folders::Model>, DbErr>;
}

/// How many of the typed words a document has to contain.
///
/// # Why one index needs two readings
///
/// A search box takes one to three words and means "narrow to these", so every
/// word must be present — that is what makes a second word useful. A question
/// takes eight or ten and means "what is relevant to this", and requiring all
/// of them matches nothing: `plainto_tsquery` ANDs its lexemes, so
/// "how do I dilute the sanitiser" needs a document containing both `dilut`
/// and `sanit` and finds the sanitiser SOP only if it happens to use the word
/// "dilute".
///
/// That is not hypothetical. `POST /documents/ask` shipped with [`Self::All`]
/// and returned zero sources for a question whose answer was the top hit of
/// the same words typed into the search box.
///
/// So the mode is an argument rather than a second query method: retrieval is
/// one implementation, with one visibility filter, read two ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// Every word. What a search box means.
    All,
    /// Any word, ranked by how many matched. What a question means.
    Any,
}

impl MatchMode {
    /// The `tsquery` expression for this mode, as SQL over one bind parameter.
    ///
    /// [`Self::Any`] rewrites Postgres's own parse of the phrase rather than
    /// splitting the words here: `plainto_tsquery` has already stemmed,
    /// lower-cased and dropped the stop words, and its `::text` output is
    /// `'dilut' & 'sanit'`. Turning that `&` into a `|` cannot inject
    /// anything, because the string being rewritten was produced by the parser
    /// and never by the caller — which is exactly why this is not built from
    /// the raw input.
    fn tsquery(self, param: &str) -> String {
        match self {
            MatchMode::All => format!("plainto_tsquery('english', {param})"),
            MatchMode::Any => {
                format!("replace(plainto_tsquery('english', {param})::text, '&', '|')::tsquery")
            }
        }
    }
}

/// One earlier exchange in a session.
///
/// Carries what was asked, what was answered and which documents the answer
/// used — and deliberately **not** their text. A turn that kept the bodies
/// would keep them after the caller stopped being allowed to read them, and
/// the per-turn re-search that makes a session safe would be undone by its own
/// history: the new search omits the document, the transcript hands it over.
#[derive(Debug, Clone)]
pub struct Turn {
    pub question: String,
    pub answer: String,
    pub cited: Vec<Uuid>,
}

/// What an engine that can answer produces.
#[derive(Debug, Clone)]
pub struct Answer {
    pub text: String,
    /// The documents the answer used, in the order it used them.
    ///
    /// Ids rather than titles, because the client already knows how to open one
    /// of these. A citation a reader cannot open is a footnote.
    pub cited: Vec<Uuid>,
}

/// The engine this deployment answers with.
///
/// One env var, defaulting to the text index. An engine that calls a model by
/// default would turn a deployment with no key configured into a search box
/// that errors — which is the failure this whole seam was built to avoid, and
/// it would arrive on the day somebody upgraded rather than the day they chose
/// to.
///
/// Returned boxed so the handler holds one type whichever engine answered.
/// `name()` is what an operator reads to tell a bad result from a fallback one.
pub fn engine() -> Box<dyn DocumentSearch> {
    Box::new(PostgresSearch)
}

/// The default: Postgres full text over the generated `tsvector` columns.
pub struct PostgresSearch;

#[async_trait::async_trait]
impl DocumentSearch for PostgresSearch {
    fn name(&self) -> &'static str {
        "postgres-fts"
    }

    /// No. This is a text index; it ranks documents and has no opinion about
    /// what they say.
    ///
    /// `None` is not a degraded state to apologise for — the Store Ops row
    /// above the matches already renders it, naming the top match rather than
    /// writing a sentence, and its own comment says why: shown *because you can
    /// open it and check, which a generated paragraph is not*. So the client
    /// ships this case today.
    async fn answer(
        &self,
        _question: &str,
        _candidates: &[documents::Model],
        _bodies: &HashMap<Uuid, String>,
        _history: &[Turn],
    ) -> Result<Option<Answer>, OxyError> {
        Ok(None)
    }

    async fn query(
        &self,
        db: &DatabaseConnection,
        org_id: Uuid,
        caller: Uuid,
        standing: &ReadStanding,
        text: &str,
        mode: MatchMode,
        limit: u64,
    ) -> Result<Vec<documents::Model>, DbErr> {
        let Some(visible) = visible_documents(org_id, caller, standing) else {
            return Ok(vec![]);
        };

        // Composed onto the visibility condition, never instead of it. The
        // `EXISTS` reaches the CURRENT version only: older versions are indexed
        // so an audit can ask which one said something, but a search that
        // surfaced a document because of text somebody deleted two versions ago
        // would be answering a question nobody asked.
        //
        // `plainto_tsquery` rather than `to_tsquery` because the input is a
        // person's words, not query syntax — `to_tsquery` raises a syntax error
        // on an unbalanced quote, which turns a typo into a 500.
        let matches = Expr::cust_with_values(
            format!(
                r#"(documents.search_tsv @@ {title}
                OR EXISTS (
                    SELECT 1 FROM document_versions v
                     WHERE v.id = documents.current_version_id
                       AND v.body_tsv @@ {body}))"#,
                title = mode.tsquery("$1"),
                body = mode.tsquery("$2"),
            ),
            [text, text],
        );

        // ORDERED, because there is a LIMIT. Without one Postgres may return
        // any `limit` of the matching rows and a different set on the next
        // identical request, under a trait doc promising "most relevant first"
        // — so the twenty-first best match could outrank the first and the
        // caller would never know the list was arbitrary.
        //
        // `ts_rank` over the title vector, then `updated_at` to break ties
        // deterministically: two documents that match a one-word query equally
        // well is the common case, not the rare one, and the newer of them is
        // the better guess.
        // Under [`MatchMode::Any`] the rank is doing more work than it does
        // for a search box: it is the only thing separating a document that
        // matched one word of the question from one that matched five.
        let rank = Expr::cust_with_values(
            format!("ts_rank(documents.search_tsv, {})", mode.tsquery("$1")),
            [text],
        );

        documents::Entity::find()
            .filter(visible)
            .filter(matches)
            .order_by_desc(rank)
            .order_by_desc(documents::Column::UpdatedAt)
            // `id` last, because the two above are not a total order. A bulk
            // move or a seed run writes many rows in the same microsecond, and
            // equal-ranked, equal-timestamped rows under a LIMIT put an
            // arbitrary subset on the page. `handlers::list` was given this and
            // its comment claimed search already had it; it did not.
            .order_by_desc(documents::Column::Id)
            .limit(limit)
            .all(db)
            .await
    }

    async fn query_folders(
        &self,
        db: &DatabaseConnection,
        org_id: Uuid,
        standing: &ReadStanding,
        text: &str,
        limit: u64,
    ) -> Result<Vec<folders::Model>, DbErr> {
        let Some(visible) = visible_folders(org_id, standing) else {
            return Ok(vec![]);
        };
        // Composed onto the same tree filter the folder listing uses, so a
        // worker cannot find an `hq` folder by name that the tree hides.
        let matches = Expr::cust_with_values(
            "folders.search_tsv @@ plainto_tsquery('english', $1)",
            [text],
        );
        let rank = Expr::cust_with_values(
            "ts_rank(folders.search_tsv, plainto_tsquery('english', $1))",
            [text],
        );
        folders::Entity::find()
            .filter(visible)
            .filter(matches)
            .order_by_desc(rank)
            .order_by_asc(folders::Column::Name)
            // `folders` has no unique index on `name` — only
            // `document_categories` does — so two folders can share one and the
            // pair above is not a total order either.
            .order_by_asc(folders::Column::Id)
            .limit(limit)
            .all(db)
            .await
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub org_id: Uuid,
    /// The words somebody typed.
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: u64,
}

fn default_limit() -> u64 {
    50
}

#[derive(serde::Serialize)]
pub struct SearchResponse {
    /// Which engine answered — see the module docs.
    pub engine: &'static str,
    pub documents: Vec<DocumentSummary>,
    /// Folders whose NAME matched. Separate from the documents rather than
    /// mixed in, because they are a different thing to click: a document opens,
    /// a folder navigates.
    pub folders: Vec<FolderNode>,
}

/// `GET /api/documents/search?org_id=&q=`
///
/// Mounted before `/documents/{id}` in reading order but matched on its own
/// merits: the router prefers a static segment over a parameter, so `search` is
/// never taken for a document id.
#[instrument(skip_all, fields(org = %q.org_id, len = q.q.len()))]
pub async fn search(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, StatusCode> {
    let text = q.q.trim();
    if text.is_empty() {
        // An empty query would match nothing and read as "your library is
        // empty". Saying which of the two it is costs one status code.
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let standing = resolve_standing(&db, user.id, q.org_id)
        .await
        .map_err(db_err)?;

    // `404` before any query runs, like every other read on this surface.
    //
    // Two things were wrong without this, and only one of them is cosmetic. The
    // engine swallowed "no standing" into `Ok(vec![])` and the handler answered
    // `200 {"documents":[],"folders":[]}` for an org the caller has nothing to
    // do with, while `list`, `list_folders` and `categories::list` all answer
    // `404` — so the one route that did not follow the rule was the one that
    // could be used to probe whether a tenant exists.
    //
    // The other is that `folder_counts` below takes an org id and no standing,
    // so it ran an unconditional `GROUP BY` over `documents` for whatever org
    // id was supplied. Empty body, real scan, any authenticated caller, any
    // tenant. `visibility` states the principle this restores: a caller with no
    // standing must be a branch the handler takes, not a filter it trusts to
    // come back empty.
    if matches!(standing, ReadStanding::None) {
        return Err(StatusCode::NOT_FOUND);
    }

    let engine = engine();
    let limit = q.limit.clamp(1, 200);
    let rows = engine
        // `All`: a search box narrows. See [`MatchMode`].
        .query(
            &db,
            q.org_id,
            user.id,
            &standing,
            text,
            MatchMode::All,
            limit,
        )
        .await
        .map_err(db_err)?;

    let folder_rows = engine
        .query_folders(&db, q.org_id, &standing, text, limit)
        .await
        .map_err(db_err)?;

    // The same hydration the listing uses, so a hit and a row show the same
    // author and the same store rather than two renderings of one document.
    let documents = hydrate::summaries(&db, user.id, rows)
        .await
        .map_err(db_err)?;
    let counts = hydrate::folder_counts(&db, q.org_id, user.id, &standing)
        .await
        .map_err(db_err)?;

    info!(
        engine = engine.name(),
        docs = documents.len(),
        folders = folder_rows.len(),
        "document search"
    );
    Ok(Json(SearchResponse {
        engine: engine.name(),
        documents,
        folders: hydrate::folder_nodes(folder_rows, &counts),
    }))
}
