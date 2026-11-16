//! `GET /admin/observability/orgs/{org_id}/resources` — what this org
//! consumes, meter by meter, and how much of each figure we can stand behind.
//!
//! The Cost panel answers "what did the LLM cost". This answers the wider
//! question, and the reason it is a separate panel is that **the meters are not
//! equally trustworthy**, and a page that hid that would be worse than one that
//! showed less.
//!
//! So every meter carries an [`Attribution`]:
//!
//! - **Direct** — the org is the only consumer and the rate is public. Storage
//!   and egress. Wrong only by the discount we actually get.
//! - **Apportioned** — a shared bill split by a measured driver. Function
//!   compute. Honest arithmetic over an operator-supplied total, and absent
//!   entirely when no total was supplied: this module never invents the bill.
//! - **Unmeasured** — we know the org consumes it and we cannot say how much.
//!   Named, with what it would take, rather than left off the page.
//!
//! The last class is the point. `internal-docs/2026-11-16-admin-observability-design.md`
//! rejected a rate card that smeared the whole shared bill across orgs; it did
//! not reject *metering*, and the storage meter in `oxy-custom-apps` — whose
//! own doc calls GB-months "the billable quantity" — had been sitting unused by
//! any cost surface. This panel is that correction.

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Json, Router, routing::get};
use chrono::{DateTime, FixedOffset, Utc};
use oxy_app::surface::admin_db::db_err;
use oxy_app_core::AppState;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::FromQueryResult;
use serde::Serialize;
use uuid::Uuid;

use super::meters::build_meters;
use super::{WindowQuery, connect_scoped, rates, storage_rollup};

/// How far we can stand behind a meter's figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    /// This org is the only consumer, and the rate is public.
    Direct,
    /// A share of a bill many orgs cause, split by a measured driver.
    Apportioned,
    /// Consumed, and not measurable here yet.
    Unmeasured,
}

/// One resource the org consumes.
#[derive(Debug, Serialize)]
pub struct Meter {
    /// Stable key; the frontend orders and labels from it.
    pub key: &'static str,
    pub label: &'static str,
    /// `None` when unmeasured — never zero, which would read as "uses none".
    pub quantity: Option<f64>,
    pub unit: &'static str,
    /// `None` when unmeasured, or measured with no rate to price it.
    pub cost_usd: Option<f64>,
    pub attribution: Attribution,
    /// What the figure rests on, or what it would take to have one. Always
    /// present for anything not `Direct` — an apportioned or missing number
    /// without its caveat is the thing this panel exists to avoid.
    pub note: Option<String>,
}

/// What every figure on the panel assumes.
#[derive(Debug, Serialize)]
pub struct Basis {
    /// The per-isolate ceiling GB-seconds are computed at, in MiB. `None` when
    /// the runtime's ceiling is disabled, in which case compute reports
    /// wall-seconds only.
    pub provisioned_memory_mb: Option<f64>,
    /// The serving fleet's monthly cost, if an operator supplied one.
    pub fleet_cost_usd_month: Option<f64>,
    /// The env var that supplies it, so the page can say how to fix an absence.
    pub fleet_cost_env: &'static str,
    pub storage_usd_per_gb_month: f64,
    pub egress_usd_per_gb: f64,
}

#[derive(Debug, Serialize)]
pub struct OrgResources {
    pub window_days: u32,
    pub basis: Basis,
    pub meters: Vec<Meter>,
    /// Everything priced above, summed. **Excludes LLM**, which the Cost panel
    /// owns — one number on two panels would be added twice by any reader
    /// comparing them.
    pub infra_cost_usd: Option<f64>,
    /// True when at least one meter could not be measured, so the total is a
    /// floor rather than a figure.
    pub partial: bool,
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/observability/orgs/{org_id}/resources", get(org_resources))
}

/// Airhouse tenants provisioned to this org's workspaces.
///
/// One row per workspace that has one, joined through `workspaces.org_id`.
/// Only `ready` tenants count: a failed or deleting one is not a resource the
/// org holds.
async fn airhouse_tenant_count(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
) -> Result<u64, sea_orm::DbErr> {
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct CountRow {
        n: i64,
    }
    let row = CountRow::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT COUNT(*)::bigint AS n FROM airhouse_tenants t \
         JOIN workspaces w ON t.workspace_id = w.id \
         WHERE w.org_id = $1 AND t.status = 'ready'",
        [org_id.into()],
    ))
    .one(db)
    .await?;
    Ok(row.map(|r| r.n.max(0) as u64).unwrap_or(0))
}

