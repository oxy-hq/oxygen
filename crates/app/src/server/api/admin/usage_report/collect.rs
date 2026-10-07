//! Reads one week of custom-app usage, and the week before it, out of Oxy's
//! Postgres.
//!
//! Five tables, all of which the platform already writes: the view row per
//! page served (`custom_app_view_event`), the page's own error beacon
//! (`custom_app_event`, name `oxy-error` — counted, its payload never read),
//! the row per function invocation (`app_function_invocations`), the release
//! log (`app_environment_events`, promotions to production) and the storage
//! sweeper's size samples (`app_storage_usage_samples`). Only the `production`
//! environment counts: a developer opening a sandbox is not usage.
//!
//! Every query names the apps it wants (`app_id = ANY`), which is what lets it
//! use each table's `(app_id, …, time)` index. The invocation table is never
//! pruned, so a scan by time alone would read all of it.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use sea_orm::{DatabaseConnection, DbErr, FromQueryResult, Statement, Value};
use uuid::Uuid;

use super::model::{AppUsage, Counts, OrgUsage, Snapshot};
use super::period::Period;

const CATALOG_SQL: &str = "\
SELECT a.id AS app_id, a.org_id, a.name, a.slug, a.published_at, \
       o.name AS org_name, o.slug AS org_slug \
FROM apps a JOIN organizations o ON o.id = a.org_id";

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

const ORG_PEOPLE_SQL: &str = "\
SELECT a.org_id, \
       COUNT(DISTINCT v.user_id) FILTER (WHERE v.viewed_at >= $2)::bigint AS people, \
       COUNT(DISTINCT v.user_id) FILTER (WHERE v.viewed_at < $2)::bigint AS prev_people \
FROM custom_app_view_event v JOIN apps a ON a.id = v.app_id \
WHERE v.app_id = ANY($4) AND v.environment = 'production' \
  AND v.viewed_at >= $1 AND v.viewed_at < $3 \
GROUP BY a.org_id";

const ERRORS_SQL: &str = "\
SELECT app_id, \
       COUNT(DISTINCT session_id) FILTER (WHERE occurred_at >= $2)::bigint AS errored, \
       COUNT(DISTINCT session_id) FILTER (WHERE occurred_at < $2)::bigint AS prev_errored \
FROM custom_app_event \
WHERE app_id = ANY($4) AND environment = 'production' AND event_name = 'oxy-error' \
  AND occurred_at >= $1 AND occurred_at < $3 \
GROUP BY app_id";

const FUNCTIONS_SQL: &str = "\
SELECT app_id, \
       COUNT(*) FILTER (WHERE created_at >= $2)::bigint AS calls, \
       COUNT(*) FILTER (WHERE created_at >= $2 AND status IN ('error', 'timeout'))::bigint AS failed, \
       COUNT(*) FILTER (WHERE created_at < $2)::bigint AS prev_calls, \
       COUNT(*) FILTER (WHERE created_at < $2 AND status IN ('error', 'timeout'))::bigint AS prev_failed \
FROM app_function_invocations \
WHERE app_id = ANY($4) AND environment = 'production' \
  AND created_at >= $1 AND created_at < $3 \
GROUP BY app_id";

const RELEASES_SQL: &str = "\
SELECT e.app_id, \
       COUNT(*) FILTER (WHERE e.at >= $2)::bigint AS releases, \
       COUNT(*) FILTER (WHERE e.at < $2)::bigint AS prev_releases \
FROM app_environment_events e \
WHERE e.app_id = ANY($4) AND e.environment = 'production' AND e.action = 'promote' \
  AND e.at >= $1 AND e.at < $3 \
GROUP BY e.app_id";

// $1 the apps, $2 an instant: each app's last measured size before it.
const STORAGE_SQL: &str = "\
SELECT DISTINCT ON (app_id) app_id, bytes \
FROM app_storage_usage_samples \
WHERE app_id = ANY($1) AND measured_at < $2 \
ORDER BY app_id, measured_at DESC";

// $1 the apps to ask about, $2 the instant to look before.
const SEEN_BEFORE_SQL: &str = "\
SELECT t.app_id FROM unnest($1::uuid[]) AS t(app_id) \
WHERE EXISTS (SELECT 1 FROM custom_app_view_event v \
              WHERE v.app_id = t.app_id AND v.environment = 'production' \
                AND v.viewed_at < $2)";

#[derive(Clone, Debug, FromQueryResult)]
pub(super) struct CatalogRow {
    pub app_id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub published_at: Option<DateTime<Utc>>,
    pub org_name: String,
    pub org_slug: String,
}

