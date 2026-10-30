//! `GET /api/customer-apps/function-failures` — failed production function
//! invocations across every app the caller may see, newest first.
//!
//! The per-app listings (`/{id}/invocations`, `/{id}/issues`) answer "what is
//! wrong with this app". This answers "what failed anywhere between these two
//! times", which is the question that ties one platform bug to every app it
//! broke. On 2026-10-28 an Airhouse regression failed `bookkeeping/post-je` with
//! `ctx.warehouse: failed to convert query results` while
//! `/api/projects/{id}/query` answered 500 with the same words; nothing could
//! put the two side by side. ferret reads this to decide an alert's priority:
//! which apps, whose, and with what error.
//!
//! **What counts as a failure** is the pager's rule,
//! [`counted_as_failure`](super::invocations::counted_as_failure), stated in
//! SQL: a row with a `failure_fingerprint` (which includes a `success` that
//! answered 5xx or caught a failed host call) or an `error` / `timeout` status.
//! The integration test holds the two to the same rows.
//!
//! **Production only.** Staging and sandboxes fail on purpose while people
//! work, and only production pages; a feed that mixed them would make every
//! correlation noisier. The per-app listings still read the others.
//!
//! **Scope.** Rows are filtered to the orgs the caller's platform grant reaches
//! (`scope_org_filter_checked`), so an App Operator bounded to some orgs sees
//! theirs and an out-of-scope `org_id` / `app_id` is an empty page, not a
//! refusal. An unreadable grant is a 500, never "unbounded": the registry list
//! shows rows through a blip, but these rows are tenants' error text. An
//! app-scoped publish token never gets here: the
//! surface's token middleware admits such a token to its own app only.
//!
//! **Bounded.** `since`/`until` (RFC 3339) default to the last 24 hours and may
//! span at most [`MAX_WINDOW_DAYS`] days; `limit` is 1–[`MAX_LIMIT`]. Pages are
//! keyset on `(created_at, id)`: `next_cursor` is opaque, and a page the caller
//! did not finish is never silently the whole answer.
//!
//! **The error text is the tenant function's own**, verbatim and in full: the
//! pager leaves it out of Slack for that reason, and a caller of this route is
//! staff reading it deliberately.
//!
//! **Customer weight** is `org_billing_status`, the org's Stripe subscription
//! state, or `null` for an org with no billing row. It is weak evidence: every
//! create-org path inserts the row as `incomplete`, so `incomplete` means "no
//! Stripe subscription", not "not a customer" — an invoiced customer reads
//! exactly like a trial. `active` is the only strong answer. oxy records nothing
//! else about who pays: the customer list `oxyc customers` reads is a GitHub
//! repo topic with no link to an org id.
//!
//! DB-only (FleetOk). The invocation query walks
//! `idx_app_function_invocations_failure_feed` — failures only, in the feed's
//! `(environment, created_at DESC, id DESC)` order — so a page reads about
//! `limit` index entries, not the window's successes. An unbounded caller
//! naming no org or app gets no `app_id` filter at all; a bounded one adds
//! `app_id = ANY(..)`, checked against those few failure rows.

use std::collections::HashMap;

use axum::Json;
use axum::extract::Query;
use axum::extract::rejection::QueryRejection;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::prelude::{AppFunctionInvocations, Apps, Organizations};
use entity::{app_function_invocations, apps, org_billing, organizations};
use migration::FUNCTION_INVOCATION_FAILURE_PREDICATE as FAILURE_PREDICATE;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::environment_scope::ScopeError;
use super::handlers::scope_org_filter_checked;

/// Rows returned when the caller names no `limit`.
pub const DEFAULT_LIMIT: u64 = 100;
/// The most rows one page returns.
pub const MAX_LIMIT: u64 = 500;
/// The widest `since`..`until` one request may ask for.
pub const MAX_WINDOW_DAYS: i64 = 7;
/// The window when the caller names no `since`.
const DEFAULT_WINDOW_HOURS: i64 = 24;