/// The window as absolute bounds, for the storage meter's integral.
fn window_bounds(days: i32) -> (DateTime<FixedOffset>, DateTime<FixedOffset>) {
    let end = Utc::now();
    let start = end - chrono::Duration::days(i64::from(days));
    (start.into(), end.into())
}

/// The fleet's wall time: the denominator an apportioned share needs.
///
/// Unfenced by design (see the store method). The bill is over every tenant, so
/// narrowing the denominator to a bounded reader's scope would overstate their
/// share by exactly the part of the platform they cannot see. A read failure
/// costs the cost column, not the page.
///
/// A function called inline at the one use site, rather than a local in the
/// handler, and that shape is the fix for a real bug: the first pass at this
/// read it, bound it to `fleet_wall_ms`, and then wrote a literal `None` into
/// the struct two lines later. The apportioned class stayed unreachable, the
/// panel went on telling operators to set `OXY_FLEET_COMPUTE_USD_MONTH`, and
/// setting it changed nothing. With no local there is nothing left to drop.
async fn read_fleet_wall_ms(state: &AppState, days: i32) -> Option<f64> {
    let store = state.observability.as_ref()?;
    match store.get_fleet_wall_ms(days.max(0) as u32).await {
        Ok(ms) => Some(ms as f64),
        Err(e) => {
            tracing::warn!(error = %e, "fleet wall-time denominator failed");
            None
        }
    }
}

async fn org_resources(
    State(state): State<AppState>,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Query(q): Query<WindowQuery>,
) -> Result<Json<OrgResources>, Response> {
    let days = q.days();
    let db = connect_scoped(&actor, org_id).await?;
    let (start, end) = window_bounds(days);

    let provisioned_mb = rates::provisioned_memory_mb();
    let fleet_cost = rates::fleet_cost_usd_month();

    // Storage, from the same bounded rollup the fleet column reads.
    //
    // An earlier pass called `metering::meter_org` here, reasoning that a
    // *costed* figure must come from the same computation as Admin → Custom
    // apps' storage number. That was wrong twice. `meter_org` loads raw
    // samples, and the sweeper writes ~96 per app per day against a window
    // that reaches 365, so it read up to ~39k rows per app per request. And it
    // bought no agreement: Custom apps meters month-to-date while this panel
    // meters an operator-chosen 7–365 days, so the periods coincide only by
    // accident — identical arithmetic over different periods is not a matching
    // number.
    //
    // The collapse stays here rather than moving down into `metering`: one
    // reading per day is the right *grade* for this panel, but running the
    // time-weighted integral over daily closing readings would change a billed
    // quantity, and the billing path is not this panel's to re-grade.
    //
    // Absent from the map means the org has no app with a storage row at all;
    // present with no readings means apps but nothing measured. Both reach the
    // meter as "unmeasured" rather than as a measured zero.
    let storage = storage_rollup::fleet_storage(&db, &[org_id], start, end)
        .await
        .map_err(db_err)?
        .remove(&org_id);

    // App traffic: the same ClickHouse aggregate the Performance panel reads,
    // so the two panels cannot report different request counts.
    let traffic = match state.observability.as_ref() {
        None => None,
        Some(store) => Some(
            store
                .get_org_app_traffic(&org_id.to_string(), days.max(0) as u32)
                .await
                .map_err(super::store_err)?,
        ),
    };

    // Degrading is fine; degrading silently is not. Without the log a database
    // fault reads on the page as "this org has no warehouse".
    let airhouse_tenants = match airhouse_tenant_count(&db, org_id).await {
        Ok(n) => Some(n),
        Err(e) => {
            tracing::warn!(error = %e, %org_id, "airhouse tenant count failed");
            None
        }
    };

    let meters = build_meters(MeterInput {
        window_days: days.max(0) as u32,
        storage,
        traffic: traffic.as_ref().map(|t| &t.totals),
        provisioned_mb,
        fleet_cost,
        airhouse_tenants,
        // The org's share of the fleet is its share of measured function
        // wall time. Read inline, for the reason on `read_fleet_wall_ms`.
        fleet_wall_ms: read_fleet_wall_ms(&state, days).await,
    });

    let infra_cost_usd = {
        let priced: Vec<f64> = meters.iter().filter_map(|m| m.cost_usd).collect();
        (!priced.is_empty()).then(|| priced.iter().sum())
    };
    let partial = meters
        .iter()
        .any(|m| m.attribution == Attribution::Unmeasured || m.quantity.is_none());

    Ok(Json(OrgResources {
        window_days: days.max(0) as u32,
        basis: Basis {
            provisioned_memory_mb: provisioned_mb,
            fleet_cost_usd_month: fleet_cost,
            fleet_cost_env: rates::FLEET_COST_ENV,
            storage_usd_per_gb_month: rates::S3_STANDARD_USD_PER_GB_MONTH,
            egress_usd_per_gb: rates::EGRESS_USD_PER_GB,
        },
        meters,
        infra_cost_usd,
        partial,
    }))
}

