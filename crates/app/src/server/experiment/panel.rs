//! The panel query — one semantic query, one [`PanelMatrix`].
//!
//! Everything here is pure except [`load_panel`] (Task 7), which builds a
//! workspace's executor and calls [`fetch_panel`]. The split exists so the
//! empty-result rule and the row → triple mapping are tested without a
//! warehouse: the shared Arrow bridge is known to panic on empty results, so a
//! zero-row answer must be turned into [`PanelOutcome::NoHistory`] before any
//! triple, panel or placebo call exists.

use chrono::NaiveDate;
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT;
use oxy_airlayer_compat::engine::experiment::{DroppedUnit, PanelMatrix};
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::{
    FilterOperator, QueryFilter, QueryRequest, TimeDimensionQuery,
};
use oxy_experiment::{ConfigError, FilterOp, Grain, MeasureFilter};
use oxy_shared::errors::OxyError;
use serde_json::{Map, Value};
use uuid::Uuid;

/// The calendar window the panel covers, `[from, to_exclusive)`.
///
/// `to_exclusive` is today for pricing: today's row is partial, and a partial
/// day enters the panel as an artificially low one.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryBounds {
    pub from: Option<NaiveDate>,
    pub to_exclusive: NaiveDate,
}

/// Which unit belongs to which group, under `assign_by` (index C2a). Group and
/// member names are exactly as the warehouse rendered them; both are sorted.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct GroupMembership(pub std::collections::BTreeMap<String, Vec<String>>);

#[derive(Debug, Clone, PartialEq)]
pub struct PanelRequest {
    /// `for_each` — the entity whose key is the unit.
    pub entity: String,
    /// The measure being read, `<view>.<measure>` — `outcome.target` for pricing.
    pub measure: String,
    /// `design.time_dimension`: the date dimension the calendar runs on
    /// (`<view>.<dimension>`), or `None` to use the measure view's only one.
    pub time_dimension: Option<String>,
    /// `design.grain`: one panel cell per unit per period of it.
    pub grain: Grain,
    /// `design.timezone`: the zone an hour-grain panel is bucketed in. Day, week and
    /// month keep the warehouse's own bucketing, so it is not sent for them.
    pub timezone: Option<String>,
    pub history: HistoryBounds,
    /// `Some` under `assign_by`: the panel becomes one row per **group** per
    /// day, the sum of its members' values. Units absent from the map are
    /// excluded. The measure must be sum- or count-typed.
    pub membership: Option<GroupMembership>,
    /// `Some`: drop every row for a unit outside this list before the panel is
    /// built — used only for a switchback's pinned fleet (PR 4 pins it, PR 5
    /// loads it), never with `membership`; both `Some` is a `Refused` outcome.
    /// Pricing always passes `None`.
    pub restrict_to: Option<Vec<String>>,
    /// `Some(floor)`: a unit holding fewer than `floor` of the days any unit
    /// holds is dropped (and listed) instead of every unit losing the days it
    /// lacks. Set for a reading of past data; `None` keeps the dense rule.
    pub unit_floor: Option<f64>,
    /// Rows of the measure's own view to keep (`outcome.target_filter` and the
    /// like); empty = the whole measure. Every filter applies.
    pub filters: Vec<MeasureFilter>,
}

/// Whose workspace a panel is loaded from, at which revision, on whose behalf.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelContext {
    pub workspace_id: Uuid,
    /// The revision the caller named; the promoted one must match.
    pub revision_id: Uuid,
    pub requested_by: Uuid,
}

/// What the warehouse held. A refusal to build a panel is a value, not an error.
#[derive(Debug)]
pub enum PanelOutcome {
    Panel(PanelMatrix),
    /// A panel built with `unit_floor`, and the units it left out (each with why).
    Thinned {
        panel: PanelMatrix,
        dropped: Vec<DroppedUnit>,
    },
    NoHistory {
        measure: String,
        entity: String,
    },
    /// The panel cannot be built for a reason a person can act on, returned as a
    /// VALUE so a queued job settles instead of retrying (index C2a): today, a
    /// non-sum/count measure under `assign_by`.
    Refused {
        reason: String,
    },
}

