//! Reads one week of custom-app usage, and the week before it, out of Oxy's
//! Postgres — and, for an app whose workspace has its usage in Airhouse, out of
//! that.
//!
//! Five sources, all of which the platform already writes: the view row per
//! page served and the page's own error beacon (name `oxy-error` — counted, its
//! payload never read), which `oxy_custom_apps::usage_reads` reads from
//! `custom_app_view_event` / `custom_app_event` or from the workspace's
//! `oxy_custom_app_usage` — the same choice the Activity tab makes — the row per
//! function invocation (`app_function_invocations`), the release log
//! (`app_environment_events`, promotions to production) and the storage
//! sweeper's size samples (`app_storage_usage_samples`). Only the `production`
//! environment counts: a developer opening a sandbox is not usage.
//!
//! Every Postgres query names the apps it wants (`app_id = ANY`), which is what
//! lets it use each table's `(app_id, …, time)` index. The invocation table is
//! never pruned, so a scan by time alone would read all of it.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use sea_orm::{DatabaseConnection, DbErr, FromQueryResult, Statement, Value};
use uuid::Uuid;

use oxy_custom_apps::usage_reads::{self, ReportApp};
pub(super) use oxy_custom_apps::usage_reads::{ErrorRow, PersonRow, ViewRow};

use super::model::{AppUsage, Counts, OrgUsage, Snapshot};
use super::period::Period;

const CATALOG_SQL: &str = "\
SELECT a.id AS app_id, a.org_id, a.project_id AS workspace_id, a.name, a.slug, a.published_at, \
       o.name AS org_name, o.slug AS org_slug \
FROM apps a JOIN organizations o ON o.id = a.org_id";

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

#[derive(Clone, Debug, FromQueryResult)]
pub(super) struct CatalogRow {
    pub app_id: Uuid,
    pub org_id: Uuid,
    /// The app's workspace: whose store holds its page views and error beacons.
    pub workspace_id: Uuid,
    pub name: String,
    pub slug: String,
    pub published_at: Option<DateTime<Utc>>,
    pub org_name: String,
    pub org_slug: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct OrgPeopleRow {
    pub org_id: Uuid,
    pub people: i64,
    pub prev_people: i64,
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
    // Page views and error beacons: each app's workspace says which store holds
    // them (`usage_reads`), the same way it does for the Activity tab.
    let apps: Vec<ReportApp> = catalog
        .iter()
        .map(|a| ReportApp {
            app_id: a.app_id,
            workspace_id: a.workspace_id,
        })
        .collect();
    let usage = usage_reads::report_usage(
        db,
        &apps,
        usage_reads::Window {
            previous_start: period.previous().start,
            start: period.start,
            end: period.end,
        },
    )
    .await?;
    let returning: HashSet<Uuid> = usage
        .views
        .iter()
        .filter(|v| v.people > 0 && v.prev_people == 0)
        .map(|v| v.app_id)
        .collect();
    // These apps had no view in the week before, so "before that week" finds
    // the same rows as "before this one" over a shorter index range.
    let returning: Vec<ReportApp> = apps
        .iter()
        .filter(|a| returning.contains(&a.app_id))
        .copied()
        .collect();
    let seen_before =
        usage_reads::apps_seen_before(db, &returning, period.previous().start).await?;
    let raw = Raw {
        org_people: org_people(&catalog, &usage.people),
        errors: usage.errors,
        functions: rows(db, FUNCTIONS_SQL, window()).await?,
        releases: rows(db, RELEASES_SQL, window()).await?,
        storage: rows(db, STORAGE_SQL, size_before(period.end)).await?,
        storage_before: rows(db, STORAGE_SQL, size_before(period.start)).await?,
        seen_before,
        views: usage.views,
    };
    Ok(assemble(period, catalog, raw))
}

/// People per org, this week and the week before: a person who opened two of an
/// org's apps is one person, whichever stores the two apps' views are in.
fn org_people(catalog: &[CatalogRow], people: &[PersonRow]) -> Vec<OrgPeopleRow> {
    let org_of: HashMap<Uuid, Uuid> = catalog.iter().map(|a| (a.app_id, a.org_id)).collect();
    let mut current: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    let mut previous: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    for person in people {
        let Some(org) = org_of.get(&person.app_id) else {
            continue;
        };
        if person.views > 0 {
            current.entry(*org).or_default().insert(person.user_id);
        }
        if person.prev_views > 0 {
            previous.entry(*org).or_default().insert(person.user_id);
        }
    }
    let orgs: HashSet<Uuid> = current.keys().chain(previous.keys()).copied().collect();
    orgs.into_iter()
        .map(|org_id| OrgPeopleRow {
            org_id,
            people: current.get(&org_id).map_or(0, |s| s.len() as i64),
            prev_people: previous.get(&org_id).map_or(0, |s| s.len() as i64),
        })
        .collect()
}

async fn rows<T: FromQueryResult>(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<Value>,
) -> Result<Vec<T>, DbErr> {
    let statement = Statement::from_sql_and_values(db.get_database_backend(), sql, values);
    T::find_by_statement(statement).all(db).await
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