/// Everything the meter list is built from. A struct rather than eight
/// arguments so the pure builder below can be tested without a request.
pub(super) struct MeterInput<'a> {
    pub(super) window_days: u32,
    pub(super) storage: Option<storage_rollup::OrgStorage>,
    pub(super) traffic: Option<&'a oxy_observability::types::OrgAppTotals>,
    pub(super) provisioned_mb: Option<f64>,
    pub(super) fleet_cost: Option<f64>,
    /// Fleet-wide function wall time, the denominator for apportioning the
    /// serving bill. `None` only when there is no observability store or its
    /// read failed; with no denominator compute reports its quantity and says
    /// why it has no cost.
    pub(super) fleet_wall_ms: Option<f64>,
    /// Airhouse tenants provisioned to this org's workspaces. `None` when the
    /// read failed — not zero, which would say the org has no warehouse.
    pub(super) airhouse_tenants: Option<u64>,
}

#[cfg(test)]
mod tests {
    /// The handler's own source, above this module.
    ///
    /// Both tests below check **wiring**: which function the handler calls, and
    /// what it hands on. Neither is reachable from a test on `build_meters`,
    /// because that is handed the input the handler failed to build — which is
    /// exactly how F8 shipped "fixed" while still broken. So they read the
    /// source instead.
    ///
    /// The split matters. Scanning the whole file would let a literal written
    /// in this module satisfy its own `contains` assertion, and a test that
    /// passes because of its own text is worse than no test. `split` on the
    /// first `#[cfg(test)]` is the attribute above this line, so `next()` is
    /// everything before it.
    fn handler_source() -> &'static str {
        include_str!("resources.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a first segment")
    }

    /// F8, twice. Reported as "the env var prices nothing", fixed by adding the
    /// read, and shipped still broken: the value went into a local and a literal
    /// `None` went into the struct two lines on. Every apportioned cost stayed
    /// `None`, and the panel kept telling operators to set an env var that could
    /// not take effect.
    #[test]
    fn the_handler_passes_the_denominator_it_reads() {
        let src = handler_source();
        assert!(
            src.contains("fleet_wall_ms: read_fleet_wall_ms(&state, days).await"),
            "the fleet denominator must flow straight from its read into the meter \
             input; a local in between is what got dropped last time"
        );
        assert!(
            !src.contains("fleet_wall_ms: None"),
            "a literal None here disables the whole apportioned class and makes \
             OXY_FLEET_COMPUTE_USD_MONTH dead config"
        );
    }

    /// F11. `meter_org` loads raw samples over the window plus a 45-day
    /// look-back; the sweeper writes ~96 per app per day and this window reaches
    /// 365, so calling it here read up to ~39k rows per app per request.
    #[test]
    fn the_per_org_storage_read_is_the_bounded_rollup() {
        let src = handler_source();
        assert!(
            src.contains("storage_rollup::fleet_storage(&db, &[org_id], start, end)"),
            "per-org storage must come from the daily-collapsed, look-back-bounded \
             rollup, the same read the fleet column uses"
        );
        assert!(
            !src.contains("meter_org(&db"),
            "meter_org integrates raw samples for the billing path; on a window \
             that reaches 365 days it is an unbounded read, and it never matched \
             Custom apps' month-to-date period anyway"
        );
    }
}
