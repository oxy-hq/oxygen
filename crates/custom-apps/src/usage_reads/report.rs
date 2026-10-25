//! The weekly report's reads: page views, error sessions and per-person views
//! over two weeks, and which apps were opened before.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use oxy_cameras::airhouse::escape::sql_str;
use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement, Value};
use uuid::Uuid;

use super::{PRODUCTION, Split, at, facts, or_fall_back, query, split};
use crate::usage_airhouse::{SinkError, TABLE, micros, ts_literal};

// ── The weekly report ───────────────────────────────────────────────────────

/// An app the report asks about, with the workspace whose store holds its usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportApp {
    pub app_id: Uuid,
    pub workspace_id: Uuid,
}

/// The two weeks the report compares: `[previous_start, start)` and
/// `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub previous_start: DateTime<Utc>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl Window {
    fn micros(self) -> Self {
        Self {
            previous_start: micros(self.previous_start),
            start: micros(self.start),
            end: micros(self.end),
        }
    }
}

#[derive(Clone, Debug, Default, FromQueryResult)]
pub struct ViewRow {
    pub app_id: Uuid,
    pub views: i64,
    pub people: i64,
    pub sessions: i64,
    pub prev_views: i64,
    pub prev_people: i64,
    pub prev_sessions: i64,
}

#[derive(Clone, Debug, Default, FromQueryResult)]
pub struct ErrorRow {
    pub app_id: Uuid,
    pub errored: i64,
    pub prev_errored: i64,
}

/// One person's page views of one app in each of the two weeks. The report
/// counts people per org across apps, which no per-app row can: a person who
/// opened two apps is one person.
#[derive(Clone, Debug, Default, FromQueryResult)]
pub struct PersonRow {
    pub app_id: Uuid,
    pub user_id: Uuid,
    pub views: i64,
    pub prev_views: i64,
}

/// What the report reads from the usage tables for a set of apps.
#[derive(Debug, Default)]
pub struct ReportUsage {
    pub views: Vec<ViewRow>,
    pub errors: Vec<ErrorRow>,
    pub people: Vec<PersonRow>,
}

/// The report's page views, error sessions and per-person views for `apps` over
/// `window`. Each app is answered by its workspace's store; an Airhouse that
/// cannot answer hands its apps to Postgres.
pub async fn report_usage(
    db: &DatabaseConnection,
    apps: &[ReportApp],
    window: Window,
) -> Result<ReportUsage, DbErr> {
    let window = window.micros();
    let Split {
        airhouse,
        mut postgres,
    } = split(db, apps).await?;
    let mut out = ReportUsage::default();
    let answers = futures::future::join_all(airhouse.iter().map(|(workspace_id, app_ids)| async {
        (
            *workspace_id,
            airhouse_report(db, *workspace_id, app_ids, window).await,
        )
    }))
    .await;
    for (workspace_id, result) in answers {
        match or_fall_back(workspace_id, "the usage report", result) {
            Some(found) => {
                out.views.extend(found.views);
                out.errors.extend(found.errors);
                out.people.extend(found.people);
            }
            None => postgres.extend(airhouse[&workspace_id].iter().copied()),
        }
    }
    if !postgres.is_empty() {
        let found = postgres_report(db, postgres, window).await?;
        out.views.extend(found.views);
        out.errors.extend(found.errors);
        out.people.extend(found.people);
    }
    Ok(out)
}

