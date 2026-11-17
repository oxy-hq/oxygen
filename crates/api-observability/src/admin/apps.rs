//! `GET /admin/observability/orgs/{org_id}/apps` — one org's custom-app
//! performance: traffic, the latency tail, the three failure classes apart,
//! the per-app breakdown and the slowest routes.
//!
//! The aggregates are `oxy_observability`'s
//! (`backends/clickhouse/org_apps.rs`); this is the handler that fences the
//! read to the org, resolves app ids to names, and shapes the answer.
//!
//! **`measured: false` is not an error and not a zero.** With no observability
//! store configured (`OXY_OBSERVABILITY_BACKEND` unset — which is every local
//! box that did not run `oxy start`), there is nothing to read. Answering 200
//! with zeros would draw an org as having no traffic, which is the defect the
//! store's own doc comments keep insisting on: "the difference between 'served
//! nothing' and 'we never asked'". So the response says which it is, once, and
//! the page names it instead of rendering a dash per row.

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Json, Router, routing::get};
use oxy_app_core::AppState;
use oxy_app_core::admin_db::db_err;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_observability::types::OrgAppTraffic;
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

use super::{WindowQuery, connect_scoped};

/// Totals and the latency tail. Latency is milliseconds as `f64` because these
/// are interpolated quantiles — a p95 over an even sample is not one of the
/// samples, and rounding here would claim a precision the number does not have.
#[derive(Debug, Default, Serialize)]
pub struct AppTotals {
    pub requests: u64,
    pub failed: u64,
    pub sessions: u64,
    pub people: u64,
    pub apps: u64,
    pub bytes: u64,
    pub host_calls: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    /// p95 of start-up time over requests that paid one. Warm requests record
    /// 0 and are excluded, so this is the cold-start tail.
    pub cold_start_p95_ms: f64,
    /// Exact summed wall time, and the start-up share of it. `wall_ms -
    /// init_ms` is the tenant's own code.
    pub wall_ms: u64,
    pub init_ms: u64,
    /// `None` on no traffic: no requests have no opinion about reliability.
    pub failure_rate: Option<f64>,
}