/// `?since=&until=&limit=&cursor=&org_id=&app_id=`.
#[derive(Debug, Default, Deserialize)]
pub struct FailuresQuery {
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u64>,
    pub cursor: Option<String>,
    pub org_id: Option<Uuid>,
    pub app_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct FailuresPage {
    pub failures: Vec<FunctionFailure>,
    /// Pass back as `cursor` for the next, older page. `None`: this was the last.
    pub next_cursor: Option<String>,
    /// The window actually read, after defaults.
    pub since: String,
    pub until: String,
}

/// One failed production invocation, with whose app it was.
#[derive(Debug, Serialize)]
pub struct FunctionFailure {
    pub invocation_id: Uuid,
    pub invoked_at: String,
    pub org_id: Uuid,
    pub org_slug: Option<String>,
    pub org_name: Option<String>,
    /// `active` | `past_due` | `unpaid` | `canceled` | `incomplete`, or `None`
    /// for an org with no billing row.
    pub org_billing_status: Option<String>,
    pub app_id: Uuid,
    pub app_slug: String,
    pub function_name: String,
    /// `route` | `schedule` | `manual` | `airway`.
    pub mode: String,
    /// The recorded status, verbatim — a counted failure can be a `success`.
    pub status: String,
    /// `threw` | `internal` | `exceeded_memory` | `platform` | `timeout` |
    /// `http_5xx` | `host_call`; `None` for a row written before kinds were.
    pub kind: Option<String>,
    /// The pager's key. The same fingerprint on two apps is one failure.
    pub fingerprint: Option<String>,
    /// The full error text, as the function produced it.
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    /// `app_builds.id` of the build that ran.
    pub build_uuid: Uuid,
}

/// The validated request: a window, a page size, and where the page starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub limit: u64,
    pub after: Option<Cursor>,
    pub org_id: Option<Uuid>,
    pub app_id: Option<Uuid>,
}

/// The last row of a page: the next page holds rows strictly older than it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

impl Cursor {
    /// `<unix microseconds>.<uuid>` — opaque to callers, exact to the row.
    pub fn encode(&self) -> String {
        format!("{}.{}", self.created_at.timestamp_micros(), self.id)
    }

    pub fn decode(raw: &str) -> Option<Self> {
        let (micros, id) = raw.split_once('.')?;
        Some(Self {
            created_at: DateTime::from_timestamp_micros(micros.parse().ok()?)?,
            id: id.parse().ok()?,
        })
    }
}

fn bad(code: &'static str, message: impl Into<String>) -> ScopeError {
    ScopeError::new(StatusCode::BAD_REQUEST, code, message)
}

fn parse_time(field: &str, raw: Option<&str>) -> Result<Option<DateTime<Utc>>, ScopeError> {
    let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(raw)
        .map(|t| Some(t.with_timezone(&Utc)))
        .map_err(|e| bad("invalid_time", format!("{field} must be RFC 3339: {e}")))
}

impl Request {
    /// `q` checked and defaulted against `now`. Out of range is an error rather
    /// than a clamp: a caller who asked for a month and got a week would read a
    /// truncated history as the whole one.
    pub fn from_query(q: &FailuresQuery, now: DateTime<Utc>) -> Result<Self, ScopeError> {
        let until = parse_time("until", q.until.as_deref())?.unwrap_or(now);
        let since = parse_time("since", q.since.as_deref())?
            .unwrap_or(until - Duration::hours(DEFAULT_WINDOW_HOURS));
        if since >= until {
            return Err(bad("invalid_window", "since must be before until"));
        }
        if until - since > Duration::days(MAX_WINDOW_DAYS) {
            return Err(bad(
                "invalid_window",
                format!("since..until may span at most {MAX_WINDOW_DAYS} days"),
            ));
        }
        let limit = match q.limit {
            None => DEFAULT_LIMIT,
            Some(n) if (1..=MAX_LIMIT).contains(&n) => n,
            Some(n) => {
                return Err(bad(
                    "invalid_limit",
                    format!("limit must be between 1 and {MAX_LIMIT}; got {n}"),
                ));
            }
        };
        let after = match q.cursor.as_deref().filter(|c| !c.is_empty()) {
            None => None,
            Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| {
                bad("invalid_cursor", "cursor is not one this endpoint returned")
            })?),
        };
        Ok(Self {
            since,
            until,
            limit,
            after,
            org_id: q.org_id,
            app_id: q.app_id,
        })
    }
}