/// Which of `candidates` had a production page view before `before`. Used for
/// an app that first appears this week: was it ever opened earlier?
pub async fn apps_seen_before(
    db: &DatabaseConnection,
    candidates: &[ReportApp],
    before: DateTime<Utc>,
) -> Result<HashSet<Uuid>, DbErr> {
    let before = micros(before);
    let Split {
        airhouse,
        mut postgres,
    } = split(db, candidates).await?;
    let mut seen = HashSet::new();
    let answers = futures::future::join_all(airhouse.iter().map(|(workspace_id, app_ids)| async {
        (
            *workspace_id,
            airhouse_seen_before(db, *workspace_id, app_ids, before).await,
        )
    }))
    .await;
    for (workspace_id, result) in answers {
        match or_fall_back(workspace_id, "which apps were opened before", result) {
            Some(found) => seen.extend(found),
            None => postgres.extend(airhouse[&workspace_id].iter().copied()),
        }
    }
    if !postgres.is_empty() {
        #[derive(FromQueryResult)]
        struct Row {
            app_id: Uuid,
        }
        let found = Row::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT t.app_id FROM unnest($1::uuid[]) AS t(app_id) \
             WHERE EXISTS (SELECT 1 FROM custom_app_view_event v \
                           WHERE v.app_id = t.app_id AND v.environment = 'production' \
                             AND v.viewed_at < $2)",
            [postgres.into(), at(before).into()],
        ))
        .all(db)
        .await?;
        seen.extend(found.into_iter().map(|r| r.app_id));
    }
    Ok(seen)
}

