//! Everything pricing asks of the warehouse, in one call.
//!
//! [`fetch_readings`] is synchronous and pure over a layer and an executor, so
//! the production source and the test fixture run the same code. The membership
//! and the panel are the candidate — their faults fail the call; the lag, spread
//! and prior reads are diagnostics — their faults become `notes`, which become
//! warnings. See `internal-docs/experiments.md`, "Data that arrives late" and
//! "Guardrails".

use chrono::{DateTime, NaiveDate, Utc};
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_experiment::timeline::today_in;
use oxy_experiment::{ExperimentConfig, Grain};
use oxy_shared::errors::OxyError;

use super::candidate::Candidate;
use super::lag::{LagReading, guardrail_measures, lag_warnings, read_back_measures, read_lag};
use super::panel::{
    GroupMembership, HistoryBounds, PanelOutcome, PanelRefusal, PanelRequest, fetch_panel,
    resolve_spec,
};
use super::panel_groups::fetch_groups;
use super::prior::{PriorEdge, PriorReading, read_prior};
use super::spread::{UnitMeans, read_unit_means, spread_warnings, trigger_measures};

/// What the job wants read for one candidate.
#[derive(Debug, Clone)]
pub struct ReadAsk {
    /// The panel to load; `membership` is filled by [`fetch_readings`] under `assign_by`.
    pub panel: PanelRequest,
    pub assign_by: Option<String>,
    /// Read-back and guardrail measures: their arrival lag is wanted.
    pub lag_measures: Vec<String>,
    /// The read-back measures alone: at hour grain each needs a time of day.
    pub read_back_measures: Vec<String>,
    /// Trigger-guardrail measures: their per-unit means are wanted.
    pub spread_measures: Vec<String>,
    /// `outcome.driver` → `outcome.target`, when a driver is named.
    pub prior_edge: Option<PriorEdge>,
    pub today: NaiveDate,
}

/// What came back. `notes` are the diagnostics that could not be read.
pub struct Readings {
    pub panel: PanelOutcome,
    pub membership: Option<GroupMembership>,
    pub lags: Vec<LagReading>,
    pub spreads: Vec<UnitMeans>,
    pub prior: PriorReading,
    pub notes: Vec<String>,
}

impl Readings {
    /// A source that reads nothing but the panel (the default `PanelSource::read`).
    pub fn panel_only(panel: PanelOutcome, note: &str) -> Self {
        Self {
            panel,
            membership: None,
            lags: Vec::new(),
            spreads: Vec::new(),
            prior: PriorReading::NotAsked,
            notes: vec![note.to_string()],
        }
    }
}

/// The reads one candidate of `cfg` needs, as of `now` in `design.timezone`.
pub fn read_ask(cfg: &ExperimentConfig, candidate: &Candidate, now: DateTime<Utc>) -> ReadAsk {
    let today = today_in(cfg.design.timezone, now);
    let read_back = read_back_measures(cfg);
    let mut lag_measures = read_back.clone();
    for m in guardrail_measures(cfg) {
        if !lag_measures.contains(&m) {
            lag_measures.push(m);
        }
    }
    ReadAsk {
        panel: PanelRequest {
            entity: cfg.for_each.clone(),
            measure: cfg.outcome.target.clone(),
            time_dimension: cfg.design.time_dimension.clone(),
            grain: cfg.design.grain,
            timezone: Some(cfg.design.timezone.name().to_string()),
            history: HistoryBounds {
                from: None,
                to_exclusive: today,
            },
            membership: None,
            restrict_to: None,
            unit_floor: None,
            filters: cfg.outcome.target_filter.clone(),
        },
        assign_by: candidate.assign_by().map(str::to_string),
        lag_measures,
        read_back_measures: read_back,
        spread_measures: trigger_measures(cfg),
        prior_edge: cfg.outcome.driver.as_ref().map(|d| PriorEdge {
            driver: d.clone(),
            target: cfg.outcome.target.clone(),
            filtered: is_filtered(cfg),
        }),
        today,
    }
}

/// The design narrows its target or driver: the semantic model's edge between
/// them is between the unfiltered measures.
pub(crate) fn is_filtered(cfg: &ExperimentConfig) -> bool {
    !cfg.outcome.target_filter.is_empty() || !cfg.outcome.driver_filter.is_empty()
}

fn view_of(member: &str) -> &str {
    member.split('.').next().unwrap_or(member)
}

