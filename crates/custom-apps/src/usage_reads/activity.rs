//! The Activity tab's reads: summary, visitors, event counts and the drill-down.

use airhouse::tenant::escape::sql_str;
use chrono::{DateTime, FixedOffset, Utc};
use entity::{custom_app_event, custom_app_view_event};
use sea_orm::{
    ColumnTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait, FromQueryResult,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Statement,
};
use uuid::Uuid;

use super::{PRODUCTION, airhouse_workspace_of, at, facts, or_fall_back, query};
use crate::usage_airhouse::{SinkError, TABLE, micros};

// ── The Activity tab ────────────────────────────────────────────────────────

/// The headline card: production views, people and events since a cutoff, and
/// the last view ever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub views: i64,
    pub unique_users: i64,
    pub events: i64,
    pub last_viewed_at: Option<DateTime<FixedOffset>>,
}

/// [`Summary`] of `app_id` since `cutoff`.
pub async fn summary(
    db: &DatabaseConnection,
    app_id: Uuid,
    cutoff: DateTime<Utc>,
) -> Result<Summary, DbErr> {
    let cutoff = micros(cutoff);
    if let Some(workspace_id) = airhouse_workspace_of(db, app_id).await? {
        let result = airhouse_summary(db, workspace_id, app_id, cutoff).await;
        if let Some(found) = or_fall_back(workspace_id, "the activity summary", result) {
            return Ok(found);
        }
    }
    postgres_summary(db, app_id, cutoff).await
}

async fn airhouse_summary(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    app_id: Uuid,
    cutoff: DateTime<Utc>,
) -> Result<Summary, SinkError> {
    // The counts only need the window, so only the window is deduplicated; the
    // last view is a maximum, which a duplicate row cannot change, over the
    // app's views without the window function.
    let counts = format!(
        "WITH u AS ({facts}) SELECT \
           count(*) FILTER (WHERE kind = 'view')::BIGINT AS views, \
           count(DISTINCT user_id) FILTER (WHERE kind = 'view')::BIGINT AS unique_users, \
           count(*) FILTER (WHERE kind = 'event')::BIGINT AS events \
         FROM u",
        facts = facts(&[app_id], Some(cutoff), None),
    );
    let last = format!(
        "SELECT epoch_us(max(occurred_at)) AS last_viewed_us FROM {TABLE} \
         WHERE app_id = {app} AND environment = '{PRODUCTION}' AND kind = 'view'",
        app = sql_str(&app_id.to_string()),
    );
    let counts = query(db, workspace_id, &counts).await?;
    let last = query(db, workspace_id, &last).await?;
    let counts = counts
        .iter()
        .next()
        .ok_or_else(|| SinkError::Failed("the summary returned no row".into()))?;
    let last = last
        .iter()
        .next()
        .ok_or_else(|| SinkError::Failed("the last view returned no row".into()))?;
    Ok(Summary {
        views: counts.int("views")?,
        unique_users: counts.int("unique_users")?,
        events: counts.int("events")?,
        last_viewed_at: last.opt_ts("last_viewed_us")?.map(at),
    })
}

async fn postgres_summary(
    db: &DatabaseConnection,
    app_id: Uuid,
    cutoff: DateTime<Utc>,
) -> Result<Summary, DbErr> {
    let cutoff = at(cutoff);
    let views = custom_app_view_event::Entity::find()
        .filter(custom_app_view_event::Column::AppId.eq(app_id))
        .filter(custom_app_view_event::Column::Environment.eq(PRODUCTION))
        .filter(custom_app_view_event::Column::ViewedAt.gte(cutoff))
        .count(db)
        .await? as i64;
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    let unique_users = Count::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT COUNT(DISTINCT user_id)::bigint AS n \
         FROM custom_app_view_event \
         WHERE app_id = $1 AND viewed_at >= $2 AND environment = 'production'",
        [app_id.into(), cutoff.into()],
    ))
    .one(db)
    .await?
    .map_or(0, |r| r.n);
    let events = custom_app_event::Entity::find()
        .filter(custom_app_event::Column::AppId.eq(app_id))
        .filter(custom_app_event::Column::Environment.eq(PRODUCTION))
        .filter(custom_app_event::Column::OccurredAt.gte(cutoff))
        .count(db)
        .await? as i64;
    let last_viewed_at = custom_app_view_event::Entity::find()
        .filter(custom_app_view_event::Column::AppId.eq(app_id))
        .filter(custom_app_view_event::Column::Environment.eq(PRODUCTION))
        .order_by_desc(custom_app_view_event::Column::ViewedAt)
        .one(db)
        .await?
        .map(|r| r.viewed_at);
    Ok(Summary {
        views,
        unique_users,
        events,
        last_viewed_at,
    })
}

