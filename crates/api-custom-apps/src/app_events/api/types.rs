//! `GET …/events/types`: every event type of the org, with its publisher,
//! schema revision and last publish, and for each listener in each
//! environment its health, read live from the deliveries and their attempts,
//! and where its subscription came from. A type is listed when an app of the
//! org declares it, a function listens to it, or it was published.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
};
use serde::Serialize;
use uuid::Uuid;

use super::reply::ApiError;
use crate::app_events::schema::live_revision;
use oxy_custom_apps::events::table::is_table_event_type;

#[derive(Debug, Serialize)]
pub(super) struct TypesReply {
    types: Vec<TypeRow>,
    /// The functions that run on a table, with where each cursor is.
    tables: Vec<super::tables::TableRow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TypeRow {
    #[serde(rename = "type")]
    event_type: String,
    /// The publishing app's slug.
    publisher: String,
    /// The event's name within its publisher.
    event: String,
    /// The revision production's build declares, else the newest recorded.
    schema_revision: Option<i32>,
    last_published_at: Option<DateTime<Utc>>,
    listeners: Vec<ListenerHealth>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListenerHealth {
    /// `<app>/<function>`.
    listener: String,
    app: String,
    function: String,
    environment: String,
    pending: i64,
    /// Pending with at least one attempt.
    retrying: i64,
    dead: i64,
    held: i64,
    /// How long the oldest pending delivery has been pending (since it was
    /// created or last redelivered).
    oldest_pending_age_seconds: Option<f64>,
    last_success_at: Option<DateTime<Utc>>,
    last_failure_at: Option<DateTime<Utc>>,
    last_failure_outcome: Option<String>,
    /// Attempts open on pending deliveries.
    in_flight: i64,
    /// The build the subscription follows, and when it was last reconciled.
    build_id: Option<Uuid>,
    reconciled_at: Option<DateTime<Utc>>,
}

/// One listener of one type in one environment.
type Key = (String, Uuid, String, String);

const DELIVERY_HEALTH_SQL: &str = "\
SELECT e.type AS event_type, d.app_id, d.function_name, d.environment, \
       count(*) FILTER (WHERE d.status = 'pending') AS pending, \
       count(*) FILTER (WHERE d.status = 'pending' AND d.attempts > 0) AS retrying, \
       count(*) FILTER (WHERE d.status = 'dead') AS dead, \
       count(*) FILTER (WHERE d.status = 'held') AS held, \
       EXTRACT(EPOCH FROM now() - min(COALESCE(b.restarted_at, d.created_at)) \
               FILTER (WHERE d.status = 'pending'))::float8 AS oldest \
  FROM app_event_deliveries d \
  JOIN app_events e ON e.id = d.event_id \
  LEFT JOIN app_event_delivery_budgets b ON b.delivery_id = d.id \
 WHERE e.org_id = $1 \
 GROUP BY 1, 2, 3, 4";

const ATTEMPT_HEALTH_SQL: &str = "\
SELECT e.type AS event_type, d.app_id, d.function_name, d.environment, \
       max(a.finished_at) FILTER (WHERE a.outcome = 'ok') AS last_success_at, \
       max(a.finished_at) FILTER (WHERE a.outcome <> 'ok') AS last_failure_at, \
       (array_agg(a.outcome ORDER BY a.finished_at DESC) \
           FILTER (WHERE a.outcome <> 'ok'))[1] AS last_failure_outcome, \
       count(*) FILTER (WHERE a.finished_at IS NULL AND d.status = 'pending') AS in_flight \
  FROM app_event_delivery_attempts a \
  JOIN app_event_deliveries d ON d.id = a.delivery_id \
  JOIN app_events e ON e.id = d.event_id \
 WHERE e.org_id = $1 \
 GROUP BY 1, 2, 3, 4";

const SUBSCRIPTIONS_SQL: &str = "\
SELECT event_type, app_id, function_name, environment, build_id, updated_at \
  FROM app_event_subscriptions WHERE org_id = $1";

const DECLARED_SQL: &str = "\
SELECT s.app_id, a.slug, s.event_name, max(s.revision) AS revision \
  FROM app_event_schemas s JOIN apps a ON a.id = s.app_id \
 WHERE a.org_id = $1 GROUP BY 1, 2, 3";

const LAST_PUBLISHED_SQL: &str = "\
SELECT type AS event_type, max(published_at) AS last_published_at \
  FROM app_events WHERE org_id = $1 GROUP BY 1";

/// The reply for `org_id`.
pub(super) async fn list(db: &DatabaseConnection, org_id: Uuid) -> Result<TypesReply, ApiError> {
    let read = |e| ApiError::db("read the org's event types", e);
    let slugs = app_slugs(db, org_id).await.map_err(read)?;
    let mut listeners = health(db, org_id, &slugs).await.map_err(read)?;
    let mut types = declared(db, org_id).await.map_err(read)?;
    for row in rows(db, LAST_PUBLISHED_SQL, org_id).await.map_err(read)? {
        let event_type: String = row.try_get("", "event_type").map_err(read)?;
        // A table's changes are listed under `tables`, not as an event type.
        if is_table_event_type(&event_type) {
            continue;
        }
        let entry = types
            .entry(event_type.clone())
            .or_insert_with(|| undeclared(&event_type));
        entry.last_published_at = row.try_get("", "last_published_at").map_err(read)?;
    }
    for ((event_type, ..), health) in std::mem::take(&mut listeners) {
        if is_table_event_type(&event_type) {
            continue;
        }
        types
            .entry(event_type.clone())
            .or_insert_with(|| undeclared(&event_type))
            .listeners
            .push(health);
    }
    let mut types: Vec<TypeRow> = types.into_values().collect();
    for row in &mut types {
        row.listeners
            .sort_by(|a, b| (&a.listener, &a.environment).cmp(&(&b.listener, &b.environment)));
    }
    let tables = super::tables::list(db, org_id).await.map_err(read)?;
    Ok(TypesReply { types, tables })
}

/// The types the org's apps declare, each at production's revision (else the
/// newest recorded).
async fn declared(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<BTreeMap<String, TypeRow>, DbErr> {
    let mut types = BTreeMap::new();
    for row in rows(db, DECLARED_SQL, org_id).await? {
        let app_id: Uuid = row.try_get("", "app_id")?;
        let publisher: String = row.try_get("", "slug")?;
        let event: String = row.try_get("", "event_name")?;
        let newest: Option<i32> = row.try_get("", "revision")?;
        let live = live_revision(db, app_id, "production", &event).await?;
        let event_type = oxy_custom_apps::events::qualified_type(&publisher, &event);
        let entry = TypeRow {
            event_type: event_type.clone(),
            publisher,
            event,
            schema_revision: live.map(|r| r.revision).or(newest),
            last_published_at: None,
            listeners: Vec::new(),
        };
        types.insert(event_type, entry);
    }
    Ok(types)
}

/// A type no app of the org declares (a listener names it, or an event
/// was published before its declaration went): its publisher from its name.
fn undeclared(event_type: &str) -> TypeRow {
    let (publisher, event) = event_type.split_once('/').unwrap_or(("", event_type));
    TypeRow {
        event_type: event_type.to_string(),
        publisher: publisher.to_string(),
        event: event.to_string(),
        schema_revision: None,
        last_published_at: None,
        listeners: Vec::new(),
    }
}

/// Each listener's health, from its subscription and its deliveries.
async fn health(
    db: &DatabaseConnection,
    org_id: Uuid,
    slugs: &HashMap<Uuid, String>,
) -> Result<BTreeMap<Key, ListenerHealth>, DbErr> {
    let mut found: BTreeMap<Key, ListenerHealth> = BTreeMap::new();
    for row in rows(db, SUBSCRIPTIONS_SQL, org_id).await? {
        let entry = listener_entry(&mut found, &row, slugs)?;
        entry.build_id = row.try_get("", "build_id")?;
        entry.reconciled_at = row.try_get("", "updated_at")?;
    }
    for row in rows(db, DELIVERY_HEALTH_SQL, org_id).await? {
        let entry = listener_entry(&mut found, &row, slugs)?;
        entry.pending = row.try_get("", "pending")?;
        entry.retrying = row.try_get("", "retrying")?;
        entry.dead = row.try_get("", "dead")?;
        entry.held = row.try_get("", "held")?;
        entry.oldest_pending_age_seconds = row.try_get("", "oldest")?;
    }
    for row in rows(db, ATTEMPT_HEALTH_SQL, org_id).await? {
        let entry = listener_entry(&mut found, &row, slugs)?;
        entry.last_success_at = row.try_get("", "last_success_at")?;
        entry.last_failure_at = row.try_get("", "last_failure_at")?;
        entry.last_failure_outcome = row.try_get("", "last_failure_outcome")?;
        entry.in_flight = row.try_get("", "in_flight")?;
    }
    Ok(found)
}

/// The entry `row` (`event_type, app_id, function_name, environment`) is about.
fn listener_entry<'a>(
    found: &'a mut BTreeMap<Key, ListenerHealth>,
    row: &QueryResult,
    slugs: &HashMap<Uuid, String>,
) -> Result<&'a mut ListenerHealth, DbErr> {
    let event_type: String = row.try_get("", "event_type")?;
    let app_id: Uuid = row.try_get("", "app_id")?;
    let function: String = row.try_get("", "function_name")?;
    let environment: String = row.try_get("", "environment")?;
    let app = slugs
        .get(&app_id)
        .cloned()
        .unwrap_or_else(|| app_id.to_string());
    let key = (event_type, app_id, function.clone(), environment.clone());
    Ok(found.entry(key).or_insert_with(|| ListenerHealth {
        listener: format!("{app}/{function}"),
        app,
        function,
        environment,
        ..Default::default()
    }))
}

async fn app_slugs(db: &DatabaseConnection, org_id: Uuid) -> Result<HashMap<Uuid, String>, DbErr> {
    rows(db, "SELECT id, slug FROM apps WHERE org_id = $1", org_id)
        .await?
        .into_iter()
        .map(|row| Ok((row.try_get("", "id")?, row.try_get("", "slug")?)))
        .collect()
}

async fn rows(db: &DatabaseConnection, sql: &str, org_id: Uuid) -> Result<Vec<QueryResult>, DbErr> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [org_id.into()],
    ))
    .await
}