pub async fn list_function_failures(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    query: Result<Query<FailuresQuery>, QueryRejection>,
) -> Result<Json<FailuresPage>, ScopeError> {
    let Query(q) = query.map_err(|e| bad("invalid_query", e.to_string()))?;
    let request = Request::from_query(&q, Utc::now())?;
    let db = establish_connection()
        .await
        .map_err(|e| ScopeError::internal("DB connection error", e))?;
    // Capabilities gate the route; the grant's scope filters the rows. Fails
    // closed, unlike the registry list: these rows are tenants' own error
    // text, and an unreadable grant must not read as "every org".
    let scope = scope_org_filter_checked(&db, &user)
        .await
        .map_err(|e| ScopeError::internal("platform grant unreadable", e))?;
    Ok(Json(failures(&db, scope.as_deref(), &request).await?))
}

/// One page of `request`, limited to the apps of `scope`'s orgs (`None`: all).
pub async fn failures(
    db: &DatabaseConnection,
    scope: Option<&[Uuid]>,
    request: &Request,
) -> Result<FailuresPage, ScopeError> {
    // An unbounded caller naming no org or app reads every app: no `app_id`
    // filter at all, so the failure-feed index answers the page directly
    // instead of every app's rows being gathered and sorted.
    let app_ids = if scope.is_none() && request.org_id.is_none() && request.app_id.is_none() {
        None
    } else {
        Some(app_ids_in_scope(db, scope, request).await?)
    };
    let mut rows = match app_ids {
        Some(ids) if ids.is_empty() => Vec::new(),
        ids => failed_rows(db, ids, request).await?,
    };
    let next_cursor = if rows.len() as u64 > request.limit {
        rows.truncate(request.limit as usize);
        rows.last().map(|r| {
            Cursor {
                created_at: r.created_at.with_timezone(&Utc),
                id: r.id,
            }
            .encode()
        })
    } else {
        None
    };
    let apps = app_refs(db, rows.iter().map(|r| r.app_id)).await?;
    let orgs = org_facts(db, apps.values().map(|a| a.org_id)).await?;
    let failures = rows
        .into_iter()
        .filter_map(|row| {
            let app = apps.get(&row.app_id)?;
            Some(failure(row, app, orgs.get(&app.org_id)))
        })
        .collect();
    Ok(FailuresPage {
        failures,
        next_cursor,
        since: request.since.to_rfc3339(),
        until: request.until.to_rfc3339(),
    })
}

/// The ids of the apps whose failures `request` may list: `scope`'s orgs,
/// narrowed by `org_id` / `app_id`. Ids only — the rows are not needed here.
async fn app_ids_in_scope(
    db: &DatabaseConnection,
    scope: Option<&[Uuid]>,
    request: &Request,
) -> Result<Vec<Uuid>, ScopeError> {
    let mut find = Apps::find().select_only().column(apps::Column::Id);
    if let Some(orgs) = scope {
        find = find.filter(apps::Column::OrgId.is_in(orgs.iter().copied()));
    }
    if let Some(org_id) = request.org_id {
        find = find.filter(apps::Column::OrgId.eq(org_id));
    }
    if let Some(app_id) = request.app_id {
        find = find.filter(apps::Column::Id.eq(app_id));
    }
    find.into_tuple()
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("apps lookup failed", e))
}

/// The slice of an `apps` row a failure row carries.
#[derive(Debug, Clone)]
struct AppRef {
    id: Uuid,
    org_id: Uuid,
    slug: String,
}

/// The apps `ids` name, by id — three columns, not the whole row.
async fn app_refs(
    db: &DatabaseConnection,
    ids: impl Iterator<Item = Uuid>,
) -> Result<HashMap<Uuid, AppRef>, ScopeError> {
    let ids: std::collections::BTreeSet<Uuid> = ids.collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Uuid, String)> = Apps::find()
        .select_only()
        .columns([apps::Column::Id, apps::Column::OrgId, apps::Column::Slug])
        .filter(apps::Column::Id.is_in(ids))
        .into_tuple()
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("apps lookup failed", e))?;
    Ok(rows
        .into_iter()
        .map(|(id, org_id, slug)| (id, AppRef { id, org_id, slug }))
        .collect())
}