/// One visitor of the Activity tab's table.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct Visitor {
    pub user_id: Uuid,
    pub user_email: String,
    pub sessions: i64,
    pub views: i64,
    pub first_seen_at: DateTime<FixedOffset>,
    pub last_seen_at: DateTime<FixedOffset>,
    /// The role on their latest view that recorded one, not their role today.
    pub app_role: Option<String>,
    pub org_role: Option<String>,
}

/// Production visitors of `app_id` since `cutoff`, most recent first.
pub async fn visitors(
    db: &DatabaseConnection,
    app_id: Uuid,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Visitor>, DbErr> {
    let cutoff = micros(cutoff);
    if let Some(workspace_id) = airhouse_workspace_of(db, app_id).await? {
        let result = airhouse_visitors(db, workspace_id, app_id, cutoff, limit).await;
        if let Some(found) = or_fall_back(workspace_id, "the visitors", result) {
            return Ok(found);
        }
    }
    postgres_visitors(db, app_id, cutoff, limit).await
}

async fn airhouse_visitors(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    app_id: Uuid,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Visitor>, SinkError> {
    // Roles are per-view snapshots; a roll-up picks the latest *recorded* one,
    // so a view that recorded none never blanks out a known role.
    let sql = format!(
        "WITH u AS ({facts}) SELECT \
           user_id, max(user_email) AS user_email, \
           count(DISTINCT session_id)::BIGINT AS sessions, count(*)::BIGINT AS views, \
           epoch_us(min(occurred_at)) AS first_seen_us, epoch_us(max(occurred_at)) AS last_seen_us, \
           arg_max(app_role, occurred_at) FILTER (WHERE app_role IS NOT NULL) AS app_role, \
           arg_max(org_role, occurred_at) FILTER (WHERE org_role IS NOT NULL) AS org_role \
         FROM u WHERE kind = 'view' \
         GROUP BY user_id ORDER BY last_seen_us DESC, user_id LIMIT {limit}",
        facts = facts(&[app_id], Some(cutoff), None),
        limit = limit.max(0),
    );
    let rows = query(db, workspace_id, &sql).await?;
    rows.iter()
        .map(|row| {
            Ok(Visitor {
                user_id: row.uuid("user_id")?,
                user_email: row.text("user_email")?,
                sessions: row.int("sessions")?,
                views: row.int("views")?,
                first_seen_at: at(row.ts("first_seen_us")?),
                last_seen_at: at(row.ts("last_seen_us")?),
                app_role: row.opt_text("app_role")?,
                org_role: row.opt_text("org_role")?,
            })
        })
        .collect()
}

async fn postgres_visitors(
    db: &DatabaseConnection,
    app_id: Uuid,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Visitor>, DbErr> {
    // Deliberately NOT a join to `app_members` / `org_members`: that would show
    // today's role against last month's activity and silently rewrite the log.
    // A role is the latest **recorded** one (`FILTER (WHERE … IS NOT NULL)`),
    // because NULL means "not recorded", not "no role".
    Visitor::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"SELECT
             user_id,
             MAX(user_email) AS user_email,
             COUNT(DISTINCT session_id)::bigint AS sessions,
             COUNT(*)::bigint AS views,
             MIN(viewed_at) AS first_seen_at,
             MAX(viewed_at) AS last_seen_at,
             (array_agg(app_role ORDER BY viewed_at DESC)
                FILTER (WHERE app_role IS NOT NULL))[1] AS app_role,
             (array_agg(org_role ORDER BY viewed_at DESC)
                FILTER (WHERE org_role IS NOT NULL))[1] AS org_role
           FROM custom_app_view_event
           WHERE app_id = $1 AND viewed_at >= $2 AND environment = 'production'
           GROUP BY user_id
           ORDER BY last_seen_at DESC, user_id
           LIMIT $3"#,
        [app_id.into(), at(cutoff).into(), limit.into()],
    ))
    .all(db)
    .await
}

/// One event name of the Activity tab's table.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct EventGroup {
    pub event_name: String,
    pub count: i64,
    pub last_fired_at: DateTime<FixedOffset>,
}

