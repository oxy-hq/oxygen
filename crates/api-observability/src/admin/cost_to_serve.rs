//! Cost to serve: what one org cost over a window, by component, and the rules
//! for comparing it against the window before.
//!
//! LLM spend plus the resources this surface can price — app storage and egress
//! at list (direct), and function compute only as a share of an
//! operator-supplied fleet bill (apportioned). Every component is the Resources
//! panel's own arithmetic, from `rates`, so the fleet list and the org console
//! cannot price one org two ways. Unmeasured components are absent, never zero.
//!
//! Pure apart from [`retention_horizon_days`], which reads boot config: the
//! fleet handler reads, this module decides, and the handler serializes.

use oxy_observability::types::OrgAppTotals;
use oxy_usage_report::llm_usage::price;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use uuid::Uuid;

use super::{cost_rollup, rates, storage_rollup};

pub(super) const BYTES_PER_GB_DECIMAL: f64 = 1_000_000_000.0;

/// What one window's cost to serve is priced against.
#[derive(Debug, Clone, Copy)]
pub(super) struct Basis {
    pub(super) days: u32,
    pub(super) apps_measured: bool,
    /// False when the storage read failed: storage is left out of every total
    /// rather than counted as zero.
    pub(super) include_storage: bool,
    /// The whole fleet's wall time, the apportioning denominator.
    pub(super) fleet_wall_ms: Option<f64>,
    /// The operator-supplied monthly serving bill.
    pub(super) fleet_bill: Option<f64>,
}

impl Basis {
    /// Whether this window's totals include compute. A bill alone is not
    /// enough: a fleet that ran nothing apportions nothing (`rates::apportion`
    /// declines a zero denominator), so its compute is unpriced, not zero.
    pub(super) fn compute_priced(&self) -> bool {
        self.fleet_bill.is_some() && self.fleet_wall_ms.is_some_and(|w| w > 0.0)
    }
}

/// One window's reads, already fetched.
pub(super) struct Window<'a> {
    pub(super) cost_rows: &'a [cost_rollup::FleetOrgRow],
    pub(super) traffic: &'a HashMap<String, OrgAppTotals>,
    pub(super) storage: &'a HashMap<Uuid, storage_rollup::OrgStorage>,
    pub(super) basis: Basis,
}

/// One org's cost to serve, by component.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct CostToServe {
    pub(super) llm_usd: f64,
    pub(super) storage_usd: Option<f64>,
    pub(super) egress_usd: Option<f64>,
    pub(super) compute_usd: Option<f64>,
}

impl CostToServe {
    /// LLM plus every component that could be priced. An unpriced component
    /// adds nothing, and the response says which ones those are.
    pub(super) fn total(&self) -> f64 {
        self.llm_usd
            + self.storage_usd.unwrap_or(0.0)
            + self.egress_usd.unwrap_or(0.0)
            + self.compute_usd.unwrap_or(0.0)
    }
}

/// Price one org's resources for a window.
///
/// An org absent from a **measured** traffic rollup served nothing, so its
/// egress and compute share are zero; on an unmeasured deployment they are
/// absent. Storage with no readings is absent, never zero.
pub(super) fn cost_to_serve(
    llm_usd: f64,
    storage: Option<&storage_rollup::OrgStorage>,
    traffic: Option<&OrgAppTotals>,
    b: &Basis,
) -> CostToServe {
    let storage_usd = storage
        .filter(|s| b.include_storage && s.apps_with_samples > 0)
        .map(|s| rates::storage_usd(rates::gib_months(s.gib, b.days)));
    let (bytes, wall_ms) = match traffic {
        Some(t) => (Some(t.bytes as f64), Some(t.wall_ms as f64)),
        None if b.apps_measured => (Some(0.0), Some(0.0)),
        None => (None, None),
    };
    CostToServe {
        llm_usd,
        storage_usd,
        egress_usd: bytes.map(|bytes| rates::egress_usd(bytes / BYTES_PER_GB_DECIMAL)),
        compute_usd: wall_ms
            .and_then(|w| rates::compute_usd(w, b.fleet_wall_ms, b.fleet_bill, b.days)),
    }
}

/// One org's priced tokens, folded from its per-model rows.
#[derive(Default)]
pub(super) struct Acc {
    pub(super) cost_usd: f64,
    pub(super) run_count: i64,
    pub(super) unpriced_runs: i64,
    pub(super) unattended_runs: i64,
    pub(super) input_tokens: i64,
    pub(super) output_tokens: i64,
    pub(super) cache_creation: i64,
    pub(super) cache_read: i64,
}