#[derive(Clone, Debug, Default, FromQueryResult)]
pub(super) struct ViewRow {
    pub app_id: Uuid,
    pub views: i64,
    pub people: i64,
    pub sessions: i64,
    pub prev_views: i64,
    pub prev_people: i64,
    pub prev_sessions: i64,
}

#[derive(Clone, Debug, Default, FromQueryResult)]
pub(super) struct OrgPeopleRow {
    pub org_id: Uuid,
    pub people: i64,
    pub prev_people: i64,
}

#[derive(Clone, Debug, Default, FromQueryResult)]
pub(super) struct ErrorRow {
    pub app_id: Uuid,
    pub errored: i64,
    pub prev_errored: i64,
}

#[derive(Clone, Debug, Default, FromQueryResult)]
pub(super) struct FunctionRow {
    pub app_id: Uuid,
    pub calls: i64,
    pub failed: i64,
    pub prev_calls: i64,
    pub prev_failed: i64,
}

#[derive(Clone, Debug, Default, FromQueryResult)]
pub(super) struct ReleaseRow {
    pub app_id: Uuid,
    pub releases: i64,
    pub prev_releases: i64,
}

#[derive(Clone, Debug, Default, FromQueryResult)]
pub(super) struct StorageRow {
    pub app_id: Uuid,
    pub bytes: i64,
}

#[derive(FromQueryResult)]
struct AppIdRow {
    app_id: Uuid,
}

/// Everything the queries returned, before it is shaped into a report.
#[derive(Default)]
pub(super) struct Raw {
    pub views: Vec<ViewRow>,
    pub org_people: Vec<OrgPeopleRow>,
    pub errors: Vec<ErrorRow>,
    pub functions: Vec<FunctionRow>,
    pub releases: Vec<ReleaseRow>,
    /// Each app's size at the end of the week, and at its start. An app with
    /// no row was never measured by then.
    pub storage: Vec<StorageRow>,
    pub storage_before: Vec<StorageRow>,
    /// Of the apps opened this week and not the week before: those somebody
    /// had opened earlier still.
    pub seen_before: HashSet<Uuid>,
}

/// The usage report for `period`, read fresh.
pub async fn collect(db: &DatabaseConnection, period: Period) -> Result<Snapshot, DbErr> {
    let catalog: Vec<CatalogRow> = rows(db, CATALOG_SQL, vec![]).await?;
    if catalog.is_empty() {
        return Ok(assemble(period, catalog, Raw::default()));
    }
    let app_ids: Vec<Uuid> = catalog.iter().map(|a| a.app_id).collect();
    let window = || -> Vec<Value> {
        vec![
            period.previous().start.into(),
            period.start.into(),
            period.end.into(),
            app_ids.clone().into(),
        ]
    };
    let size_before = |at: DateTime<Utc>| -> Vec<Value> { vec![app_ids.clone().into(), at.into()] };
    let views: Vec<ViewRow> = rows(db, VIEWS_SQL, window()).await?;
    let returning: Vec<Uuid> = views
        .iter()
        .filter(|v| v.people > 0 && v.prev_people == 0)
        .map(|v| v.app_id)
        .collect();
    // These apps had no view in the week before, so "before that week" finds
    // the same rows as "before this one" over a shorter index range.
    let seen_before = seen_before(db, returning, period.previous().start).await?;
    let raw = Raw {
        org_people: rows(db, ORG_PEOPLE_SQL, window()).await?,
        errors: rows(db, ERRORS_SQL, window()).await?,
        functions: rows(db, FUNCTIONS_SQL, window()).await?,
        releases: rows(db, RELEASES_SQL, window()).await?,
        storage: rows(db, STORAGE_SQL, size_before(period.end)).await?,
        storage_before: rows(db, STORAGE_SQL, size_before(period.start)).await?,
        seen_before,
        views,
    };
    Ok(assemble(period, catalog, raw))
}

async fn rows<T: FromQueryResult>(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<Value>,
) -> Result<Vec<T>, DbErr> {
    let statement = Statement::from_sql_and_values(db.get_database_backend(), sql, values);
    T::find_by_statement(statement).all(db).await
}

/// Which of `candidates` somebody opened before `before`.
async fn seen_before(
    db: &DatabaseConnection,
    candidates: Vec<Uuid>,
    before: DateTime<Utc>,
) -> Result<HashSet<Uuid>, DbErr> {
    if candidates.is_empty() {
        return Ok(HashSet::new());
    }
    let found: Vec<AppIdRow> =
        rows(db, SEEN_BEFORE_SQL, vec![candidates.into(), before.into()]).await?;
    Ok(found.into_iter().map(|r| r.app_id).collect())
}

fn n(count: i64) -> u64 {
    u64::try_from(count).unwrap_or(0)
}