/// Production events of `app_id` since `cutoff`, by name, most frequent first.
pub async fn event_groups(
    db: &DatabaseConnection,
    app_id: Uuid,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<EventGroup>, DbErr> {
    let cutoff = micros(cutoff);
    if let Some(workspace_id) = airhouse_workspace_of(db, app_id).await? {
        let result = airhouse_event_groups(db, workspace_id, app_id, cutoff, limit).await;
        if let Some(found) = or_fall_back(workspace_id, "the event counts", result) {
            return Ok(found);
        }
    }
    EventGroup::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"SELECT
             event_name,
             COUNT(*)::bigint AS count,
             MAX(occurred_at) AS last_fired_at
           FROM custom_app_event
           WHERE app_id = $1 AND occurred_at >= $2 AND environment = 'production'
           GROUP BY event_name
           ORDER BY count DESC, event_name
           LIMIT $3"#,
        [app_id.into(), at(cutoff).into(), limit.into()],
    ))
    .all(db)
    .await
}

async fn airhouse_event_groups(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    app_id: Uuid,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<EventGroup>, SinkError> {
    let sql = format!(
        "WITH u AS ({facts}) SELECT event_name, count(*)::BIGINT AS count, \
           epoch_us(max(occurred_at)) AS last_fired_us \
         FROM u WHERE kind = 'event' \
         GROUP BY event_name ORDER BY count DESC, event_name LIMIT {limit}",
        facts = facts(&[app_id], Some(cutoff), None),
        limit = limit.max(0),
    );
    let rows = query(db, workspace_id, &sql).await?;
    rows.iter()
        .map(|row| {
            Ok(EventGroup {
                event_name: row.text("event_name")?,
                count: row.int("count")?,
                last_fired_at: at(row.ts("last_fired_us")?),
            })
        })
        .collect()
}

/// One occurrence of an event, for the drill-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventOccurrence {
    pub id: Uuid,
    pub event_name: String,
    pub user_email: String,
    pub payload: serde_json::Value,
    pub occurred_at: DateTime<FixedOffset>,
}

/// The latest `limit` production occurrences of `event_name` since `cutoff`.
pub async fn event_occurrences(
    db: &DatabaseConnection,
    app_id: Uuid,
    event_name: &str,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<EventOccurrence>, DbErr> {
    let cutoff = micros(cutoff);
    if let Some(workspace_id) = airhouse_workspace_of(db, app_id).await? {
        let result =
            airhouse_event_occurrences(db, workspace_id, app_id, event_name, cutoff, limit).await;
        if let Some(found) = or_fall_back(workspace_id, "the event occurrences", result) {
            return Ok(found);
        }
    }
    Ok(custom_app_event::Entity::find()
        .filter(custom_app_event::Column::AppId.eq(app_id))
        .filter(custom_app_event::Column::Environment.eq(PRODUCTION))
        .filter(custom_app_event::Column::EventName.eq(event_name))
        .filter(custom_app_event::Column::OccurredAt.gte(at(cutoff)))
        .order_by_desc(custom_app_event::Column::OccurredAt)
        .order_by_asc(custom_app_event::Column::Id)
        .limit(limit.max(0) as u64)
        .all(db)
        .await?
        .into_iter()
        .map(|r| EventOccurrence {
            id: r.id,
            event_name: r.event_name,
            user_email: r.user_email,
            payload: r.payload,
            occurred_at: r.occurred_at,
        })
        .collect())
}

async fn airhouse_event_occurrences(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    app_id: Uuid,
    event_name: &str,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<EventOccurrence>, SinkError> {
    let sql = format!(
        "WITH u AS ({facts}) SELECT event_id, event_name, user_email, payload, \
           epoch_us(occurred_at) AS occurred_us \
         FROM u WHERE kind = 'event' AND event_name = {name} \
         ORDER BY occurred_at DESC, event_id LIMIT {limit}",
        facts = facts(&[app_id], Some(cutoff), None),
        name = sql_str(event_name),
        limit = limit.max(0),
    );
    let rows = query(db, workspace_id, &sql).await?;
    rows.iter()
        .map(|row| {
            let payload = row.text("payload")?;
            Ok(EventOccurrence {
                id: row.uuid("event_id")?,
                event_name: row.text("event_name")?,
                user_email: row.text("user_email")?,
                payload: serde_json::from_str(&payload)
                    .map_err(|e| SinkError::Failed(format!("an event payload is not JSON: {e}")))?,
                occurred_at: at(row.ts("occurred_us")?),
            })
        })
        .collect()
}
