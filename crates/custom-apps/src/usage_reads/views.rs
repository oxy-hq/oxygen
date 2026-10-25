//! Page-view reads for lists of apps: each app's last view, and who viewed
//! each app in each of a set of time windows. The admin apps list and the
//! platform facts read these instead of querying a store themselves.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use oxy_cameras::airhouse::escape::sql_str;
use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement, Value};
use uuid::Uuid;

use super::{PRODUCTION, ReportApp, facts, per_store, query};
use crate::usage_airhouse::{SinkError, TABLE, micros, ts_literal};

/// The customers of [`last_views`]: views by `staff` do not count, and only
/// views since `since` are looked at.
#[derive(Debug, Clone, Copy)]
pub struct Customers<'a> {
    pub staff: &'a [Uuid],
    pub since: DateTime<Utc>,
}

/// One app's newest production view, by anyone, and by a customer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastViews {
    pub app_id: Uuid,
    pub anyone: DateTime<Utc>,
    /// `None` when [`last_views`] was asked about no customers, or no customer
    /// viewed the app since [`Customers::since`].
    pub customer: Option<DateTime<Utc>>,
}

/// Each app's newest production view. An app nobody has viewed has no entry.
/// With `customers`, also each app's newest view by a customer.
pub async fn last_views(
    db: &DatabaseConnection,
    apps: &[ReportApp],
    customers: Option<Customers<'_>>,
) -> Result<HashMap<Uuid, LastViews>, DbErr> {
    let customers = customers.map(|c| Customers {
        staff: c.staff,
        since: micros(c.since),
    });
    let rows = per_store(
        db,
        apps,
        "the apps' last views",
        |workspace_id, app_ids| airhouse_last_views(db, workspace_id, app_ids, customers),
        |app_ids| postgres_last_views(db, app_ids, customers),
    )
    .await?;
    Ok(rows.into_iter().map(|r| (r.app_id, r)).collect())
}

async fn airhouse_last_views(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    apps: Vec<Uuid>,
    customers: Option<Customers<'_>>,
) -> Result<Vec<LastViews>, SinkError> {
    // A maximum needs no per-id dedupe: a duplicate row cannot change it.
    let customer = match customers {
        Some(c) => format!(
            "epoch_us(max(occurred_at) FILTER (WHERE occurred_at >= {since}{not_staff}))",
            since = ts_literal(c.since),
            not_staff = if c.staff.is_empty() {
                String::new()
            } else {
                format!(" AND user_id NOT IN ({})", list(c.staff))
            },
        ),
        None => "NULL".to_string(),
    };
    let sql = format!(
        "SELECT app_id, epoch_us(max(occurred_at)) AS anyone_us, {customer} AS customer_us \
         FROM {TABLE} \
         WHERE kind = 'view' AND environment = '{PRODUCTION}' AND app_id IN ({apps}) \
         GROUP BY app_id",
        apps = list(&apps),
    );
    query(db, workspace_id, &sql)
        .await?
        .iter()
        .map(|r| {
            Ok(LastViews {
                app_id: r.uuid("app_id")?,
                anyone: r.ts("anyone_us")?,
                customer: r.opt_ts("customer_us")?,
            })
        })
        .collect()
}

// $1 the apps. One probe of `(app_id, viewed_at DESC)` per app, never a `MAX`
// over every view an app ever had.
const LAST_VIEWED_SQL: &str = "\
SELECT a.app_id, last.viewed_at AS anyone, NULL::timestamptz AS customer \
FROM unnest($1::uuid[]) AS a(app_id) \
CROSS JOIN LATERAL (SELECT v.viewed_at FROM custom_app_view_event v \
                    WHERE v.app_id = a.app_id AND v.environment = 'production' \
                    ORDER BY v.viewed_at DESC LIMIT 1) last";

// $1 the apps, $2 the staff, $3 the customers' horizon. The same probe, and a
// second that skips staff's rows. It is floored at $3: an app only staff open
// would otherwise walk every view staff ever made of it.
const LAST_AND_CUSTOMER_VIEWED_SQL: &str = "\
SELECT a.app_id, last.viewed_at AS anyone, cust.viewed_at AS customer \
FROM unnest($1::uuid[]) AS a(app_id) \
CROSS JOIN LATERAL (SELECT v.viewed_at FROM custom_app_view_event v \
                    WHERE v.app_id = a.app_id AND v.environment = 'production' \
                    ORDER BY v.viewed_at DESC LIMIT 1) last \
LEFT JOIN LATERAL (SELECT v.viewed_at FROM custom_app_view_event v \
                   WHERE v.app_id = a.app_id AND v.environment = 'production' \
                     AND v.viewed_at >= $3 AND v.user_id <> ALL($2::uuid[]) \
                   ORDER BY v.viewed_at DESC LIMIT 1) cust ON true";

async fn postgres_last_views(
    db: &DatabaseConnection,
    apps: Vec<Uuid>,
    customers: Option<Customers<'_>>,
) -> Result<Vec<LastViews>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        app_id: Uuid,
        anyone: DateTime<Utc>,
        customer: Option<DateTime<Utc>>,
    }
    let (sql, values): (&str, Vec<Value>) = match customers {
        Some(c) => (
            LAST_AND_CUSTOMER_VIEWED_SQL,
            vec![apps.into(), c.staff.to_vec().into(), c.since.into()],
        ),
        None => (LAST_VIEWED_SQL, vec![apps.into()]),
    };
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| LastViews {
            app_id: r.app_id,
            anyone: r.anyone,
            customer: r.customer,
        })
        .collect())
}