/// One failure class and cause.
#[derive(Debug, Serialize)]
pub struct AppFailure {
    /// The class the writer stamped — an explicit 5xx, an implicit failure (a
    /// 200 carrying a shell that never mounted), or a policy one (too slow).
    /// Passed through, never re-derived from `status`.
    pub outcome: String,
    pub error_kind: String,
    pub requests: u64,
    pub share: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct AppDay {
    pub day: String,
    pub requests: u64,
    pub failed: u64,
    pub p95_ms: f64,
    pub wall_ms: u64,
    pub init_ms: u64,
    pub failure_rate: Option<f64>,
}

/// One of the org's apps.
#[derive(Debug, Serialize)]
pub struct AppRow {
    pub app_id: String,
    /// The app's name and slug, where the catalog still has it. An app id with
    /// no row — a deleted app whose events are still inside retention — keeps
    /// its id as its name rather than disappearing from a total it contributed
    /// to.
    pub name: String,
    pub slug: String,
    pub requests: u64,
    pub failed: u64,
    pub sessions: u64,
    pub bytes: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub cold_start_p95_ms: f64,
    pub failure_rate: Option<f64>,
}

/// One route, ranked by its latency tail.
#[derive(Debug, Serialize)]
pub struct AppRoute {
    pub app_id: String,
    pub app_name: String,
    pub route: String,
    pub requests: u64,
    pub failed: u64,
    pub p95_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct OrgApps {
    pub window_days: u32,
    /// False when this deployment has no observability store, so every field
    /// below is empty for want of a source rather than for want of traffic.
    pub measured: bool,
    pub total: AppTotals,
    pub failures: Vec<AppFailure>,
    pub by_day: Vec<AppDay>,
    pub by_app: Vec<AppRow>,
    pub routes: Vec<AppRoute>,
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/observability/orgs/{org_id}/apps", get(org_apps))
}

/// App names for the ids the aggregate returned.
///
/// One statement over the ids actually seen, rather than "every app of this
/// org": an app deleted inside the 90-day retention still has events, and the
/// page must still be able to name what it is looking at.
#[derive(Debug, FromQueryResult)]
struct AppNameRow {
    id: Uuid,
    name: String,
    slug: String,
}

/// The name lookup's statement, with each id bound.
///
/// `None` for an empty id list, which is the short-circuit rather than a
/// comment about one: an empty `IN ()` is a syntax error, and an org with no
/// app traffic is an ordinary state.
fn app_names_sql(ids: &[Uuid], values: &mut Vec<sea_orm::Value>) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let placeholders = ids
        .iter()
        .map(|id| {
            values.push((*id).into());
            format!("${}", values.len())
        })
        .collect::<Vec<_>>()
        .join(",");
    Some(format!(
        "SELECT id, name, slug FROM apps WHERE id IN ({placeholders})"
    ))
}

async fn app_names(
    db: &sea_orm::DatabaseConnection,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, (String, String)>, sea_orm::DbErr> {
    let mut values: Vec<sea_orm::Value> = Vec::with_capacity(ids.len());
    let Some(sql) = app_names_sql(ids, &mut values) else {
        return Ok(HashMap::new());
    };
    Ok(
        AppNameRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .all(db)
        .await?
        .into_iter()
        .map(|r| (r.id, (r.name, r.slug)))
        .collect(),
    )
}

async fn org_apps(
    State(state): State<AppState>,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Query(q): Query<WindowQuery>,
) -> Result<Json<OrgApps>, Response> {
    let days = q.days();
    let db = connect_scoped(&actor, org_id).await?;

    // No store configured: say so rather than reporting a quiet org.
    let Some(store) = state.observability.as_ref() else {
        return Ok(Json(OrgApps {
            window_days: days as u32,
            measured: false,
            total: AppTotals::default(),
            failures: Vec::new(),
            by_day: Vec::new(),
            by_app: Vec::new(),
            routes: Vec::new(),
        }));
    };

    let traffic = store
        .get_org_app_traffic(&org_id.to_string(), days.max(0) as u32)
        .await
        .map_err(super::store_err)?;

    let names = app_names(&db, &collect_app_ids(&traffic))
        .await
        .map_err(db_err)?;

    Ok(Json(build_org_apps(days.max(0) as u32, &traffic, &names)))
}

/// Every app id the aggregate mentioned, from either list, deduped and sorted.
///
/// Sorted so the name lookup's bound parameters are stable between requests —
/// an unordered `IN` list defeats statement caching for no reason.
pub(super) fn collect_app_ids(traffic: &OrgAppTraffic) -> Vec<Uuid> {
    traffic
        .by_app
        .iter()
        .map(|a| a.app_id.as_str())
        .chain(traffic.routes.iter().map(|r| r.app_id.as_str()))
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Shape the store's aggregate into the response.
///
/// Pure, and out of the handler: `backend-architecture.md` keeps HTTP handlers
/// to parse → call → serialize, and this is the fold — resolving names,
/// applying the missing-app fallback, and deriving every rate. It is also the
/// only way to test the fallback below without driving an HTTP request.
pub(super) fn build_org_apps(
    window_days: u32,
    traffic: &OrgAppTraffic,
    names: &HashMap<Uuid, (String, String)>,
) -> OrgApps {
    let name_of = |id: &str| -> (String, String) {
        Uuid::parse_str(id)
            .ok()
            .and_then(|u| names.get(&u).cloned())
            // An app with no catalog row keeps its id as its label: it
            // contributed to the totals above, so hiding it would make the
            // breakdown fail to add up.
            .unwrap_or_else(|| (id.to_string(), String::new()))
    };

    let t = &traffic.totals;
    OrgApps {
        window_days,
        measured: true,
        total: AppTotals {
            requests: t.requests,
            failed: t.failed,
            sessions: t.sessions,
            people: t.people,
            apps: t.apps,
            bytes: t.bytes,
            host_calls: t.host_calls,
            p50_ms: t.p50_ms,
            p95_ms: t.p95_ms,
            p99_ms: t.p99_ms,
            cold_start_p95_ms: t.cold_start_p95_ms,
            wall_ms: t.wall_ms,
            init_ms: t.init_ms,
            failure_rate: t.failure_ratio(),
        },
        failures: traffic
            .outcomes
            .iter()
            .map(|o| AppFailure {
                outcome: o.outcome.clone(),
                error_kind: o.error_kind.clone(),
                requests: o.requests,
                // Of all requests, not of all failures: "3% of traffic was a
                // white screen" is the actionable framing; "40% of failures"
                // hides whether there were four of them or forty thousand.
                share: super::derive::failure_rate(o.requests, t.requests),
            })
            .collect(),
        by_day: traffic
            .by_day
            .iter()
            .map(|d| AppDay {
                day: d.day.clone(),
                requests: d.requests,
                failed: d.failed,
                p95_ms: d.p95_ms,
                wall_ms: d.wall_ms,
                init_ms: d.init_ms,
                failure_rate: super::derive::failure_rate(d.failed, d.requests),
            })
            .collect(),
        by_app: traffic
            .by_app
            .iter()
            .map(|a| {
                let (name, slug) = name_of(&a.app_id);
                AppRow {
                    app_id: a.app_id.clone(),
                    name,
                    slug,
                    requests: a.requests,
                    failed: a.failed,
                    sessions: a.sessions,
                    bytes: a.bytes,
                    p50_ms: a.p50_ms,
                    p95_ms: a.p95_ms,
                    cold_start_p95_ms: a.cold_start_p95_ms,
                    failure_rate: super::derive::failure_rate(a.failed, a.requests),
                }
            })
            .collect(),
        routes: traffic
            .routes
            .iter()
            .map(|r| AppRoute {
                app_id: r.app_id.clone(),
                app_name: name_of(&r.app_id).0,
                route: r.route.clone(),
                requests: r.requests,
                failed: r.failed,
                p95_ms: r.p95_ms,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failure class's share is of **all traffic**, not of all failures.
    /// "40% of failures were white screens" hides whether there were four of
    /// them or forty thousand; "3% of traffic was a white screen" does not.
    #[test]
    fn a_failure_class_share_is_of_all_traffic() {
        // 30 white screens out of 1,000 requests is 3%, not 100% of failures.
        assert_eq!(super::super::derive::failure_rate(30, 1_000), Some(0.03));
    }

    /// An app the catalog no longer has keeps its id as its label. It
    /// contributed to the totals, so dropping the row would leave a breakdown
    /// that does not add up to the figure above it — and events outlive a
    /// deleted app by up to the retention window.
    #[test]
    fn an_app_with_no_catalog_row_keeps_its_id_as_its_name() {
        let names: HashMap<Uuid, (String, String)> = HashMap::new();
        let id = Uuid::from_u128(42).to_string();
        let resolved = Uuid::parse_str(&id)
            .ok()
            .and_then(|u| names.get(&u).cloned())
            .unwrap_or_else(|| (id.clone(), String::new()));
        assert_eq!(resolved.0, id, "the id stands in for a missing name");
        assert_eq!(resolved.1, "", "and there is no slug to link with");
    }

    /// An app the catalog no longer has keeps its id as its label and an
    /// **empty slug** — which is how the table knows not to link it. It
    /// contributed to the totals, so dropping the row would leave a breakdown
    /// that does not add up to the figure above it.
    #[test]
    fn a_missing_app_keeps_its_id_and_has_no_slug() {
        let mut traffic = OrgAppTraffic::default();
        traffic.by_app.push(oxy_observability::types::OrgAppRow {
            app_id: "11111111-1111-4111-8111-111111111111".into(),
            requests: 10,
            failed: 0,
            sessions: 1,
            bytes: 0,
            p50_ms: 1.0,
            p95_ms: 2.0,
            cold_start_p95_ms: 0.0,
        });
        let out = build_org_apps(30, &traffic, &HashMap::new());
        assert_eq!(out.by_app[0].name, "11111111-1111-4111-8111-111111111111");
        assert_eq!(
            out.by_app[0].slug, "",
            "no slug means no console to link to"
        );
    }

    /// A known app carries its name and slug, which is what the row links by.
    #[test]
    fn a_known_app_carries_its_name_and_slug() {
        let id = Uuid::from_u128(9);
        let mut traffic = OrgAppTraffic::default();
        traffic.by_app.push(oxy_observability::types::OrgAppRow {
            app_id: id.to_string(),
            requests: 1,
            failed: 0,
            sessions: 0,
            bytes: 0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            cold_start_p95_ms: 0.0,
        });
        let names = HashMap::from([(
            id,
            ("Lease Tracker".to_string(), "lease-tracker".to_string()),
        )]);
        let out = build_org_apps(30, &traffic, &names);
        assert_eq!(out.by_app[0].name, "Lease Tracker");
        assert_eq!(out.by_app[0].slug, "lease-tracker");
    }

    /// Ids are collected from both lists and deduped: an app that appears in
    /// the breakdown and again in the slowest routes is looked up once.
    #[test]
    fn app_ids_are_collected_from_both_lists_and_deduped() {
        let id = Uuid::from_u128(4).to_string();
        let mut traffic = OrgAppTraffic::default();
        traffic.by_app.push(oxy_observability::types::OrgAppRow {
            app_id: id.clone(),
            requests: 1,
            failed: 0,
            sessions: 0,
            bytes: 0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            cold_start_p95_ms: 0.0,
        });
        traffic.routes.push(oxy_observability::types::OrgAppRoute {
            app_id: id.clone(),
            route: "/api/fn/x".into(),
            requests: 1,
            failed: 0,
            p95_ms: 1.0,
        });
        // A malformed id is skipped rather than failing the lookup.
        traffic.routes.push(oxy_observability::types::OrgAppRoute {
            app_id: "not-a-uuid".into(),
            route: "/api/fn/y".into(),
            requests: 1,
            failed: 0,
            p95_ms: 1.0,
        });
        assert_eq!(collect_app_ids(&traffic), vec![Uuid::from_u128(4)]);
    }

    /// The name lookup builds no statement when there is nothing to look up.
    /// An empty `IN ()` is a syntax error, and an org with no app traffic is
    /// an ordinary state — so the absence is the contract, not a tolerated
    /// query that happens to return nothing.
    #[test]
    fn no_app_ids_means_no_statement() {
        assert_eq!(app_names_sql(&[], &mut Vec::new()), None);
    }

    /// Each id is bound, never formatted in. These ids come from the
    /// observability store's rows, so they are not caller input — but the
    /// lookup is one `format!` away from being, and a bound parameter cannot
    /// become SQL.
    #[test]
    fn app_ids_are_bound_as_numbered_placeholders() {
        let mut values: Vec<sea_orm::Value> = Vec::new();
        let sql = app_names_sql(&[Uuid::nil(), Uuid::from_u128(5)], &mut values)
            .expect("two ids make a statement");
        assert!(sql.contains("WHERE id IN ($1,$2)"), "{sql}");
        assert_eq!(values.len(), 2);
    }
}
