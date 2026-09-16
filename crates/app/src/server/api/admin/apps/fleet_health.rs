//! `GET /api/admin/apps/health` — every published custom app and whether it is
//! working.
//!
//! See `internal-docs/2026-09-16-custom-app-fleet-observability-design.md`.
//!
//! ## What this is for
//!
//! Until this endpoint there was no fleet view of custom-app health at all. The
//! cockpit's status LED is `isLive ? "Live" : "Draft"` — publish state.
//! `app_availability::gather` computes a burn verdict *per app* and then rolls it
//! into one per-workspace `HealthStatus`, discarding the per-app answer, and only
//! for workspaces that opted into Workspace Health with a `health_check:` block.
//! So an operator asking "which of our apps are broken right now" had nowhere to
//! look.
//!
//! ## Why it is allowed to say "I don't know"
//!
//! Four separate layers turn an absent measurement into a green result today:
//! capture unconfigured, the workspace never evaluated, the ClickHouse query
//! erroring, and traffic below the burn evaluator's floor. Every one of them ends
//! as `HealthStatus::Healthy`.
//!
//! This endpoint reports [`AppHealth::NotMeasured`] and [`AppHealth::Quiet`]
//! instead, and never folds either into `Operational`. That is the whole point of
//! the surface: a fleet table whose green ticks include the apps nobody is
//! measuring is worse than no table, because it converts an open question into a
//! false answer.
//!
//! ## Cost and classification
//!
//! Reads Postgres (`apps`, `organizations`) and ClickHouse. No working copy, no
//! `.git`, no state dir — **FleetOk**, any replica answers. That matters here
//! more than usual: a fleet-health endpoint pinned to the singleton goes dark
//! exactly when the singleton is the thing in trouble.
//!
//! ClickHouse work is bounded by the page, not by the fleet: the availability
//! read is one grouped query per window rather than one per app, and the
//! heartbeat baseline is a single additional query. Both are scoped
//! `(org_id, app_id)` tuples so they lead with the table's sort key.

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use entity::apps;
use entity::prelude::Apps;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_observability::burn_rate::{ALERT_WINDOWS_MINUTES, SloConfig, evaluate as evaluate_burn};
use oxy_observability::fleet_health::{AppHealth, AppVerdict, classify};
use oxy_observability::heartbeat::{self, Heartbeat};
use oxy_observability::types::AppAvailabilityWindow;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::handlers::scope_org_filter;
use super::ops::org_slugs_for;

/// Most apps one request may evaluate.
///
/// The ClickHouse predicate carries one tuple per app, so an unbounded page
/// would build an unbounded `IN` list — and unbounded observability queries have
/// taken this backend offline before, which is why the cap is here and not
/// merely in the frontend's page size.
const MAX_PAGE: u64 = 200;

/// The window whose request count the table shows, and which the heartbeat
/// compares against a cycle ago. Both must name the same span or the "traffic"
/// column and the silence verdict describe different things.
const TRAFFIC_WINDOW_MINUTES: u32 = heartbeat::WINDOW_MINUTES;

#[derive(Debug, Deserialize)]
pub struct FleetHealthQuery {
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    offset: Option<u64>,
    /// `true` to return only the apps an operator should look at — down,
    /// degraded, or unmeasured. Quiet and operational apps are dropped from
    /// `apps`, but still counted in `summary`.
    #[serde(default)]
    needs_attention: Option<bool>,
}

impl FleetHealthQuery {
    fn needs_attention(&self) -> bool {
        self.needs_attention.unwrap_or(false)
    }
}

/// One app's row in the fleet table.
#[derive(Debug, Serialize)]
pub struct AppHealthRow {
    pub app_id: Uuid,
    pub app_slug: String,
    pub app_name: String,
    pub org_id: Uuid,
    pub org_slug: String,
    pub health: AppHealth,
    /// Why, in a line. `None` only when the app is plainly operational.
    pub reason: Option<String>,
    /// Requests over `window_minutes`, and how many of them failed.
    /// Both `0` on an unmeasured app — read `health` first, not these.
    pub requests: u64,
    pub failed: u64,
    /// What the same window carried one cycle ago, when there was a baseline.
    /// `None` means no established rhythm to compare against.
    pub baseline: Option<u64>,
    pub window_minutes: u32,
}