/// A time window, `[start, end)`.
pub type TimeWindow = (DateTime<Utc>, DateTime<Utc>);

/// One person's production page views of one app inside one window.
/// `window` is the window's position in the slice [`viewers_by_window`] was
/// given; windows may overlap, and a view counts in every window it falls in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WindowViewer {
    pub app_id: Uuid,
    pub window: usize,
    pub user_id: Uuid,
    pub views: i64,
}

/// Every person who viewed one of `apps` in production inside each of
/// `windows`, with their view count. A caller counting people across apps or
/// orgs needs the people themselves, which no per-app count can give: a person
/// who opened two apps is one person.
pub async fn viewers_by_window(
    db: &DatabaseConnection,
    apps: &[ReportApp],
    windows: &[TimeWindow],
) -> Result<Vec<WindowViewer>, DbErr> {
    let windows: Vec<TimeWindow> = windows
        .iter()
        .map(|(s, e)| (micros(*s), micros(*e)))
        .collect();
    if windows.is_empty() {
        return Ok(Vec::new());
    }
    let windows = &windows;
    per_store(
        db,
        apps,
        "who viewed the apps",
        |workspace_id, app_ids| airhouse_viewers(db, workspace_id, app_ids, windows),
        |app_ids| postgres_viewers(db, app_ids, windows),
    )
    .await
}

fn bounds(windows: &[TimeWindow]) -> TimeWindow {
    let lo = windows.iter().map(|w| w.0).min().unwrap_or_default();
    let hi = windows.iter().map(|w| w.1).max().unwrap_or_default();
    (lo, hi)
}

async fn airhouse_viewers(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    apps: Vec<Uuid>,
    windows: &[TimeWindow],
) -> Result<Vec<WindowViewer>, SinkError> {
    let (lo, hi) = bounds(windows);
    let rows = windows
        .iter()
        .enumerate()
        .map(|(i, (s, e))| format!("({i}, {}, {})", ts_literal(*s), ts_literal(*e)))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH u AS ({facts}), \
         w AS (SELECT * FROM (VALUES {rows}) AS w(idx, starts_at, ends_at)) \
         SELECT u.app_id, w.idx::BIGINT AS idx, u.user_id, count(*)::BIGINT AS views \
         FROM u JOIN w ON u.occurred_at >= w.starts_at AND u.occurred_at < w.ends_at \
         WHERE u.kind = 'view' \
         GROUP BY u.app_id, w.idx, u.user_id",
        facts = facts(&apps, Some(lo), Some(hi)),
    );
    query(db, workspace_id, &sql)
        .await?
        .iter()
        .map(|r| {
            Ok(WindowViewer {
                app_id: r.uuid("app_id")?,
                window: window_index(r.int("idx")?)?,
                user_id: r.uuid("user_id")?,
                views: r.int("views")?,
            })
        })
        .collect()
}

fn window_index(idx: i64) -> Result<usize, SinkError> {
    usize::try_from(idx).map_err(|_| SinkError::Failed(format!("window index {idx}")))
}

// $1 window starts, $2 window ends, $3 the apps, $4 and $5 the outer bounds,
// which let the scan use `(app_id, viewed_at)`.
const VIEWERS_SQL: &str = "\
SELECT v.app_id, (w.idx - 1)::bigint AS idx, v.user_id, COUNT(*)::bigint AS views \
FROM custom_app_view_event v \
JOIN unnest($1::timestamptz[], $2::timestamptz[]) WITH ORDINALITY AS w(starts_at, ends_at, idx) \
  ON v.viewed_at >= w.starts_at AND v.viewed_at < w.ends_at \
WHERE v.app_id = ANY($3) AND v.environment = 'production' \
  AND v.viewed_at >= $4 AND v.viewed_at < $5 \
GROUP BY v.app_id, w.idx, v.user_id";

async fn postgres_viewers(
    db: &DatabaseConnection,
    apps: Vec<Uuid>,
    windows: &[TimeWindow],
) -> Result<Vec<WindowViewer>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        app_id: Uuid,
        idx: i64,
        user_id: Uuid,
        views: i64,
    }
    let (lo, hi) = bounds(windows);
    let starts: Vec<DateTime<Utc>> = windows.iter().map(|w| w.0).collect();
    let ends: Vec<DateTime<Utc>> = windows.iter().map(|w| w.1).collect();
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        VIEWERS_SQL,
        vec![
            starts.into(),
            ends.into(),
            apps.into(),
            lo.into(),
            hi.into(),
        ],
    ))
    .all(db)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(WindowViewer {
                app_id: r.app_id,
                window: usize::try_from(r.idx)
                    .map_err(|_| DbErr::Custom(format!("window index {}", r.idx)))?,
                user_id: r.user_id,
                views: r.views,
            })
        })
        .collect()
}

fn list(ids: &[Uuid]) -> String {
    ids.iter()
        .map(|id| sql_str(&id.to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}
