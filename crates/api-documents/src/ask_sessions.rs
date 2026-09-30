//! Sessions: what somebody asked the library, kept so they can look again.
//!
//! # Why these are on the fleet while [`super::ask`] is not
//!
//! Asking runs a model, which means reading an agent config off the workspace
//! working copy, which pins that one handler to the ide singleton. These four
//! read and write Postgres and nothing else, so they stay `FleetOk` — the same
//! rule that kept `/documents/search` off the singleton. Note the segment
//! counts: `/documents/ask` is `IdeOnly` and `/documents/ask/sessions` is not,
//! and the guard test asserts both so a later change cannot collapse them.
//!
//! # Authorization is ownership, and there is no override
//!
//! A session belongs to the person who opened it. An officer cannot read
//! somebody else's, and that is a decision rather than an omission: reading
//! what a worker asked is surveillance, not administration. The org standing
//! is still checked, so a caller with no standing gets `404` before any row is
//! touched — but standing alone never reaches another person's transcript.
//!
//! # What a stored turn may carry
//!
//! The question, the prose, and the ids the answer cited. Never document text.
//! [`turns_of`] rebuilds [`Turn`]s from storage for the next prompt, and the
//! type it rebuilds them into has no field for a body — which is what makes
//! "a session cannot become a second copy of the library" a property of the
//! types rather than a rule somebody has to remember.

use axum::Json;
use axum::extract::{OriginalUri, Path, Query};
use axum::http::StatusCode;
use entity::{document_ask_sessions, document_ask_turns};
use oxy::database::client::establish_connection;
use oxy_app_core::pagination::{self, Paged, trim_overfetch};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, ModelTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::Deserialize;
use uuid::Uuid;

use super::dto::DocumentSummary;
use super::handlers::db_err;
use super::hydrate;
use super::search::Turn;
use super::visibility::{ReadStanding, resolve_standing, visible_documents};

/// How many turns of a stored session are replayed into the next prompt.
///
/// The same window [`super::ask`] applies to a client-sent history, stated once
/// and used by both, so a session that lives on the server and one that does
/// not cost the same to continue.
pub const REPLAY_TURNS: u64 = 8;

#[derive(Debug, Deserialize)]
pub struct OrgQuery {
    pub org_id: Uuid,
}

