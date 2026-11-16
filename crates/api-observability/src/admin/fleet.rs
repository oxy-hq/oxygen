//! `GET /admin/observability/fleet` — one row per org: spend, app traffic and
//! the latency tail, so the surface answers *"what is out there?"* before
//! *"which one thing?"*.
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

use axum::extract::{Query, State};
use axum::response::Response;
use axum::{Json, Router, routing::get};
use oxy_app::surface::admin_db::db_err;
use oxy_app_core::AppState;
use oxy_app_core::llm_usage::price;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;

use super::cost_rollup;
use super::{WindowQuery, connect_fleet, rates, scale, storage_rollup};

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
    // Resource meters, for the scale view
    /// Provisioned GB-seconds of function compute. `None` when unmeasured, or
    /// when the isolate ceiling is disabled and there is no basis to convert
    /// wall time against.
    pub function_compute_gbs: Option<f64>,
    /// Egress in decimal GB — the unit it is billed in, unlike storage.
    pub egress_gb: Option<f64>,
    /// Time-weighted storage over the window. `None` means **no readings**,
    /// not an empty bucket.
    pub storage_gib_month: Option<f64>,
}

/// The typical org, per dimension — the denominator the scale view indexes
/// against.
///
/// The **median**, not the mean: with one dominant tenant the mean *is* that
/// tenant, every other org indexes to nearly zero, and the comparison says
/// nothing. `None` for a dimension nothing could answer.
#[derive(Debug, Default, Serialize)]
pub struct FleetMedians {
    pub cost_usd: Option<f64>,
    pub run_count: Option<f64>,
    pub requests: Option<f64>,
    pub function_compute_gbs: Option<f64>,
    pub egress_gb: Option<f64>,
    pub storage_gib_month: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Fleet {
    pub window_days: u32,
    /// False when no observability store is configured, so every row's app
    /// fields are absent for want of a source.
    pub apps_measured: bool,
    /// The typical org per dimension, for the scale view.
    pub medians: FleetMedians,
    /// False when the storage read itself failed. Distinct from an org with no
    /// readings: one is a fault, the other is the sweeper not having reached
    /// an app yet, and the page must not show a database error as the latter.
    pub storage_read: bool,
    /// What a compute figure assumes, so the scale view can say so.
    pub provisioned_memory_mb: Option<f64>,
    pub rows: Vec<FleetRow>,
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/observability/fleet", get(fleet))
}

/// Every org the caller may see, named, so an org with no activity still has a
/// row. The fence is the caller's bounded grant, applied the same way the cost
/// rollup applies it.
#[derive(Debug, FromQueryResult)]
struct OrgRow {
    id: Uuid,
    name: String,
    slug: String,
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

async fn fleet(
    State(state): State<AppState>,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(q): Query<WindowQuery>,
) -> Result<Json<Fleet>, Response> {
    let days = q.days();
    let (db, scope) = connect_fleet(&actor).await?;

    // A bounded grant that reaches no org sees an empty list, and no statement
    // runs: an empty `IN ()` would either fail to parse or, read the other way,
    // match every org — which is the opposite of what the fence is for.
    if scope.as_deref().is_some_and(<[Uuid]>::is_empty) {
        return Ok(Json(Fleet {
            window_days: days as u32,
            apps_measured: state.observability.is_some(),
            medians: FleetMedians::default(),
            storage_read: true,
            provisioned_memory_mb: rates::provisioned_memory_mb(),
            rows: Vec::new(),
        }));
    }

    let orgs = visible_orgs(&db, scope.as_deref()).await.map_err(db_err)?;
    let cost_rows = cost_rollup::fetch_fleet(&db, days, scope.as_deref())
        .await
        .map_err(db_err)?;

    // Storage, batched: two statements over the whole fleet rather than the
    // per-org meter's two each. A failure here must not blank the page — the
    // other columns are independent — but it must not pass for "the sweeper
    // is behind" either, so it is logged and the response says `storage_read`
    // failed.
    let (start, end) = {
        let end = chrono::Utc::now();
        (
            (end - chrono::Duration::days(i64::from(days))).into(),
            end.into(),
        )
    };
    let org_ids: Vec<Uuid> = orgs.iter().map(|o| o.id).collect();
    let (storage, storage_read) =
        match storage_rollup::fleet_storage(&db, &org_ids, start, end).await {
            Ok(s) => (s, true),
            Err(e) => {
                tracing::warn!(error = %e, orgs = org_ids.len(), "fleet storage rollup failed");
                (Default::default(), false)
            }
        };

    // App performance, batched over every visible org in one statement.
    let apps_measured = state.observability.is_some();
    let traffic = match state.observability.as_ref() {
        None => Default::default(),
        Some(store) => {
            let ids: Vec<String> = orgs.iter().map(|o| o.id.to_string()).collect();
            store
                .get_fleet_org_traffic(&ids, days.max(0) as u32)
                .await
                .map_err(super::store_err)?
        }
    };

    let provisioned_mb = rates::provisioned_memory_mb();
    let rows = fold_fleet(FleetFold {
        orgs,
        cost_rows: &cost_rows,
        traffic: &traffic,
        storage: &storage,
        apps_measured,
        provisioned_mb,
        days,
    });

    let medians = medians_of(&rows);

    Ok(Json(Fleet {
        window_days: days as u32,
        apps_measured,
        medians,
        storage_read,
        provisioned_memory_mb: provisioned_mb,
        rows,
    }))
}

/// The typical org per dimension.
///
/// Orgs that could not be measured on a dimension are **excluded from that
/// dimension's sample**, rather than counted as zero. Three measured orgs and
/// three unmeasured ones would otherwise produce a median of zero, and every
/// measured org would then index against nothing.
pub(super) fn medians_of(rows: &[FleetRow]) -> FleetMedians {
    fn med<T, F>(rows: &[T], f: F) -> Option<f64>
    where
        F: Fn(&T) -> Option<f64>,
    {
        let sample: Vec<f64> = rows.iter().filter_map(f).collect();
        scale::median(&sample)
    }
    FleetMedians {
        cost_usd: med(rows, |r| Some(r.cost_usd)),
        run_count: med(rows, |r| Some(r.run_count as f64)),
        requests: med(rows, |r| r.requests.map(|v| v as f64)),
        function_compute_gbs: med(rows, |r| r.function_compute_gbs),
        egress_gb: med(rows, |r| r.egress_gb),
        storage_gib_month: med(rows, |r| r.storage_gib_month),
    }
}

/// Everything the per-org fold needs. A struct rather than seven arguments,
/// and the reason the fold is not in the handler at all: pricing and
/// aggregation are business logic, and `backend-architecture.md` keeps HTTP
/// handlers to parse → call → serialize. `cost.rs` already splits this way
/// with `fold_days` / `fold_buckets`; this follows it.
pub(super) struct FleetFold<'a> {
    pub(super) orgs: Vec<OrgRow>,
    pub(super) cost_rows: &'a [cost_rollup::FleetOrgRow],
    pub(super) traffic:
        &'a std::collections::HashMap<String, oxy_observability::types::OrgAppTotals>,
    pub(super) storage: &'a std::collections::HashMap<Uuid, storage_rollup::OrgStorage>,
    pub(super) apps_measured: bool,
    pub(super) provisioned_mb: Option<f64>,
    pub(super) days: i32,
}

/// One org's priced tokens, folded from its per-model rows.
#[derive(Default)]
struct Acc {
    cost_usd: f64,
    run_count: i64,
    unpriced_runs: i64,
    unattended_runs: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation: i64,
    cache_read: i64,
}

/// Fold the per-(org, model) cost rows into one accumulator per org.
///
/// Each bucket is priced at **its own** rate before anything is summed:
/// pricing a summed token count would charge every token at one model's rate,
/// and be wrong by whatever the model mix is.
fn accumulate_cost(rows: &[cost_rollup::FleetOrgRow]) -> BTreeMap<Uuid, Acc> {
    let mut by_org: BTreeMap<Uuid, Acc> = BTreeMap::new();
    for r in rows {
        let acc = by_org.entry(r.org_id).or_default();
        match price(
            &r.model,
            r.input_tokens,
            r.output_tokens,
            r.cache_creation,
            r.cache_read,
        ) {
            Some(c) => acc.cost_usd += c,
            None => acc.unpriced_runs += r.run_count,
        }
        acc.run_count += r.run_count;
        acc.unattended_runs += r.unattended_runs;
        acc.input_tokens += r.input_tokens;
        acc.output_tokens += r.output_tokens;
        acc.cache_creation += r.cache_creation;
        acc.cache_read += r.cache_read;
    }
    by_org
}

/// Build the fleet's rows. Pure: every input is already read.
pub(super) fn fold_fleet(f: FleetFold<'_>) -> Vec<FleetRow> {
    let by_org = accumulate_cost(f.cost_rows);
    f.orgs
        .into_iter()
        .map(|o| {
            let acc = by_org.get(&o.id);
            let cost_usd = acc.map(|a| a.cost_usd).unwrap_or(0.0);
            let run_count = acc.map(|a| a.run_count).unwrap_or(0);
            let unpriced_runs = acc.map(|a| a.unpriced_runs).unwrap_or(0);
            let t = f.traffic.get(&o.id.to_string());
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
                requests: t.map(|t| t.requests).or(zero_if(f.apps_measured, 0)),
                sessions: t.map(|t| t.sessions).or(zero_if(f.apps_measured, 0)),
                people: t.map(|t| t.people).or(zero_if(f.apps_measured, 0)),
                apps: t.map(|t| t.apps).or(zero_if(f.apps_measured, 0)),
                p95_ms: t.map(|t| t.p95_ms),
                failure_rate: t.and_then(|t| super::derive::failure_rate(t.failed, t.requests)),
                // Exact summed wall time, the same figure the Resources panel
                // meters, so the two cannot disagree about how busy an org was.
                function_compute_gbs: t
                    .and_then(|t| rates::gb_seconds(t.wall_ms as f64, f.provisioned_mb)),
                egress_gb: t.map(|t| t.bytes as f64 / 1_000_000_000.0),
                // `None` means no readings, which is not an empty bucket: the
                // sweeper only appends a sample when its walk completed.
                storage_gib_month: f
                    .storage
                    .get(&o.id)
                    .filter(|s| s.apps_with_samples > 0)
                    .map(|s| s.gib * (f64::from(f.days.max(0) as u32) / 30.0)),
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
mod tests {
    use super::*;

    /// A measured-but-quiet org reports zero; an unmeasured deployment reports
    /// absent. These are different answers and the column header says which.
    #[test]
    fn measured_and_quiet_is_zero_while_unmeasured_is_absent() {
        assert_eq!(zero_if(true, 0u64), Some(0));
        assert_eq!(zero_if(false, 0u64), None);
    }

    /// A bounded grant binds each org it may see. The org list is the page's
    /// spine — every row comes from it — so a fence that leaked here would
    /// list orgs the cost rollup then refuses to populate, and the page would
    /// read "these orgs spent nothing" about tenants the caller may not see.
    #[test]
    fn the_org_list_binds_each_in_scope_org() {
        let mut values: Vec<sea_orm::Value> = Vec::new();
        let sql = visible_orgs_sql(Some(&[Uuid::nil(), Uuid::from_u128(3)]), &mut values);
        assert!(
            sql.contains("WHERE id IN ($1,$2)"),
            "the list takes no window param, so orgs start at $1: {sql}"
        );
        assert_eq!(values.len(), 2, "one bound value per org");
    }

    /// An unbounded reader gets no `WHERE` at all. `IN ()` would list no orgs,
    /// so the whole surface would render as an empty platform.
    #[test]
    fn an_unbounded_reader_lists_every_org() {
        let sql = visible_orgs_sql(None, &mut Vec::new());
        assert!(!sql.contains("WHERE"), "no fence for an owner: {sql}");
        assert!(sql.contains("ORDER BY name"), "{sql}");
    }

    fn row(cost: f64, requests: Option<u64>, storage: Option<f64>) -> FleetRow {
        FleetRow {
            org_id: Uuid::nil(),
            org_name: String::new(),
            org_slug: String::new(),
            cost_usd: cost,
            run_count: 0,
            unpriced_runs: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_per_run_usd: None,
            monthly_run_rate_usd: None,
            unattended_run_share: None,
            cache_hit_share: None,
            requests,
            sessions: None,
            people: None,
            apps: None,
            p95_ms: None,
            failure_rate: None,
            function_compute_gbs: None,
            egress_gb: None,
            storage_gib_month: storage,
        }
    }

    /// An org that could not be measured on a dimension is excluded from that
    /// dimension's sample, not counted as zero.
    ///
    /// Three measured orgs beside three unmeasured ones would otherwise give a
    /// median of zero, and every measured org would index against nothing —
    /// the scale view would show six orgs all infinitely above typical.
    #[test]
    fn an_unmeasured_org_is_excluded_from_that_dimension_not_zeroed() {
        let rows = [
            row(10.0, Some(100), Some(4.0)),
            row(20.0, None, None),
            row(30.0, Some(300), Some(8.0)),
            row(40.0, None, None),
        ];
        let m = medians_of(&rows);
        // Two measured values, 100 and 300 → 200. Counting the absences as
        // zero would give 50.
        assert_eq!(m.requests, Some(200.0));
        assert_eq!(m.storage_gib_month, Some(6.0));
        // Cost is answerable for every org, so all four count.
        assert_eq!(m.cost_usd, Some(25.0));
    }

    /// A dimension nothing could answer has no median at all, so the view says
    /// there is no typical account rather than indexing against zero.
    #[test]
    fn a_dimension_nothing_can_answer_has_no_median() {
        let rows = [row(1.0, None, None), row(2.0, None, None)];
        let m = medians_of(&rows);
        assert_eq!(m.requests, None);
        assert_eq!(m.storage_gib_month, None);
        assert_eq!(m.function_compute_gbs, None);
    }

    /// The list is ordered by name, not by cost. The *table* sorts by spend —
    /// that is the frontend's job and it is interactive — but the API's order
    /// must not depend on data, or the same request returns rows in a
    /// different order as spend moves, which breaks pagination the day it is
    /// added.
    #[test]
    fn the_api_orders_by_name_not_by_spend() {
        let sql = visible_orgs_sql(None, &mut Vec::new());
        assert!(sql.ends_with("ORDER BY name"), "{sql}");
    }
}