#[derive(Debug, Serialize)]
pub struct FleetHealthResponse {
    pub apps: Vec<AppHealthRow>,
    /// Counts by verdict across the returned page, for the summary strip.
    pub summary: FleetSummary,
    /// Published apps in scope, before paging. `apps.len()` can be smaller for
    /// two independent reasons — the page cap and `needs_attention` — and a
    /// reader that cannot tell "that is all of them" from "that is the first
    /// 200" will believe the wrong one.
    pub total: u64,
    /// Whether apps in scope were left off this page — by the page cap, not by
    /// `needs_attention`, which never hides an app the operator asked to see.
    pub has_more: bool,
    /// When this was evaluated. A stale value on a page that is supposed to
    /// refresh is how a dead evaluator is caught — without it, "nothing has
    /// changed" and "nothing is running" look identical.
    pub evaluated_at: String,
    /// `false` when `OXY_OBSERVABILITY_BACKEND` is unset. The UI must say so
    /// rather than render an all-green table: with capture off, every row is
    /// `not_measured` and the fleet is not being watched at all.
    pub observability_configured: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct FleetSummary {
    pub down: u64,
    pub degraded: u64,
    pub not_measured: u64,
    pub quiet: u64,
    pub operational: u64,
}

impl FleetSummary {
    fn count(&mut self, health: AppHealth) {
        let slot = match health {
            AppHealth::Down => &mut self.down,
            AppHealth::Degraded => &mut self.degraded,
            AppHealth::NotMeasured => &mut self.not_measured,
            AppHealth::Quiet => &mut self.quiet,
            AppHealth::Operational => &mut self.operational,
        };
        *slot += 1;
    }
}

pub async fn get_fleet_health(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<FleetHealthQuery>,
) -> Result<Json<FleetHealthResponse>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // Same row filter as the registry list: capabilities gate verbs, scope
    // filters rows. An out-of-scope app is absent, not denied.
    let scope = scope_org_filter(&db, &user).await;
    let limit = q.limit.unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE);
    let (rows, total) = published_page(&db, scope, limit, q.offset).await?;

    let org_slugs = org_slugs_for(&db, &rows).await.unwrap_or_else(|e| {
        // A missing slug costs a display name, not a verdict. The health of the
        // fleet is the point of this endpoint; it must not 500 over cosmetics.
        tracing::warn!("fleet health: org slug lookup failed: {e}");
        Default::default()
    });

    let fleet = evaluate_fleet(&rows).await;
    let (apps, summary) = build_rows(&rows, &org_slugs, &fleet, q.needs_attention());

    Ok(Json(FleetHealthResponse {
        apps,
        summary,
        total,
        has_more: has_more(q.offset.unwrap_or(0), rows.len() as u64, total),
        evaluated_at: chrono::Utc::now().to_rfc3339(),
        observability_configured: oxy_observability::global::get_global().is_some(),
    }))
}

/// Whether apps in scope were left off this page.
///
/// Truncation has to be visible: a page that silently stops at [`MAX_PAGE`]
/// reads as "that is the whole fleet", which on this surface is the same class
/// of lie as reporting an unmeasured app as healthy.
///
/// `offset + returned` is how many rows have been consumed through the end of
/// this page, so the test is whether anything remains **after** it. Shipped once
/// as `>`, which is false for every input — the query cannot return rows that do
/// not exist — so the banner never rendered and the truncation stayed exactly as
/// silent as before. Pure, so the comparison is pinned by a unit test rather
/// than by a fixture of 201 published apps.
fn has_more(offset: u64, returned: u64, total: u64) -> bool {
    offset + returned < total
}