/// `design.time_dimension` belongs to the target's view; another view's measure
/// uses that view's only date dimension (`None` here).
pub(crate) fn time_dimension_for<'a>(design_td: Option<&'a str>, measure: &str) -> Option<&'a str> {
    design_td.filter(|t| view_of(t) == view_of(measure))
}

fn td_for<'a>(measure: &str, ask: &'a ReadAsk) -> Option<&'a str> {
    time_dimension_for(ask.panel.time_dimension.as_deref(), measure)
}

/// Membership, then the panel, then — only if there is a panel — the diagnostics.
pub fn fetch_readings(
    layer: &SemanticLayer,
    executor: &QueryExecutor,
    ask: &ReadAsk,
) -> Result<Readings, OxyError> {
    let membership = match &ask.assign_by {
        Some(by) => Some(fetch_groups(layer, executor, &ask.panel.entity, by)?),
        None => None,
    };
    let request = PanelRequest {
        membership: membership.clone(),
        ..ask.panel.clone()
    };
    let panel = fetch_panel(layer, executor, &request)?;
    let mut readings = Readings {
        panel,
        membership,
        lags: Vec::new(),
        spreads: Vec::new(),
        prior: PriorReading::NotAsked,
        notes: Vec::new(),
    };
    if matches!(readings.panel, PanelOutcome::Panel(_)) {
        diagnostics(layer, executor, ask, &mut readings);
    }
    Ok(readings)
}

/// At hour grain read-back watches the lever hour by hour, so a read-back measure
/// whose calendar is a date (no time of day) could never see a switch inside a day:
/// one note naming each.
pub fn hour_read_back_notes(layer: &SemanticLayer, ask: &ReadAsk) -> Vec<String> {
    if ask.panel.grain != Grain::Hour {
        return Vec::new();
    }
    ask.read_back_measures
        .iter()
        .filter_map(|m| {
            let req = PanelRequest {
                measure: m.clone(),
                time_dimension: td_for(m, ask).map(str::to_string),
                membership: None,
                restrict_to: None,
                unit_floor: None,
                filters: vec![],
                ..ask.panel.clone()
            };
            match resolve_spec(layer, &req) {
                Err(PanelRefusal::HourNeedsDatetime { dimension }) => Some(format!(
                    "at hour grain read-back checks the lever hour by hour, and read-back measure `{m}` is read on `{dimension}`, a date with no time of day: no flip could be confirmed. Read back through a measure on a datetime calendar"
                )),
                _ => None,
            }
        })
        .collect()
}

fn diagnostics(layer: &SemanticLayer, executor: &QueryExecutor, ask: &ReadAsk, out: &mut Readings) {
    out.notes.extend(hour_read_back_notes(layer, ask));
    for m in &ask.lag_measures {
        match read_lag(layer, executor, m, td_for(m, ask), ask.today) {
            Ok(l) => out.lags.push(l),
            Err(why) => out
                .notes
                .push(format!("the arrival lag of `{m}` could not be read: {why}")),
        }
    }
    for m in &ask.spread_measures {
        let req = PanelRequest {
            measure: m.clone(),
            time_dimension: td_for(m, ask).map(str::to_string),
            // A trigger is judged on daily values (`check::guardrails`), whatever the grain.
            grain: Grain::Day,
            timezone: None,
            membership: None,
            ..ask.panel.clone()
        };
        match read_unit_means(layer, executor, &req) {
            Ok(s) => out.spreads.push(s),
            Err(why) => out
                .notes
                .push(format!("the unit means of `{m}` could not be read: {why}")),
        }
    }
    if let Some(edge) = &ask.prior_edge {
        out.prior = match resolve_spec(layer, &ask.panel) {
            Ok(spec) => read_prior(layer, executor, edge, &spec.time_dimension, ask.today),
            Err(e) => PriorReading::Absent {
                why: format!("the target's calendar could not be resolved: {e}"),
            },
        };
    }
}

/// The warnings the readings earn, in a stable order: lag, spread, unread
/// diagnostics, and a missing prior.
pub fn warnings(cfg: &ExperimentConfig, r: &Readings, today: NaiveDate) -> Vec<String> {
    let mut out = lag_warnings(cfg, &r.lags, today);
    out.extend(spread_warnings(&r.spreads));
    out.extend(r.notes.iter().cloned());
    out.extend(r.prior.warning());
    out
}
