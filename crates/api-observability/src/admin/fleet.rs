//! `GET /admin/observability/fleet` — one row per org: what it costs to serve,
//! what it did for that, and how its apps performed, so the surface answers
//! *"what is out there?"* before *"which one thing?"*.
//!
//! That ordering is a rule the admin console wrote for itself after
//! `/admin/apps` shipped as a redirect to whichever app most needed a person:
//! "a redirect answers 'which one thing?' while hiding 'what is out there?',
//! and it made the URL resolve differently depending on data"
//! (`web-app/src/pages/admin/CLAUDE.md`).
//!
//! Orgs with **no** spend and no traffic are included, not filtered out. An
//! org that went quiet is a finding, and a list that only shows active orgs
//! cannot show it.
//!
//! **Cost to serve** — LLM spend plus the app resources this surface can price
//! — and the rules for comparing it with the window before live in
//! `cost_to_serve`. This module reads, folds the rows, and serializes.

use axum::extract::{Query, State};
use axum::response::Response;
use axum::{Json, Router, routing::get};
use oxy_app::surface::admin_db::db_err;
use oxy_app_core::AppState;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

use super::cost_rollup::{self, Period};
use super::cost_to_serve::{
    self, BYTES_PER_GB_DECIMAL, Basis, PriorMissing, PriorReads, TrafficRead, Window,
    accumulate_cost, cost_to_serve,
};
use super::{WindowQuery, connect_fleet, rates, storage_rollup};