/// The three members the query is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelSpec {
    pub entity: String,
    pub measure: String,
    pub key_dimension: String,
    pub time_dimension: String,
    pub grain: Grain,
    pub timezone: Option<String>,
    pub filters: Vec<MeasureFilter>,
}

impl PanelSpec {
    fn alias(member: &str) -> String {
        member.replace('.', "__")
    }
    pub(super) fn key_alias(&self) -> String {
        Self::alias(&self.key_dimension)
    }
    pub(super) fn measure_alias(&self) -> String {
        Self::alias(&self.measure)
    }
    /// A granular time dimension is projected under `<dimension>.<granularity>`.
    pub(super) fn period_alias(&self) -> String {
        format!(
            "{}__{}",
            Self::alias(&self.time_dimension),
            self.grain.as_str()
        )
    }
}

/// One variant per guard; each message states only what that guard tested.
///
/// The first group is a refusal a person can act on (→ `OxyError::ValidationError`);
/// the second is an infrastructure fault (→ `OxyError::RuntimeError`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PanelRefusal {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(
        "the entity `{0}` is not a primary entity of any view in the workspace's semantic model"
    )]
    UnknownEntity(String),
    #[error("the entity `{entity}` is a primary entity of {} views ({}); v1 needs exactly one", .views.len(), .views.join(", "))]
    AmbiguousEntity { entity: String, views: Vec<String> },
    #[error("the entity `{0}` is a primary entity that declares no key")]
    NoKey(String),
    #[error(
        "the measure `{0}` is not `<view>.<measure>` naming a measure the semantic model declares"
    )]
    UnknownMeasure(String),
    #[error(
        "the filter names `{dimension}`, which is not a dimension of `{view}`, the view its measure is read from"
    )]
    UnknownFilterDimension { dimension: String, view: String },
    #[error(
        "the measure `{measure}` is read from `{view}`, which declares no date or datetime dimension, so there is no calendar to run its panel on"
    )]
    NoTimeDimension { measure: String, view: String },
    #[error("`design.time_dimension` is absent and `{view}`, the view the measure `{measure}` is read from, declares {} date/datetime dimensions [{}]; name one so the panel's calendar is not a guess", .found.len(), .found.join(", "))]
    TimeDimensionAmbiguous {
        measure: String,
        view: String,
        found: Vec<String>,
    },
    #[error("the time dimension `{given}` is not a date or datetime dimension of `{view}`, the view the measure `{measure}` is read from; its date dimensions are [{}]", .found.join(", "))]
    TimeDimensionNotADate {
        measure: String,
        view: String,
        given: String,
        found: Vec<String>,
    },
    #[error(
        "the panel query returned {rows} rows for {measure} over {entity}, but none had a key, a day and a numeric value ({skipped_key} without a usable key, {skipped_day} without a parseable day, {skipped_value} without a numeric value)"
    )]
    NoUsableRows {
        measure: String,
        entity: String,
        rows: usize,
        skipped_key: usize,
        skipped_day: usize,
        skipped_value: usize,
    },
    #[error(
        "hour grain reads hourly values, and `{dimension}` is a date dimension with no time of day; name a datetime dimension of the outcome's view as the calendar, or use day grain"
    )]
    HourNeedsDatetime { dimension: String },
    /// Weeks are ISO weeks (Monday first), and the design's dates are Mondays; a
    /// warehouse whose weeks start on another day would mix the days either side
    /// of a switch into one cell.
    #[error(
        "the warehouse returned a {grain} of {measure} starting {date} ({weekday}); at {grain} grain every period must start on {expected}, so this measure cannot be read at {grain} grain here. Use a different grain",
        grain = .grain.as_str(),
        expected = if *.grain == Grain::Week { "a Monday" } else { "the 1st of a month" }
    )]
    PeriodNotAligned {
        measure: String,
        grain: Grain,
        date: NaiveDate,
        weekday: String,
    },
    #[error(
        "the group query returned {rows} rows for `assign_by: {assign_by}` over {entity}, but none had both a unit key and a group value ({skipped} skipped)"
    )]
    NoGroups {
        entity: String,
        assign_by: String,
        rows: usize,
        skipped: usize,
    },
    #[error(
        "none of the {rows} rows of {measure} over {entity} belongs to a unit in the {groups} groups of the membership, so no group has a series"
    )]
    NoGroupedRows {
        measure: String,
        entity: String,
        rows: usize,
        groups: usize,
    },
    #[error(
        "the workspace is on revision {actual}, but this request was made against revision {expected}; ask for pricing again"
    )]
    Revision { expected: Uuid, actual: String },
    // ── infrastructure ──
    #[error("the panel query failed: {0}")]
    Query(String),
    #[error("the panel query task panicked before returning rows: {0}")]
    Panicked(String),
    #[error("the workspace could not be read for pricing: {0}")]
    Workspace(String),
}