/// Fold the per-(org, model) cost rows into one accumulator per org.
///
/// Each bucket is priced at **its own** rate before anything is summed:
/// pricing a summed token count would charge every token at one model's rate,
/// and be wrong by whatever the model mix is.
pub(super) fn accumulate_cost(rows: &[cost_rollup::FleetOrgRow]) -> BTreeMap<Uuid, Acc> {
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

/// Every listed org's total cost to serve for one window.
fn cost_to_serve_by_org(org_ids: &[Uuid], w: &Window<'_>) -> HashMap<Uuid, f64> {
    let by_org = accumulate_cost(w.cost_rows);
    org_ids
        .iter()
        .map(|id| {
            let llm = by_org.get(id).map(|a| a.cost_usd).unwrap_or(0.0);
            let c = cost_to_serve(
                llm,
                w.storage.get(id),
                w.traffic.get(&id.to_string()),
                &w.basis,
            );
            (*id, c.total())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The prior window
// ---------------------------------------------------------------------------

/// How many days back the reads behind a cost to serve still hold data: the
/// shorter of run retention (`OXY_RUN_RETENTION_DAYS`, which deletes finished
/// runs and their events) and, when app traffic is part of the total, the
/// observability store's TTL. `None` when nothing is pruned.
pub(super) fn retention_horizon_days(include_app_traffic: bool) -> Option<u32> {
    let runs = oxy_app_core::config::BootConfig::current()
        .run_retention
        .days();
    let traffic = include_app_traffic.then_some(oxy_observability::RETENTION_DAYS);
    horizon(runs, traffic)
}

/// The shorter of two retention windows, where `None` keeps everything and a
/// zero is the observability store's "no TTL".
fn horizon(runs: Option<u32>, traffic: Option<u32>) -> Option<u32> {
    [runs, traffic.filter(|d| *d > 0)]
        .into_iter()
        .flatten()
        .min()
}

/// Whether the `days` before a `days`-long window are still retained.
///
/// The comparison reads `(now − 2·days, now − days]`. Past the horizon those
/// reads succeed and return almost nothing, so every org would read as new or
/// as having jumped: expired data shown as change. No comparison is the honest
/// answer.
pub(super) fn prior_retained(days: u32, horizon: Option<u32>) -> bool {
    horizon.is_none_or(|h| days.saturating_mul(2) <= h)
}

/// Why there is no comparison with the window before. Only one of these is a
/// fault, so the page gives each its own sentence: telling an operator a read
/// failed when it did not sends them looking for an outage that never happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorMissing {
    /// The window before reaches past retention: its runs and events are
    /// deleted, so it was not read.
    Retention,
    /// A read of the window before failed.
    ReadFailed,
    /// Every read came back, but the two windows priced different components:
    /// compute was priced in this window and not in the one before.
    Incomparable,
}

/// The prior window's reads, all of which came back.
#[derive(Default)]
pub(super) struct PriorReads {
    pub(super) cost_rows: Vec<cost_rollup::FleetOrgRow>,
    pub(super) storage: HashMap<Uuid, storage_rollup::OrgStorage>,
    /// Empty on a deployment with no observability store; the basis's
    /// `apps_measured` says which.
    pub(super) traffic: HashMap<String, OrgAppTotals>,
    pub(super) fleet_wall_ms: Option<f64>,
}

/// One window's traffic read: per-org totals and the fleet's wall time, or
/// `None` when the deployment has no observability store.
pub(super) type TrafficRead = Option<(HashMap<String, OrgAppTotals>, Option<f64>)>;

/// The prior window's three reads, or nothing if any of them failed.
///
/// A comparison built from two of three components would compare different
/// sums, so one failed read drops the comparison rather than inventing a part.
pub(super) fn gather_prior<A: Display, B: Display, C: Display>(
    cost: Result<Vec<cost_rollup::FleetOrgRow>, A>,
    storage: Result<HashMap<Uuid, storage_rollup::OrgStorage>, B>,
    traffic: Result<TrafficRead, C>,
) -> Option<PriorReads> {
    match (cost, storage, traffic) {
        (Ok(cost_rows), Ok(storage), Ok(traffic)) => {
            let (traffic, fleet_wall_ms) = traffic.unwrap_or_default();
            Some(PriorReads {
                cost_rows,
                storage,
                traffic,
                fleet_wall_ms,
            })
        }
        (cost, storage, traffic) => {
            tracing::warn!(
                cost = %err_or_ok(&cost),
                storage = %err_or_ok(&storage),
                traffic = %err_or_ok(&traffic),
                "fleet prior-window read failed; comparison dropped"
            );
            None
        }
    }
}

fn err_or_ok<T, E: Display>(r: &Result<T, E>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(e) => e.to_string(),
    }
}

/// Each org's prior-window total, priced from the same components as the
/// current window's — or `None` when the two cannot be compared.
///
/// - Storage is in the prior total exactly when it is in the current one: if
///   storage could not be read now, it is left out then, or the change would
///   be a change in what is counted.
/// - Compute must be in both totals or in neither, judged by whether each
///   window *actually priced* it (a bill and a non-zero fleet wall time), not
///   by whether its denominator read returned. Priced now and not then would
///   read as compute appearing from nowhere, so there is no comparison;
///   priced then and not now leaves it out of the prior total too.
pub(super) fn prior_totals(
    org_ids: &[Uuid],
    current: &Basis,
    prior: &PriorReads,
) -> Option<HashMap<Uuid, f64>> {
    let then = Basis {
        fleet_wall_ms: prior.fleet_wall_ms,
        ..*current
    };
    let fleet_wall_ms = match (current.compute_priced(), then.compute_priced()) {
        (true, true) => prior.fleet_wall_ms,
        (true, false) => return None,
        (false, _) => None,
    };
    Some(cost_to_serve_by_org(
        org_ids,
        &Window {
            cost_rows: &prior.cost_rows,
            traffic: &prior.traffic,
            storage: &prior.storage,
            basis: Basis {
                fleet_wall_ms,
                ..*current
            },
        },
    ))
}

/// Each org's prior-window total, or why there is none: the reads' own
/// outcome, then whether what they read can be compared with this window.
pub(super) fn compare(
    org_ids: &[Uuid],
    current: &Basis,
    prior: Result<PriorReads, PriorMissing>,
) -> Result<HashMap<Uuid, f64>, PriorMissing> {
    prior.and_then(|p| prior_totals(org_ids, current, &p).ok_or(PriorMissing::Incomparable))
}

#[cfg(test)]
#[path = "cost_to_serve_tests.rs"]
mod tests;