/// [`Raw`], keyed for looking one app or one org up.
struct Lookup {
    views: HashMap<Uuid, ViewRow>,
    errors: HashMap<Uuid, ErrorRow>,
    functions: HashMap<Uuid, FunctionRow>,
    releases: HashMap<Uuid, ReleaseRow>,
    storage: HashMap<Uuid, u64>,
    storage_before: HashMap<Uuid, u64>,
    seen_before: HashSet<Uuid>,
    org_people: HashMap<Uuid, OrgPeopleRow>,
}

impl From<Raw> for Lookup {
    fn from(raw: Raw) -> Self {
        let sizes = |rows: Vec<StorageRow>| -> HashMap<Uuid, u64> {
            rows.into_iter().map(|r| (r.app_id, n(r.bytes))).collect()
        };
        Self {
            views: raw.views.into_iter().map(|r| (r.app_id, r)).collect(),
            errors: raw.errors.into_iter().map(|r| (r.app_id, r)).collect(),
            functions: raw.functions.into_iter().map(|r| (r.app_id, r)).collect(),
            releases: raw.releases.into_iter().map(|r| (r.app_id, r)).collect(),
            storage: sizes(raw.storage),
            storage_before: sizes(raw.storage_before),
            seen_before: raw.seen_before,
            org_people: raw.org_people.into_iter().map(|r| (r.org_id, r)).collect(),
        }
    }
}

impl Lookup {
    /// One app's week, and the week before it.
    fn counts(&self, app: Uuid) -> (Counts, Counts) {
        let v = self.views.get(&app).cloned().unwrap_or_default();
        let e = self.errors.get(&app).cloned().unwrap_or_default();
        let f = self.functions.get(&app).cloned().unwrap_or_default();
        let r = self.releases.get(&app).cloned().unwrap_or_default();
        let current = Counts {
            views: n(v.views),
            people: n(v.people),
            sessions: n(v.sessions),
            error_sessions: n(e.errored),
            function_calls: n(f.calls),
            function_failures: n(f.failed),
            releases: n(r.releases),
        };
        let previous = Counts {
            views: n(v.prev_views),
            people: n(v.prev_people),
            sessions: n(v.prev_sessions),
            error_sessions: n(e.prev_errored),
            function_calls: n(f.prev_calls),
            function_failures: n(f.prev_failed),
            releases: n(r.prev_releases),
        };
        (current, previous)
    }

    fn org(&self, row: &CatalogRow) -> OrgUsage {
        let seen = self.org_people.get(&row.org_id);
        OrgUsage {
            org_id: row.org_id,
            name: row.org_name.clone(),
            slug: row.org_slug.clone(),
            people: seen.map_or(0, |p| n(p.people)),
            prev_people: seen.map_or(0, |p| n(p.prev_people)),
            apps: Vec::new(),
        }
    }
}

/// Shape the rows into a report: every app that is live or saw any use in
/// either week, under its org; orgs and apps ordered busiest first.
pub(super) fn assemble(period: Period, catalog: Vec<CatalogRow>, raw: Raw) -> Snapshot {
    let lookup = Lookup::from(raw);
    let mut orgs: HashMap<Uuid, OrgUsage> = HashMap::new();
    for row in catalog {
        let (current, previous) = lookup.counts(row.app_id);
        if row.published_at.is_none() && !current.any_activity() && !previous.any_activity() {
            continue;
        }
        let first_week =
            current.people > 0 && previous.people == 0 && !lookup.seen_before.contains(&row.app_id);
        let org = orgs.entry(row.org_id).or_insert_with(|| lookup.org(&row));
        org.apps.push(AppUsage {
            app_id: row.app_id,
            name: row.name,
            slug: row.slug,
            published_at: row.published_at,
            first_week,
            storage_bytes: lookup.storage.get(&row.app_id).copied(),
            storage_bytes_before: lookup.storage_before.get(&row.app_id).copied(),
            current,
            previous,
        });
    }
    Snapshot {
        period,
        orgs: ordered(orgs.into_values().collect()),
    }
}

/// Busiest first, by people then views; names break ties so the order is stable.
fn ordered(mut orgs: Vec<OrgUsage>) -> Vec<OrgUsage> {
    for org in &mut orgs {
        org.apps.sort_by(|a, b| {
            (b.current.people, b.current.views)
                .cmp(&(a.current.people, a.current.views))
                .then_with(|| a.name.cmp(&b.name))
        });
    }
    orgs.sort_by(|a, b| {
        (b.people, b.views())
            .cmp(&(a.people, a.views()))
            .then_with(|| a.name.cmp(&b.name))
    });
    orgs
}

#[cfg(test)]
#[path = "collect_tests.rs"]
mod tests;