/// One org's row.
///
/// Every app-side field is `Option`, and `None` means **not measured** — this
/// deployment has no observability store — rather than zero. The two are
/// different answers to "how is this org's app doing", and the column header
/// says which, once, instead of every row showing a dash.
#[derive(Debug, Serialize)]
pub struct FleetRow {
    pub org_id: Uuid,
    pub org_name: String,
    pub org_slug: String,
    // Cost (Postgres: always answerable)
    /// Priced LLM spend over the window.
    pub cost_usd: f64,
    pub run_count: i64,
    pub unpriced_runs: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_per_run_usd: Option<f64>,
    pub monthly_run_rate_usd: Option<f64>,
    /// Fraction of runs a schedule seeded. Computed from runs rather than
    /// dollars here, because the fleet rollup groups by model and summing a
    /// per-model dollar split per attendance class would double the statement.
    pub unattended_run_share: Option<f64>,
    pub cache_hit_share: Option<f64>,
    // Custom-app performance (ClickHouse: absent when unmeasured)
    pub requests: Option<u64>,
    pub sessions: Option<u64>,
    pub people: Option<u64>,
    pub apps: Option<u64>,
    pub p95_ms: Option<f64>,
    pub failure_rate: Option<f64>,
    // Resource quantities
    /// Provisioned GB-seconds of function compute. `None` when unmeasured, or
    /// when the isolate ceiling is disabled and there is no basis to convert
    /// wall time against.
    pub function_compute_gbs: Option<f64>,
    /// Egress in decimal GB — the unit it is billed in, unlike storage.
    pub egress_gb: Option<f64>,
    /// Time-weighted storage over the window. `None` means **no readings**,
    /// not an empty bucket.
    pub storage_gib_month: Option<f64>,
    // Cost to serve
    /// App storage at S3 list. `None` when there are no readings.
    pub storage_usd: Option<f64>,
    /// App egress at AWS list. `None` when apps are unmeasured.
    pub egress_usd: Option<f64>,
    /// This org's share of the operator-supplied serving bill. `None` without
    /// one: the fleet's price is never invented.
    pub compute_usd: Option<f64>,
    /// LLM spend plus every priced component above.
    pub cost_to_serve_usd: f64,
    /// The same total over the `days` before the window, read with the same
    /// components. `None` when the prior window could not be read — no
    /// comparison, rather than a comparison against zero.
    pub prior_cost_to_serve_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Fleet {
    pub window_days: u32,
    /// False when no observability store is configured, so every row's app
    /// fields are absent for want of a source.
    pub apps_measured: bool,
    /// False when the storage read itself failed. Distinct from an org with no
    /// readings: one is a fault, the other is the sweeper not having reached
    /// an app yet, and the page must not show a database error as the latter.
    pub storage_read: bool,
    /// True when an operator-supplied fleet bill prices function compute, so
    /// the page can say which components its totals include.
    pub compute_priced: bool,
    /// False when there is no prior window to compare against — it was past
    /// retention, a read of it failed, or it priced different components — so
    /// the page drops its comparison rather than reporting every org as new.
    pub prior_read: bool,
    /// Why `prior_read` is false — past retention, a failed read, or windows
    /// that priced different components — so the page says which. Only a
    /// failed read is a fault. `None` when there is a comparison.
    pub prior_missing: Option<PriorMissing>,
    /// What a compute figure assumes, so the page can say so.
    pub provisioned_memory_mb: Option<f64>,
    pub rows: Vec<FleetRow>,
}

impl Fleet {
    /// The answer for a grant that reaches no org: nothing to read, so
    /// nothing failed.
    fn empty(window_days: u32, apps_measured: bool) -> Self {
        Self {
            window_days,
            apps_measured,
            storage_read: true,
            compute_priced: false,
            prior_read: true,
            prior_missing: None,
            provisioned_memory_mb: rates::provisioned_memory_mb(),
            rows: Vec::new(),
        }
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/observability/fleet", get(fleet))
}

/// Every org the caller may see, named, so an org with no activity still has a
/// row. The fence is the caller's bounded grant, applied the same way the cost
/// rollup applies it.
#[derive(Debug, FromQueryResult)]
pub(super) struct OrgRow {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) slug: String,
}

/// The org list's statement, with each in-scope org bound as its own
/// placeholder. Pure, so the fence is testable without a database — this list
/// is the page's spine and every row comes from it.
fn visible_orgs_sql(scope: Option<&[Uuid]>, values: &mut Vec<sea_orm::Value>) -> String {
    let fence = match scope {
        None => String::new(),
        Some(orgs) => {
            let placeholders = orgs
                .iter()
                .map(|org| {
                    values.push((*org).into());
                    format!("${}", values.len())
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(" WHERE id IN ({placeholders})")
        }
    };
    format!("SELECT id, name, slug FROM organizations{fence} ORDER BY name")
}

async fn visible_orgs(
    db: &sea_orm::DatabaseConnection,
    scope: Option<&[Uuid]>,
) -> Result<Vec<OrgRow>, sea_orm::DbErr> {
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let sql = visible_orgs_sql(scope, &mut values);
    OrgRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await
}

/// `[now - (days + offset), now - offset]`, for the storage rollup's explicit
/// bounds. The ClickHouse and Postgres reads express the same window in SQL.
fn bounds(
    days: i32,
    offset_days: i32,
) -> (
    chrono::DateTime<chrono::FixedOffset>,
    chrono::DateTime<chrono::FixedOffset>,
) {
    let end = chrono::Utc::now() - chrono::Duration::days(i64::from(offset_days));
    (
        (end - chrono::Duration::days(i64::from(days))).into(),
        end.into(),
    )
}

/// The app-store reads for one window. `Ok(None)` when the deployment has no
/// observability store; the fleet's wall time only when a bill needs it.
async fn read_traffic(
    state: &AppState,
    org_ids: &[String],
    days: u32,
    offset_days: u32,
    priced: bool,
) -> Result<TrafficRead, oxy_shared::errors::OxyError> {
    let Some(store) = state.observability.as_ref() else {
        return Ok(None);
    };
    let traffic = store
        .get_fleet_org_traffic(org_ids, days, offset_days)
        .await?;
    // The apportioning denominator. Its failure leaves compute unpriced for
    // the window rather than failing the page: every other column stands.
    let fleet_wall_ms = if priced {
        match store.get_fleet_wall_ms(days, offset_days).await {
            Ok(ms) => Some(ms as f64),
            Err(e) => {
                tracing::warn!(error = %e, offset_days, "fleet wall time read failed");
                None
            }
        }
    } else {
        None
    };
    Ok(Some((traffic, fleet_wall_ms)))
}

/// What every read of one request shares.
struct Reader<'a> {
    state: &'a AppState,
    db: &'a sea_orm::DatabaseConnection,
    scope: Option<&'a [Uuid]>,
    org_ids: &'a [Uuid],
    org_keys: Vec<String>,
    days: i32,
    priced: bool,
}

type WindowReads = (
    Result<Vec<cost_rollup::FleetOrgRow>, sea_orm::DbErr>,
    Result<HashMap<Uuid, storage_rollup::OrgStorage>, sea_orm::DbErr>,
    Result<TrafficRead, oxy_shared::errors::OxyError>,
);

impl Reader<'_> {
    /// One window's cost, storage and traffic reads, run together.
    async fn window(&self, period: Period) -> WindowReads {
        let offset = match period {
            Period::Current => 0,
            Period::Prior => self.days,
        };
        let (start, end) = bounds(self.days, offset);
        let window = self.days.max(0) as u32;
        futures::join!(
            cost_rollup::fetch_fleet(self.db, self.days, self.scope, period),
            storage_rollup::fleet_storage(self.db, self.org_ids, start, end),
            read_traffic(
                self.state,
                &self.org_keys,
                window,
                offset.max(0) as u32,
                self.priced
            ),
        )
    }

    /// The prior window, when it is still retained and every read of it
    /// came back; otherwise why not.
    async fn prior(&self, retained: bool) -> Result<PriorReads, PriorMissing> {
        if !retained {
            return Err(PriorMissing::Retention);
        }
        let (cost, storage, traffic) = self.window(Period::Prior).await;
        cost_to_serve::gather_prior(cost, storage, traffic).ok_or(PriorMissing::ReadFailed)
    }
}

/// Storage: a failure must not blank the page — the other columns are
/// independent — but it must not pass for "the sweeper is behind" either, so
/// it is logged and the response says `storage_read` failed.
fn storage_or_unread(
    read: Result<HashMap<Uuid, storage_rollup::OrgStorage>, sea_orm::DbErr>,
    orgs: usize,
) -> (HashMap<Uuid, storage_rollup::OrgStorage>, bool) {
    match read {
        Ok(s) => (s, true),
        Err(e) => {
            tracing::warn!(error = %e, orgs, "fleet storage rollup failed");
            (HashMap::new(), false)
        }
    }
}

async fn fleet(
    State(state): State<AppState>,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(q): Query<WindowQuery>,
) -> Result<Json<Fleet>, Response> {
    let days = q.days();
    let window = days.max(0) as u32;
    let (db, scope) = connect_fleet(&actor).await?;
    let apps_measured = state.observability.is_some();

    // A bounded grant that reaches no org sees an empty list, and no statement
    // runs: an empty `IN ()` would either fail to parse or, read the other way,
    // match every org — which is the opposite of what the fence is for.
    if scope.as_deref().is_some_and(<[Uuid]>::is_empty) {
        return Ok(Json(Fleet::empty(window, apps_measured)));
    }

    let orgs = visible_orgs(&db, scope.as_deref()).await.map_err(db_err)?;
    let org_ids: Vec<Uuid> = orgs.iter().map(|o| o.id).collect();
    let fleet_bill = rates::fleet_cost_usd_month();
    let reader = Reader {
        state: &state,
        db: &db,
        scope: scope.as_deref(),
        org_ids: &org_ids,
        org_keys: org_ids.iter().map(Uuid::to_string).collect(),
        days,
        priced: fleet_bill.is_some(),
    };
    let retained =
        cost_to_serve::prior_retained(window, cost_to_serve::retention_horizon_days(apps_measured));

    // Both windows at once. The current window's cost and traffic reads fail
    // the page, as they always have; the prior is a comparison, and one that
    // cannot be made is dropped, not invented.
    let ((cost_rows, storage, traffic), prior) =
        futures::join!(reader.window(Period::Current), reader.prior(retained));
    let cost_rows = cost_rows.map_err(db_err)?;
    let (traffic, fleet_wall_ms) = traffic.map_err(super::store_err)?.unwrap_or_default();
    let (storage, storage_read) = storage_or_unread(storage, org_ids.len());

    let current = Window {
        cost_rows: &cost_rows,
        traffic: &traffic,
        storage: &storage,
        basis: Basis {
            days: window,
            apps_measured,
            include_storage: storage_read,
            fleet_wall_ms,
            fleet_bill,
        },
    };
    let prior_totals = cost_to_serve::compare(&org_ids, &current.basis, prior);

    let provisioned_mb = rates::provisioned_memory_mb();
    let rows = fold_fleet(FleetFold {
        orgs,
        current: &current,
        prior: prior_totals.as_ref().ok(),
        provisioned_mb,
        days,
    });

    Ok(Json(Fleet {
        window_days: window,
        apps_measured,
        storage_read,
        compute_priced: current.basis.compute_priced(),
        prior_read: prior_totals.is_ok(),
        prior_missing: prior_totals.as_ref().err().copied(),
        provisioned_memory_mb: provisioned_mb,
        rows,
    }))
}

/// Everything the per-org fold needs. A struct rather than a list of
/// arguments, and the reason the fold is not in the handler at all: pricing
/// and aggregation are business logic, and `backend-architecture.md` keeps
/// HTTP handlers to parse → call → serialize. `cost.rs` already splits this
/// way with `fold_days` / `fold_buckets`; this follows it.
pub(super) struct FleetFold<'a> {
    pub(super) orgs: Vec<OrgRow>,
    pub(super) current: &'a Window<'a>,
    /// Each org's prior-window total, or `None` when it could not be read.
    pub(super) prior: Option<&'a HashMap<Uuid, f64>>,
    pub(super) provisioned_mb: Option<f64>,
    pub(super) days: i32,
}