/// One page of published apps, plus how many there are in total.
///
/// Only published apps: a draft serves nobody, so it has no availability to
/// report and would fill the table with rows that can only ever be quiet.
async fn published_page(
    db: &sea_orm::DatabaseConnection,
    scope: Option<Vec<Uuid>>,
    limit: u64,
    offset: Option<u64>,
) -> Result<(Vec<apps::Model>, u64), StatusCode> {
    let base = || {
        let mut q = Apps::find().filter(apps::Column::PublishedAt.is_not_null());
        if let Some(orgs) = &scope {
            q = q.filter(apps::Column::OrgId.is_in(orgs.clone()));
        }
        q
    };
    let total = base().count(db).await.map_err(|e| {
        tracing::error!("fleet health: app count failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let rows = base()
        .order_by_asc(apps::Column::Slug)
        .limit(Some(limit))
        .offset(offset)
        .all(db)
        .await
        .map_err(|e| {
            tracing::error!("fleet health: app lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok((rows, total))
}

/// Turn the page and its verdicts into rows, worst first, plus the summary.
///
/// **The summary counts every app on the page, including ones `needs_attention`
/// filters out of `apps`.** A strip whose numbers changed depending on the
/// filter would be describing the filter rather than the fleet — and the
/// filter's own buttons are rendered from it.
fn build_rows(
    rows: &[apps::Model],
    org_slugs: &std::collections::HashMap<Uuid, String>,
    fleet: &Fleet,
    attention_only: bool,
) -> (Vec<AppHealthRow>, FleetSummary) {
    let mut summary = FleetSummary::default();
    let mut out = Vec::with_capacity(rows.len());

    for app in rows {
        let assessed = fleet.assessed.get(&app.id);
        let verdict = assessed
            .map(|a| a.verdict.clone())
            .unwrap_or_else(|| AppVerdict::not_measured(fleet.unmeasured));
        summary.count(verdict.health);
        if attention_only && !verdict.health.needs_attention() {
            continue;
        }
        let window = assessed.and_then(|a| a.traffic_window());
        out.push(AppHealthRow {
            app_id: app.id,
            app_slug: app.slug.clone(),
            app_name: app.name.clone(),
            org_id: app.org_id,
            org_slug: org_slugs.get(&app.org_id).cloned().unwrap_or_default(),
            health: verdict.health,
            reason: verdict.reason,
            requests: window.map(|w| w.total).unwrap_or(0),
            failed: window.map(|w| w.failed).unwrap_or(0),
            baseline: assessed.and_then(|a| a.baseline),
            window_minutes: TRAFFIC_WINDOW_MINUTES,
        });
    }

    // Worst first. `AppHealth` orders that way, and the secondary key keeps rows
    // from shuffling between refreshes at equal severity.
    out.sort_by(|a, b| {
        a.health
            .cmp(&b.health)
            .then_with(|| a.org_slug.cmp(&b.org_slug))
            .then_with(|| a.app_slug.cmp(&b.app_slug))
    });
    (out, summary)
}

/// Why an app ended up `not_measured`. These have **different fixes**, so the
/// row has to say which one it hit: capture being off is a deployment setting,
/// a failing query is an outage in the observability store, and conflating them
/// sends an operator to the wrong place.
///
/// Capture off applies to every row at once, and the response's
/// `observability_configured: false` lets the UI explain it once for the page
/// rather than repeating it down a column.
const NO_CAPTURE: &str = "observability capture is not configured (OXY_OBSERVABILITY_BACKEND)";
/// Placeholder for a page with no apps in it: there is no row to carry a
/// reason, so this is never rendered.
const NOTHING_ASKED: &str = "no apps in scope";
const QUERY_FAILED: &str =
    "the observability store did not answer — this is not a verdict about the app";

/// What one pass over the fleet produced.
struct Fleet {
    assessed: std::collections::HashMap<Uuid, Assessed>,
    /// The reason to report for any app absent from `assessed`.
    unmeasured: &'static str,
}

/// One app's assessment plus the raw numbers behind it.
struct Assessed {
    verdict: AppVerdict,
    windows: Vec<AppAvailabilityWindow>,
    baseline: Option<u64>,
}

impl Assessed {
    fn traffic_window(&self) -> Option<&AppAvailabilityWindow> {
        self.windows
            .iter()
            .find(|w| w.window_minutes == TRAFFIC_WINDOW_MINUTES)
    }
}

/// Evaluate every app in the page against ClickHouse.
///
/// Returns an entry per app that was **measured**. An app missing from the map
/// was not assessed, and the caller reports it as `not_measured` — the
/// distinction this whole module exists to preserve.
async fn evaluate_fleet(rows: &[apps::Model]) -> Fleet {
    let mut out = std::collections::HashMap::new();
    let Some(store) = oxy_observability::global::get_global() else {
        // Capture is off — the default on a developer's `oxy serve`. Every row
        // becomes `not_measured`, which is the honest answer.
        return Fleet {
            assessed: out,
            unmeasured: NO_CAPTURE,
        };
    };
    if rows.is_empty() {
        // Nothing was asked about, so nothing is unmeasured. The reason is
        // unreachable — no row can look it up — but naming it QUERY_FAILED here
        // would put a lie one refactor away from being read.
        return Fleet {
            assessed: out,
            unmeasured: NOTHING_ASKED,
        };
    }

    let keys: Vec<(String, String)> = rows
        .iter()
        .map(|a| (a.org_id.to_string(), a.id.to_string()))
        .collect();

    let availability = match store
        .get_fleet_availability(&keys, ALERT_WINDOWS_MINUTES)
        .await
    {
        Ok(a) => a,
        Err(e) => {
            // A failed query is NOT a healthy fleet. Returning empty here leaves
            // every row `not_measured`, which is what the operator guide says is
            // missing today: "a broken query looks exactly like all apps healthy".
            tracing::warn!("fleet health: availability query failed: {e}");
            return Fleet {
                assessed: out,
                unmeasured: QUERY_FAILED,
            };
        }
    };

    // Best-effort: without a baseline the heartbeat simply has no opinion, which
    // is a correct verdict rather than a degraded one. A burn verdict is still
    // worth reporting, so this failure must not take the page down with it.
    let baselines = store
        .get_fleet_heartbeat_baseline(&keys, heartbeat::WINDOW_MINUTES, heartbeat::CYCLE_DAYS)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("fleet health: heartbeat baseline query failed: {e}");
            Default::default()
        });

    let cfg = SloConfig::default();
    for app in rows {
        let id = app.id.to_string();
        let Some(windows) = availability.get(&id) else {
            continue;
        };
        let baseline = baselines.get(&id).copied();
        let current = windows
            .iter()
            .find(|w| w.window_minutes == heartbeat::WINDOW_MINUTES)
            .map(|w| w.total)
            .unwrap_or(0);
        let beat = match baseline {
            Some(b) => heartbeat::evaluate(current, b),
            None => Heartbeat::NoBaseline,
        };
        let burn = evaluate_burn(windows, &cfg);
        out.insert(
            app.id,
            Assessed {
                verdict: classify(windows, &burn, beat),
                windows: windows.clone(),
                baseline,
            },
        );
    }
    Fleet {
        assessed: out,
        // Reached only if an app is missing from a successful response, which
        // `get_fleet_availability` does not do — it seeds every app it is
        // asked about. Naming the store is still the right answer if it ever
        // does: what is missing is the measurement, not the configuration.
        unmeasured: QUERY_FAILED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comparison shipped inverted once, which made the whole
    /// truncation-is-visible change a no-op. These are the four cases that
    /// distinguish `<` from `>`.
    #[test]
    fn has_more_is_true_only_when_rows_remain_after_this_page() {
        // A full page with more behind it — the case the banner exists for.
        assert!(has_more(0, MAX_PAGE, MAX_PAGE + 1));
        // The whole fleet fits: consumed == total.
        assert!(!has_more(0, MAX_PAGE, MAX_PAGE));
        assert!(!has_more(0, 3, 3));
        // Paged to the end, and past it.
        assert!(has_more(MAX_PAGE, MAX_PAGE, MAX_PAGE * 3));
        assert!(!has_more(MAX_PAGE * 2, 0, MAX_PAGE * 2));
        assert!(!has_more(500, 0, 10), "an offset past the end is not more");
        // An empty fleet cannot have more.
        assert!(!has_more(0, 0, 0));
    }

    #[test]
    fn summary_counts_each_verdict_into_its_own_slot() {
        let mut s = FleetSummary::default();
        s.count(AppHealth::Down);
        s.count(AppHealth::Down);
        s.count(AppHealth::NotMeasured);
        s.count(AppHealth::Operational);
        assert_eq!(s.down, 2);
        assert_eq!(s.not_measured, 1);
        assert_eq!(s.operational, 1);
        assert_eq!(s.degraded, 0);
        assert_eq!(s.quiet, 0);
    }

    /// The traffic column and the heartbeat must describe the same span, or the
    /// table shows a request count over one window and calls the app silent over
    /// another.
    #[test]
    fn the_traffic_window_is_the_heartbeat_window() {
        assert_eq!(TRAFFIC_WINDOW_MINUTES, heartbeat::WINDOW_MINUTES);
        assert!(ALERT_WINDOWS_MINUTES.contains(&TRAFFIC_WINDOW_MINUTES));
    }

    /// An unmeasured app must not be filtered out of an attention-only view:
    /// "we are not watching this" is exactly what an operator needs to see.
    #[test]
    fn unmeasured_apps_survive_the_needs_attention_filter() {
        assert!(AppHealth::NotMeasured.needs_attention());
        assert!(AppHealth::Down.needs_attention());
        assert!(AppHealth::Degraded.needs_attention());
        assert!(!AppHealth::Operational.needs_attention());
        assert!(!AppHealth::Quiet.needs_attention());
    }
}