impl From<PanelRefusal> for OxyError {
    fn from(r: PanelRefusal) -> Self {
        match r {
            PanelRefusal::Query(_) | PanelRefusal::Panicked(_) | PanelRefusal::Workspace(_) => {
                OxyError::RuntimeError(r.to_string())
            }
            other => OxyError::ValidationError(other.to_string()),
        }
    }
}

/// How [`PanelRefusal::Revision`]'s message begins; kept beside it so
/// [`is_revision_refusal`] recognises it after the `OxyError` mapping above.
const REVISION_PREFIX: &str = "the workspace is on revision ";

/// A [`PanelRefusal::Revision`] once mapped to `OxyError::ValidationError`. It is a
/// race (a promotion between asking and reading) or a workspace never promoted,
/// not a property of the request: a queued job that reads it must fail and be
/// retried, never seal it as a person-actionable refusal.
pub(crate) fn is_revision_refusal(e: &OxyError) -> bool {
    matches!(e, OxyError::ValidationError(m) if m.starts_with(REVISION_PREFIX))
}

/// `Ok` only when the workspace's promoted revision is the one the caller named.
///
/// The definition is read at the request's revision, the semantic model at the
/// promoted one; pricing across a promotion would price a model the caller
/// never saw.
pub fn check_revision(actual: Option<Uuid>, expected: Uuid) -> Result<(), PanelRefusal> {
    match actual {
        Some(a) if a == expected => Ok(()),
        Some(a) => Err(PanelRefusal::Revision {
            expected,
            actual: a.to_string(),
        }),
        None => Err(PanelRefusal::Revision {
            expected,
            actual: "none (never promoted)".to_string(),
        }),
    }
}

/// The one query. `limit: Some(UNBOUNDED_QUERY_LIMIT)` because the default 10k
/// cap would silently truncate the panel and bias every estimate that reads it
/// (`/Users/home/code/airlayer/src/engine/metric_tree_fit.rs:423-427`).
/// Both bounds move back to the start of their period, so the panel holds whole
/// periods only: the period `to_exclusive` falls in has not finished.
pub fn build_request(spec: &PanelSpec, history: &HistoryBounds) -> QueryRequest {
    let bound = |operator: FilterOperator, date: NaiveDate| QueryFilter {
        member: Some(spec.time_dimension.clone()),
        operator: Some(operator),
        values: vec![date.to_string()],
        and: None,
        or: None,
    };
    let grain = spec.grain;
    let mut filters = vec![bound(
        FilterOperator::BeforeDate,
        grain.floor(history.to_exclusive),
    )];
    if let Some(from) = history.from {
        filters.push(bound(FilterOperator::AfterOrOnDate, grain.floor(from)));
    }
    filters.extend(spec.filters.iter().map(measure_filter));
    QueryRequest {
        measures: vec![spec.measure.clone()],
        dimensions: vec![spec.key_dimension.clone()],
        time_dimensions: vec![TimeDimensionQuery {
            dimension: spec.time_dimension.clone(),
            granularity: Some(grain.as_str().to_string()),
            date_range: None,
        }],
        filters,
        timezone: (grain == Grain::Hour)
            .then(|| spec.timezone.clone())
            .flatten(),
        limit: Some(UNBOUNDED_QUERY_LIMIT),
        ..QueryRequest::new()
    }
}

fn measure_filter(f: &MeasureFilter) -> QueryFilter {
    let operator = match f.op {
        FilterOp::Equals => FilterOperator::Equals,
        FilterOp::NotEquals => FilterOperator::NotEquals,
    };
    QueryFilter {
        member: Some(f.dimension.clone()),
        operator: Some(operator),
        values: f.values.clone(),
        and: None,
        or: None,
    }
}