/// Build the fleet's rows. Pure: every input is already read.
pub(super) fn fold_fleet(f: FleetFold<'_>) -> Vec<FleetRow> {
    let w = f.current;
    let by_org = accumulate_cost(w.cost_rows);
    f.orgs
        .into_iter()
        .map(|o| {
            let acc = by_org.get(&o.id);
            let cost_usd = acc.map(|a| a.cost_usd).unwrap_or(0.0);
            let run_count = acc.map(|a| a.run_count).unwrap_or(0);
            let unpriced_runs = acc.map(|a| a.unpriced_runs).unwrap_or(0);
            let t = w.traffic.get(&o.id.to_string());
            let storage = w.storage.get(&o.id);
            let serve = cost_to_serve(cost_usd, storage, t, &w.basis);
            let measured = w.basis.apps_measured;
            FleetRow {
                org_id: o.id,
                org_name: o.name,
                org_slug: o.slug,
                cost_usd,
                run_count,
                unpriced_runs,
                input_tokens: acc.map(|a| a.input_tokens).unwrap_or(0),
                output_tokens: acc.map(|a| a.output_tokens).unwrap_or(0),
                // Priced dollars over PRICED runs: `cost_usd` excludes the
                // unpriced ones, so counting them in the denominator reports
                // an org on an unknown model as cheaper per run than it is.
                cost_per_run_usd: super::derive::per_unit(
                    cost_usd,
                    (run_count - unpriced_runs).max(0) as u64,
                ),
                monthly_run_rate_usd: super::derive::monthly_run_rate(
                    cost_usd,
                    f.days.max(0) as u32,
                ),
                unattended_run_share: acc.and_then(|a| {
                    super::derive::share(a.unattended_runs as f64, a.run_count as f64)
                }),
                cache_hit_share: acc.and_then(|a| {
                    super::derive::cache_hit_share(a.input_tokens, a.cache_creation, a.cache_read)
                }),
                // `None` when unmeasured; `Some(0)` when measured and quiet.
                requests: t.map(|t| t.requests).or(zero_if(measured, 0)),
                sessions: t.map(|t| t.sessions).or(zero_if(measured, 0)),
                people: t.map(|t| t.people).or(zero_if(measured, 0)),
                apps: t.map(|t| t.apps).or(zero_if(measured, 0)),
                p95_ms: t.map(|t| t.p95_ms),
                failure_rate: t.and_then(|t| super::derive::failure_rate(t.failed, t.requests)),
                // Exact summed wall time, the same figure the Resources panel
                // meters, so the two cannot disagree about how busy an org was.
                function_compute_gbs: t
                    .and_then(|t| rates::gb_seconds(t.wall_ms as f64, f.provisioned_mb)),
                egress_gb: t.map(|t| t.bytes as f64 / BYTES_PER_GB_DECIMAL),
                // `None` means no readings, which is not an empty bucket: the
                // sweeper only appends a sample when its walk completed.
                storage_gib_month: storage
                    .filter(|s| s.apps_with_samples > 0)
                    .map(|s| rates::gib_months(s.gib, w.basis.days)),
                storage_usd: serve.storage_usd,
                egress_usd: serve.egress_usd,
                compute_usd: serve.compute_usd,
                cost_to_serve_usd: serve.total(),
                // An org missing from a prior window that *was* read spent
                // nothing then: zero, which is what makes it new.
                prior_cost_to_serve_usd: f.prior.map(|p| p.get(&o.id).copied().unwrap_or(0.0)),
            }
        })
        .collect()
}

/// `Some(zero)` when the meter is measured, `None` when it is not.
///
/// The whole point of the `Option` on these columns: an org absent from a
/// measured rollup served nothing (a zero), while an unmeasured deployment has
/// nothing to say (absent). Collapsing the two reports an unmeasured org as an
/// idle one, which is the mistake `ObservabilityStore`'s doc comments name
/// three separate times.
fn zero_if<T>(measured: bool, zero: T) -> Option<T> {
    measured.then_some(zero)
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod tests;