async fn airhouse_seen_before(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    apps: &[Uuid],
    before: DateTime<Utc>,
) -> Result<Vec<Uuid>, SinkError> {
    // Existence needs no per-id dedupe: a duplicate row is still a row.
    let list = apps
        .iter()
        .map(|a| sql_str(&a.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT DISTINCT app_id FROM {TABLE} \
         WHERE kind = 'view' AND environment = '{PRODUCTION}' AND app_id IN ({list}) \
           AND occurred_at < {before}",
        before = ts_literal(before),
    );
    let rows = query(db, workspace_id, &sql).await?;
    rows.iter().map(|row| row.uuid("app_id")).collect()
}

async fn airhouse_report(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    apps: &[Uuid],
    w: Window,
) -> Result<ReportUsage, SinkError> {
    let start = ts_literal(w.start);
    let facts = facts(apps, Some(w.previous_start), Some(w.end));
    Ok(ReportUsage {
        views: airhouse_views(db, workspace_id, &facts, &start).await?,
        errors: airhouse_errors(db, workspace_id, &facts, &start).await?,
        people: airhouse_people(db, workspace_id, &facts, &start).await?,
    })
}

async fn airhouse_views(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    facts: &str,
    start: &str,
) -> Result<Vec<ViewRow>, SinkError> {
    let sql = format!(
        "WITH u AS ({facts}) SELECT app_id, \
           count(*) FILTER (WHERE occurred_at >= {start})::BIGINT AS views, \
           count(DISTINCT user_id) FILTER (WHERE occurred_at >= {start})::BIGINT AS people, \
           count(DISTINCT session_id) FILTER (WHERE occurred_at >= {start})::BIGINT AS sessions, \
           count(*) FILTER (WHERE occurred_at < {start})::BIGINT AS prev_views, \
           count(DISTINCT user_id) FILTER (WHERE occurred_at < {start})::BIGINT AS prev_people, \
           count(DISTINCT session_id) FILTER (WHERE occurred_at < {start})::BIGINT \
             AS prev_sessions \
         FROM u WHERE kind = 'view' GROUP BY app_id"
    );
    query(db, workspace_id, &sql)
        .await?
        .iter()
        .map(|r| {
            Ok(ViewRow {
                app_id: r.uuid("app_id")?,
                views: r.int("views")?,
                people: r.int("people")?,
                sessions: r.int("sessions")?,
                prev_views: r.int("prev_views")?,
                prev_people: r.int("prev_people")?,
                prev_sessions: r.int("prev_sessions")?,
            })
        })
        .collect()
}

async fn airhouse_errors(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    facts: &str,
    start: &str,
) -> Result<Vec<ErrorRow>, SinkError> {
    let sql = format!(
        "WITH u AS ({facts}) SELECT app_id, \
           count(DISTINCT session_id) FILTER (WHERE occurred_at >= {start})::BIGINT AS errored, \
           count(DISTINCT session_id) FILTER (WHERE occurred_at < {start})::BIGINT \
             AS prev_errored \
         FROM u WHERE kind = 'event' AND event_name = 'oxy-error' GROUP BY app_id"
    );
    query(db, workspace_id, &sql)
        .await?
        .iter()
        .map(|r| {
            Ok(ErrorRow {
                app_id: r.uuid("app_id")?,
                errored: r.int("errored")?,
                prev_errored: r.int("prev_errored")?,
            })
        })
        .collect()
}

async fn airhouse_people(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    facts: &str,
    start: &str,
) -> Result<Vec<PersonRow>, SinkError> {
    let sql = format!(
        "WITH u AS ({facts}) SELECT app_id, user_id, \
           count(*) FILTER (WHERE occurred_at >= {start})::BIGINT AS views, \
           count(*) FILTER (WHERE occurred_at < {start})::BIGINT AS prev_views \
         FROM u WHERE kind = 'view' GROUP BY app_id, user_id"
    );
    query(db, workspace_id, &sql)
        .await?
        .iter()
        .map(|r| {
            Ok(PersonRow {
                app_id: r.uuid("app_id")?,
                user_id: r.uuid("user_id")?,
                views: r.int("views")?,
                prev_views: r.int("prev_views")?,
            })
        })
        .collect()
}

// $1 start of the week before, $2 start of the week, $3 its end, $4 the apps.
const VIEWS_SQL: &str = "\
SELECT app_id, \
       COUNT(*) FILTER (WHERE viewed_at >= $2)::bigint AS views, \
       COUNT(DISTINCT user_id) FILTER (WHERE viewed_at >= $2)::bigint AS people, \
       COUNT(DISTINCT session_id) FILTER (WHERE viewed_at >= $2)::bigint AS sessions, \
       COUNT(*) FILTER (WHERE viewed_at < $2)::bigint AS prev_views, \
       COUNT(DISTINCT user_id) FILTER (WHERE viewed_at < $2)::bigint AS prev_people, \
       COUNT(DISTINCT session_id) FILTER (WHERE viewed_at < $2)::bigint AS prev_sessions \
FROM custom_app_view_event \
WHERE app_id = ANY($4) AND environment = 'production' \
  AND viewed_at >= $1 AND viewed_at < $3 \
GROUP BY app_id";

const PEOPLE_SQL: &str = "\
SELECT app_id, user_id, \
       COUNT(*) FILTER (WHERE viewed_at >= $2)::bigint AS views, \
       COUNT(*) FILTER (WHERE viewed_at < $2)::bigint AS prev_views \
FROM custom_app_view_event \
WHERE app_id = ANY($4) AND environment = 'production' \
  AND viewed_at >= $1 AND viewed_at < $3 \
GROUP BY app_id, user_id";

const ERRORS_SQL: &str = "\
SELECT app_id, \
       COUNT(DISTINCT session_id) FILTER (WHERE occurred_at >= $2)::bigint AS errored, \
       COUNT(DISTINCT session_id) FILTER (WHERE occurred_at < $2)::bigint AS prev_errored \
FROM custom_app_event \
WHERE app_id = ANY($4) AND environment = 'production' AND event_name = 'oxy-error' \
  AND occurred_at >= $1 AND occurred_at < $3 \
GROUP BY app_id";

async fn postgres_report(
    db: &DatabaseConnection,
    apps: Vec<Uuid>,
    w: Window,
) -> Result<ReportUsage, DbErr> {
    let window = || -> Vec<Value> {
        vec![
            at(w.previous_start).into(),
            at(w.start).into(),
            at(w.end).into(),
            apps.clone().into(),
        ]
    };
    async fn rows<T: FromQueryResult>(
        db: &DatabaseConnection,
        sql: &str,
        values: Vec<Value>,
    ) -> Result<Vec<T>, DbErr> {
        T::find_by_statement(Statement::from_sql_and_values(
            db.get_database_backend(),
            sql,
            values,
        ))
        .all(db)
        .await
    }
    Ok(ReportUsage {
        views: rows(db, VIEWS_SQL, window()).await?,
        errors: rows(db, ERRORS_SQL, window()).await?,
        people: rows(db, PEOPLE_SQL, window()).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_window_edge_is_cut_to_microseconds() {
        let edge = Utc.timestamp_opt(1_790_000_000, 999_999_999).unwrap();
        let w = Window {
            previous_start: edge,
            start: edge,
            end: edge,
        }
        .micros();
        assert_eq!(w.start.timestamp_subsec_nanos(), 999_999_000);
    }
}