/// **The empty-result rule.** A zero-row answer returns before any triple or
/// `PanelMatrix` is constructed.
pub fn rows_to_outcome(
    rows: &[Map<String, Value>],
    spec: &PanelSpec,
) -> Result<PanelOutcome, PanelRefusal> {
    if rows.is_empty() {
        return Ok(PanelOutcome::NoHistory {
            measure: spec.measure.clone(),
            entity: spec.entity.clone(),
        });
    }
    Ok(PanelOutcome::Panel(PanelMatrix::from_triples(
        rows_to_triples(rows, spec)?,
    )))
}

/// Resolve, query, and map — synchronously, against whatever executor the
/// caller built. Production wraps this in [`load_panel`]; tests hand it DuckDB.
/// With `req.membership` the measure must add up (sum or count) — otherwise the
/// outcome is `Refused`, a value — and the panel is summed by exactly that
/// membership: the dimension is never re-resolved here. With `req.restrict_to`
/// only those units' rows survive (a switchback's pinned fleet); combining it
/// with a membership is refused.
pub fn fetch_panel(
    layer: &SemanticLayer,
    executor: &QueryExecutor,
    req: &PanelRequest,
) -> Result<PanelOutcome, OxyError> {
    if req.restrict_to.is_some() && req.membership.is_some() {
        return Ok(PanelOutcome::Refused {
            reason: "`restrict_to` (a switchback's pinned fleet) and a group membership were both given; they never combine, so the panel cannot say which to follow".to_string(),
        });
    }
    let spec = resolve_spec(layer, req)?;
    if req.membership.is_some() {
        match super::panel_groups::ensure_additive(layer, &req.measure) {
            Ok(()) => {}
            // A value, not an error: a ratio target will not become additive on a retry.
            Err(PanelRefusal::Config(c @ ConfigError::AssignByNotAdditive { .. })) => {
                return Ok(PanelOutcome::Refused {
                    reason: c.to_string(),
                });
            }
            Err(other) => return Err(other.into()),
        }
    }
    let mut rows = executor(&build_request(&spec, &req.history))
        .map_err(|e| PanelRefusal::Query(e.to_string()))?;
    if let Some(keep) = &req.restrict_to {
        restrict_rows(&mut rows, &spec, keep);
    }
    let outcome = match &req.membership {
        None => match req.unit_floor {
            Some(floor) => super::panel_rows::rows_to_thinned(&rows, &spec, floor)?,
            None => rows_to_outcome(&rows, &spec)?,
        },
        Some(m) => super::panel_groups::rows_to_group_outcome(&rows, &spec, m)?,
    };
    Ok(outcome)
}

/// Keep only the rows whose unit is in `keep` (exact, case-sensitive names).
fn restrict_rows(rows: &mut Vec<Map<String, Value>>, spec: &PanelSpec, keep: &[String]) {
    let key = spec.key_alias();
    rows.retain(|r| {
        r.get(&key)
            .and_then(render_key)
            .is_some_and(|u| keep.contains(&u))
    });
}

pub use super::panel_groups::resolve_groups;
pub use super::panel_rows::rows_to_triples;
pub(super) use super::panel_rows::{parse_day, parse_value, render_key};
pub use super::panel_spec::resolve_spec;
pub(super) use super::panel_spec::{measure_view, primary_owner, time_dimension};

/// Load the panel for `req` from a workspace's warehouse (C2a). With
/// `req.membership` the panel is one row per group per day, summed by exactly
/// that membership.
///
/// The query runs on a blocking thread: the executor is synchronous and
/// bridges to the async connector with `Handle::block_on`.
pub async fn load_panel(
    db: &sea_orm::DatabaseConnection,
    ctx: &PanelContext,
    req: &PanelRequest,
) -> Result<PanelOutcome, OxyError> {
    let opened = super::panel_workspace::open(db, ctx).await?;
    let request = req.clone();
    let (user, handle) = (ctx.requested_by, tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        let (layer, executor) = opened.into_executor(user, handle);
        fetch_panel(&layer, &*executor, &request)
    })
    .await
    .map_err(|e| OxyError::from(PanelRefusal::Panicked(e.to_string())))?
}