#[derive(Debug, serde::Serialize)]
pub struct SessionSummary {
    pub id: Uuid,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
    /// What the conversation was about, which is almost always its first
    /// question. Sent so the list can be read without opening anything.
    pub first_question: Option<String>,
    pub turns: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct TurnBody {
    pub question: String,
    pub answer: Option<String>,
    /// What the `[1]`, `[2]` markers in [`Self::answer`] point at, by position.
    ///
    /// A slot the caller may no longer open comes back as `null` rather than
    /// being removed, because removing it would renumber every marker after it
    /// and silently repoint the prose at the wrong document. A `null` slot
    /// leaves its marker as plain text, which is what the client already does
    /// with a marker it cannot resolve.
    pub sources: Vec<Option<Uuid>>,
    /// The subset the answer used, filtered to what the caller may open TODAY.
    ///
    /// A citation that survives is one they can click; one that does not simply
    /// is not there, rather than being a title they are no longer allowed to
    /// know exists.
    pub cited: Vec<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct SessionDetail {
    pub id: Uuid,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub turns: Vec<TurnBody>,
    /// Every document still cited anywhere in this session, hydrated once.
    ///
    /// The transcript's prose is left as it was written. That is deliberate and
    /// worth stating: it is a record of what the reader was already told, and
    /// unsaying it later would be a different and stranger product. What
    /// changes with permissions is what they can still OPEN.
    pub documents: Vec<DocumentSummary>,
}

/// `POST /api/documents/ask/sessions`
///
/// Opens an empty session and returns its id. Called when somebody starts a
/// conversation, never on a one-shot ask — which is why a single question
/// leaves nothing behind and a list of sessions is a list of conversations
/// rather than a log of every keystroke.
pub async fn create(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<OrgQuery>,
) -> Result<(StatusCode, Json<SessionSummary>), StatusCode> {
    let db = connect().await?;
    guard_standing(&db, user.id, q.org_id).await?;

    let now = chrono::Utc::now().fixed_offset();
    let row = document_ask_sessions::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(q.org_id),
        user_id: ActiveValue::Set(user.id),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(&db)
    .await
    .map_err(db_err)?;

    Ok((
        StatusCode::CREATED,
        Json(SessionSummary {
            id: row.id,
            created_at: row.created_at,
            updated_at: row.updated_at,
            first_question: None,
            turns: 0,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub org_id: Uuid,
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub offset: u64,
}

fn default_limit() -> u64 {
    20
}

/// `GET /api/documents/ask/sessions?org_id=&limit=&offset=`
///
/// Mine, in this org, most recent first — which is exactly the index the
/// migration creates.
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    // `OriginalUri`, not `Uri`. The router nests this under `/api`, and a bare
    // `Uri` has that prefix stripped — so the `Link` would point one path
    // segment short, at a route that does not exist. That shipped once already,
    // in `handlers::list`.
    OriginalUri(uri): OriginalUri,
    Query(q): Query<ListQuery>,
) -> Result<Paged<SessionSummary>, StatusCode> {
    let db = connect().await?;
    guard_standing(&db, user.id, q.org_id).await?;

    let limit = q.limit.clamp(1, 100);
    // One more than asked for, so "is there a next page" is answered by the
    // rows rather than by a second COUNT over the same index.
    let mut rows = document_ask_sessions::Entity::find()
        .filter(document_ask_sessions::Column::UserId.eq(user.id))
        .filter(document_ask_sessions::Column::OrgId.eq(q.org_id))
        .order_by_desc(document_ask_sessions::Column::UpdatedAt)
        // `id` last: two sessions updated in the same microsecond are not
        // ordered by the column above, and an unordered LIMIT puts an
        // arbitrary subset on the page.
        .order_by_desc(document_ask_sessions::Column::Id)
        .offset(q.offset)
        .limit(limit + 1)
        .all(&db)
        .await
        .map_err(db_err)?;

    let more = trim_overfetch(&mut rows, limit);

    let mut items = Vec::with_capacity(rows.len());
    for s in rows {
        let (first_question, turns) = head_of(&db, s.id).await.map_err(db_err)?;
        items.push(SessionSummary {
            id: s.id,
            created_at: s.created_at,
            updated_at: s.updated_at,
            first_question,
            turns,
        });
    }

    // Routed through `page` even when this is the only page, so the response
    // always carries `rel="first"`. An endpoint answering with no `Link` is
    // byte-for-byte one that never paged.
    Ok(pagination::page(
        items,
        more,
        &uri,
        &[("offset", q.offset.saturating_add(limit).to_string())],
    ))
}

/// `GET /api/documents/ask/sessions/{id}`
pub async fn read(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Result<Json<SessionDetail>, StatusCode> {
    let db = connect().await?;
    let session = mine(&db, user.id, id).await?;
    let standing = resolve_standing(&db, user.id, session.org_id)
        .await
        .map_err(db_err)?;
    if matches!(standing, ReadStanding::None) {
        return Err(StatusCode::NOT_FOUND);
    }

    let rows = turn_rows(&db, id, None).await.map_err(db_err)?;
    let all: Vec<Uuid> = rows
        .iter()
        .flat_map(|t| t.source_ids().into_iter().chain(t.cited_ids()))
        .collect();
    let open = openable(&db, session.org_id, user.id, &standing, &all)
        .await
        .map_err(db_err)?;
    let readable = |id: &Uuid| open.iter().any(|d| d.id == *id);

    let turns = rows
        .iter()
        .map(|t| TurnBody {
            question: t.question.clone(),
            answer: t.answer.clone(),
            sources: t
                .source_ids()
                .into_iter()
                .map(|id| readable(&id).then_some(id))
                .collect(),
            cited: t.cited_ids().into_iter().filter(readable).collect(),
        })
        .collect();

    let documents = hydrate::summaries(&db, user.id, open)
        .await
        .map_err(db_err)?;
    Ok(Json(SessionDetail {
        id: session.id,
        created_at: session.created_at,
        turns,
        documents,
    }))
}

/// `DELETE /api/documents/ask/sessions/{id}`
///
/// A real delete, not a tombstone. The trash exists for documents because
/// somebody else may need one back; a transcript has exactly one reader, and
/// "I want this gone" from that reader is the whole requirement. The turns go
/// with it by `ON DELETE CASCADE`.
pub async fn delete(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let db = connect().await?;
    let session = mine(&db, user.id, id).await?;
    session.delete(&db).await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ── what `ask` uses ──

/// The session's recent turns, oldest first, as the engine's history type.
///
/// Turns whose every citation has become unreadable are dropped. A turn's prose
/// was written from documents the caller could open at the time, and replaying
/// it into a NEW prompt after they lost that access would re-disclose the
/// content through the back door the per-turn re-search exists to close. A turn
/// that cited nothing is kept: there is nothing in it that a permission change
/// could have taken away.
pub async fn turns_of(
    db: &DatabaseConnection,
    session_id: Uuid,
    org_id: Uuid,
    caller: Uuid,
    standing: &ReadStanding,
) -> Result<Vec<Turn>, DbErr> {
    let rows = turn_rows(db, session_id, Some(REPLAY_TURNS)).await?;
    let all: Vec<Uuid> = rows.iter().flat_map(|t| t.cited_ids()).collect();
    let open = openable(db, org_id, caller, standing, &all).await?;

    Ok(rows
        .into_iter()
        .filter_map(|t| {
            let cited = t.cited_ids();
            let kept: Vec<Uuid> = cited
                .iter()
                .copied()
                .filter(|id| open.iter().any(|d| d.id == *id))
                .collect();
            if !cited.is_empty() && kept.is_empty() {
                return None;
            }
            Some(Turn {
                question: t.question,
                answer: t.answer.unwrap_or_default(),
                cited: kept,
            })
        })
        .collect())
}

/// Append a turn and bump the session, or do nothing if the session is not the
/// caller's.
///
/// Silent rather than fallible on purpose: this runs after the answer is
/// written, and a failure to record what somebody was told must not turn a
/// good answer into a `500`. The caller logs.
pub async fn record(
    db: &DatabaseConnection,
    session_id: Uuid,
    caller: Uuid,
    question: &str,
    answer: Option<&str>,
    sources: &[Uuid],
    cited: &[Uuid],
) -> Result<(), DbErr> {
    let Some(session) = document_ask_sessions::Entity::find_by_id(session_id)
        .filter(document_ask_sessions::Column::UserId.eq(caller))
        .one(db)
        .await?
    else {
        return Ok(());
    };

    let seq = document_ask_turns::Entity::find()
        .filter(document_ask_turns::Column::SessionId.eq(session_id))
        .count(db)
        .await? as i32
        + 1;

    let now = chrono::Utc::now().fixed_offset();
    document_ask_turns::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        session_id: ActiveValue::Set(session_id),
        seq: ActiveValue::Set(seq),
        question: ActiveValue::Set(question.to_string()),
        answer: ActiveValue::Set(answer.map(str::to_string)),
        sources: ActiveValue::Set(serde_json::json!(sources)),
        cited: ActiveValue::Set(serde_json::json!(cited)),
        created_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await?;

    let mut am: document_ask_sessions::ActiveModel = session.into();
    am.updated_at = ActiveValue::Set(now);
    am.update(db).await?;
    Ok(())
}

// ── helpers ──

async fn connect() -> Result<DatabaseConnection, StatusCode> {
    establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

/// `404` for a caller with no standing in the org — the same answer every other
/// read on this surface gives, and made before any row is touched.
async fn guard_standing(db: &DatabaseConnection, user: Uuid, org: Uuid) -> Result<(), StatusCode> {
    match resolve_standing(db, user, org).await.map_err(db_err)? {
        ReadStanding::None => Err(StatusCode::NOT_FOUND),
        _ => Ok(()),
    }
}

/// The session, if it is the caller's.
///
/// `404` rather than `403` for somebody else's: whether a stranger's
/// conversation exists is not a fact this endpoint should confirm.
async fn mine(
    db: &DatabaseConnection,
    user: Uuid,
    id: Uuid,
) -> Result<document_ask_sessions::Model, StatusCode> {
    document_ask_sessions::Entity::find_by_id(id)
        .filter(document_ask_sessions::Column::UserId.eq(user))
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)
}

/// A session's turns, oldest first.
///
/// `limit` is `Some` for a replay — the LAST n turns, because a conversation is
/// continued from its recent end — and `None` for the transcript, which shows
/// what it shows.
///
/// `None` rather than `u64::MAX`, which is not a large limit but a panic:
/// sea-query converts the bound to an `i64` and unwraps, so the first read of a
/// stored session died in `TryFromIntError(PosOverflow)` several layers below
/// anything this file mentions.
async fn turn_rows(
    db: &DatabaseConnection,
    session_id: Uuid,
    limit: Option<u64>,
) -> Result<Vec<document_ask_turns::Model>, DbErr> {
    let mut query = document_ask_turns::Entity::find()
        .filter(document_ask_turns::Column::SessionId.eq(session_id))
        .order_by_desc(document_ask_turns::Column::Seq);
    if let Some(n) = limit {
        query = query.limit(n);
    }
    let mut rows = query.all(db).await?;
    rows.reverse();
    Ok(rows)
}

/// Which of these documents the caller may open right now.
///
/// Composed onto [`visible_documents`] rather than checking the ids against
/// anything of its own, so a transcript is filtered by the one rule every other
/// read uses.
async fn openable(
    db: &DatabaseConnection,
    org_id: Uuid,
    caller: Uuid,
    standing: &ReadStanding,
    ids: &[Uuid],
) -> Result<Vec<entity::documents::Model>, DbErr> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let Some(visible) = visible_documents(org_id, caller, standing) else {
        return Ok(vec![]);
    };
    entity::documents::Entity::find()
        .filter(visible)
        .filter(entity::documents::Column::Id.is_in(ids.to_vec()))
        .all(db)
        .await
}

/// The first question and the turn count, for one row of the list.
async fn head_of(
    db: &DatabaseConnection,
    session_id: Uuid,
) -> Result<(Option<String>, u64), DbErr> {
    let first = document_ask_turns::Entity::find()
        .filter(document_ask_turns::Column::SessionId.eq(session_id))
        .order_by_asc(document_ask_turns::Column::Seq)
        .one(db)
        .await?
        .map(|t| t.question);
    let turns = document_ask_turns::Entity::find()
        .filter(document_ask_turns::Column::SessionId.eq(session_id))
        .count(db)
        .await?;
    Ok((first, turns))
}