/// `request.limit + 1` failed production rows, newest first — the extra row
/// only says whether another page exists. `app_ids: None` is every app.
///
/// The failure filter is the migration's [`FAILURE_PREDICATE`] verbatim: the
/// planner uses a partial index only when it can prove the query implies the
/// index's predicate. SeaQuery parenthesises a custom expression in the `AND`
/// chain, so its `OR` stays inside it.
async fn failed_rows(
    db: &DatabaseConnection,
    app_ids: Option<Vec<Uuid>>,
    request: &Request,
) -> Result<Vec<app_function_invocations::Model>, ScopeError> {
    failed_rows_query(app_ids, request)
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("function failure query failed", e))
}

/// The query [`failed_rows`] runs — public so a test can `EXPLAIN` it.
pub fn failed_rows_query(
    app_ids: Option<Vec<Uuid>>,
    request: &Request,
) -> sea_orm::Select<AppFunctionInvocations> {
    use app_function_invocations::Column as C;
    let mut find = AppFunctionInvocations::find()
        .filter(C::Environment.eq("production"))
        .filter(C::CreatedAt.gte(request.since))
        .filter(C::CreatedAt.lt(request.until))
        .filter(Expr::cust(FAILURE_PREDICATE));
    if let Some(ids) = app_ids {
        find = find.filter(C::AppId.is_in(ids));
    }
    if let Some(after) = request.after {
        find = find.filter(
            Condition::any().add(C::CreatedAt.lt(after.created_at)).add(
                Condition::all()
                    .add(C::CreatedAt.eq(after.created_at))
                    .add(C::Id.lt(after.id)),
            ),
        );
    }
    find.order_by_desc(C::CreatedAt)
        .order_by_desc(C::Id)
        .limit(request.limit + 1)
}

/// What one org contributes to a row: slug, name, billing status.
#[derive(Debug, Default, Clone)]
struct OrgFacts {
    slug: Option<String>,
    name: Option<String>,
    billing_status: Option<String>,
}

async fn org_facts(
    db: &DatabaseConnection,
    ids: impl Iterator<Item = Uuid>,
) -> Result<HashMap<Uuid, OrgFacts>, ScopeError> {
    let ids: std::collections::BTreeSet<Uuid> = ids.collect();
    let mut out: HashMap<Uuid, OrgFacts> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let orgs = Organizations::find()
        .filter(organizations::Column::Id.is_in(ids.iter().copied()))
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("organizations lookup failed", e))?;
    for org in orgs {
        let facts = out.entry(org.id).or_default();
        facts.slug = Some(org.slug);
        facts.name = Some(org.name);
    }
    let billing = org_billing::Entity::find()
        .filter(org_billing::Column::OrgId.is_in(ids.iter().copied()))
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("org billing lookup failed", e))?;
    for row in billing {
        out.entry(row.org_id).or_default().billing_status = Some(row.status.as_str().to_string());
    }
    Ok(out)
}

fn failure(
    row: app_function_invocations::Model,
    app: &AppRef,
    org: Option<&OrgFacts>,
) -> FunctionFailure {
    let org = org.cloned().unwrap_or_default();
    FunctionFailure {
        invocation_id: row.id,
        invoked_at: row.created_at.to_rfc3339(),
        org_id: app.org_id,
        org_slug: org.slug,
        org_name: org.name,
        org_billing_status: org.billing_status,
        app_id: app.id,
        app_slug: app.slug.clone(),
        function_name: row.function_name,
        mode: row.mode,
        status: row.status,
        kind: row.error_kind,
        fingerprint: row.failure_fingerprint,
        error: row.error,
        duration_ms: row.duration_ms,
        build_uuid: row.build_id,
    }
}

#[cfg(test)]
#[path = "function_failures_tests.rs"]
mod tests;
