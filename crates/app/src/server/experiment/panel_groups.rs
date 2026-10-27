//! Groups of units — `assign_by`. Pure over a layer and an executor.
//!
//! The estimator sees a panel of groups: this module builds it (the sum of a
//! group's members per day) and the membership it is built from. See
//! `internal-docs/experiments.md`, "Groups of units".

use std::collections::{BTreeMap, HashMap};

use oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT;
use oxy_airlayer_compat::engine::experiment::PanelMatrix;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::QueryRequest;
use oxy_airlayer_compat::schema::models::MeasureType;
use oxy_airlayer_compat::{SemanticLayer, View};
use oxy_experiment::ConfigError;
use oxy_shared::errors::OxyError;
use serde_json::{Map, Value};

use super::panel::{
    GroupMembership, PanelOutcome, PanelRefusal, PanelSpec, measure_view, primary_owner,
    render_key, rows_to_triples,
};

/// The two dimensions the membership query reads, both on the entity's owning view.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupSpec {
    pub entity: String,
    /// As the candidate wrote it.
    pub assign_by: String,
    pub key_dimension: String,
    pub group_dimension: String,
}

fn unresolved(assign_by: &str, why: String) -> PanelRefusal {
    ConfigError::AssignByUnresolved {
        assign_by: assign_by.to_string(),
        why,
    }
    .into()
}

/// `assign_by` → the owning view's key and group dimensions.
pub fn resolve_group_spec(
    layer: &SemanticLayer,
    entity: &str,
    assign_by: &str,
) -> Result<GroupSpec, PanelRefusal> {
    let (owner, key) = primary_owner(layer, entity)?;
    let dimension = group_dimension_name(owner, entity, assign_by)?;
    if dimension == key {
        return Err(unresolved(
            assign_by,
            format!("`{dimension}` is the entity's key, which would make every unit its own group"),
        ));
    }
    Ok(GroupSpec {
        entity: entity.to_string(),
        assign_by: assign_by.to_string(),
        key_dimension: format!("{}.{key}", owner.name),
        group_dimension: format!("{}.{dimension}", owner.name),
    })
}

fn group_dimension_name<'a>(
    owner: &View,
    entity: &str,
    assign_by: &'a str,
) -> Result<&'a str, PanelRefusal> {
    let dimension = match assign_by.split_once('.') {
        Some((q, d)) if q == entity || q == owner.name => d,
        Some((q, _)) => {
            return Err(unresolved(
                assign_by,
                format!(
                    "`{q}` is neither the entity `{entity}` nor the view `{}` that owns it",
                    owner.name
                ),
            ));
        }
        None => assign_by,
    };
    let known: Vec<&str> = owner.dimensions.iter().map(|d| d.name.as_str()).collect();
    if known.contains(&dimension) {
        Ok(dimension)
    } else {
        Err(unresolved(
            assign_by,
            format!(
                "`{}` declares no dimension `{dimension}`; its dimensions are [{}]",
                owner.name,
                known.join(", ")
            ),
        ))
    }
}

/// One row per unit of the owning view. No measure, no filter: membership is a
/// fact about the entity, not about a window. Unbounded — a capped read would
/// silently drop members and the dropped units would read as "never in a group".
pub fn build_membership_request(spec: &GroupSpec) -> QueryRequest {
    QueryRequest {
        dimensions: vec![spec.key_dimension.clone(), spec.group_dimension.clone()],
        limit: Some(UNBOUNDED_QUERY_LIMIT),
        ..QueryRequest::new()
    }
}

fn alias(member: &str) -> String {
    member.replace('.', "__")
}

/// Rows → membership. A unit under two group values is refused, naming it.
pub fn rows_to_membership(
    rows: &[Map<String, Value>],
    spec: &GroupSpec,
) -> Result<GroupMembership, PanelRefusal> {
    let (key_a, group_a) = (alias(&spec.key_dimension), alias(&spec.group_dimension));
    let mut group_of: BTreeMap<String, String> = BTreeMap::new();
    let mut skipped = 0usize;
    for row in rows {
        let (Some(unit), Some(group)) = (
            row.get(&key_a).and_then(render_key),
            row.get(&group_a).and_then(render_key),
        ) else {
            skipped += 1;
            continue;
        };
        match group_of.get(&unit) {
            Some(prev) if *prev != group => {
                return Err(unresolved(
                    &spec.assign_by,
                    format!(
                        "unit `{unit}` has more than one value of `{}` ({prev}, {group}); a dimension that varies within a unit cannot assign groups",
                        spec.group_dimension
                    ),
                ));
            }
            _ => {
                group_of.insert(unit, group);
            }
        }
    }
    if group_of.is_empty() {
        return Err(PanelRefusal::NoGroups {
            entity: spec.entity.clone(),
            assign_by: spec.assign_by.clone(),
            rows: rows.len(),
            skipped,
        });
    }
    let mut by_group: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (unit, group) in group_of {
        by_group.entry(group).or_default().push(unit);
    }
    Ok(GroupMembership(by_group))
}

/// A group's day is the SUM of its members' (spec): only `sum` and `count` add up.
pub fn ensure_additive(layer: &SemanticLayer, measure: &str) -> Result<(), PanelRefusal> {
    let view = measure_view(layer, measure)?;
    let name = measure.split_once('.').map(|(_, m)| m).unwrap_or(measure);
    let kind = view
        .measures_list()
        .iter()
        .find(|m| m.name == name)
        .map(|m| m.measure_type.clone())
        .ok_or_else(|| PanelRefusal::UnknownMeasure(measure.to_string()))?;
    match kind {
        MeasureType::Sum | MeasureType::Count => Ok(()),
        other => Err(ConfigError::AssignByNotAdditive {
            measure: measure.to_string(),
            aggregate: other.sql_function().to_string(),
        }
        .into()),
    }
}

/// `(unit, day, value)` → `(group, day, Σ members)`. Units outside the
/// membership are dropped; the result is sorted by group then day.
pub fn sum_by_group(
    triples: Vec<(String, i64, f64)>,
    m: &GroupMembership,
) -> Vec<(String, i64, f64)> {
    let group_of: HashMap<&str, &str> =
        m.0.iter()
            .flat_map(|(g, units)| units.iter().map(move |u| (u.as_str(), g.as_str())))
            .collect();
    let mut sums: BTreeMap<(String, i64), f64> = BTreeMap::new();
    for (unit, day, value) in triples {
        if let Some(group) = group_of.get(unit.as_str()) {
            *sums.entry(((*group).to_string(), day)).or_default() += value;
        }
    }
    sums.into_iter().map(|((g, d), v)| (g, d, v)).collect()
}

/// The group-summed twin of `rows_to_outcome`: an empty answer is still
/// `NoHistory` before anything is built.
pub fn rows_to_group_outcome(
    rows: &[Map<String, Value>],
    spec: &PanelSpec,
    m: &GroupMembership,
) -> Result<PanelOutcome, PanelRefusal> {
    if rows.is_empty() {
        return Ok(PanelOutcome::NoHistory {
            measure: spec.measure.clone(),
            entity: spec.entity.clone(),
        });
    }
    let grouped = sum_by_group(rows_to_triples(rows, spec)?, m);
    if grouped.is_empty() {
        return Err(PanelRefusal::NoGroupedRows {
            measure: spec.measure.clone(),
            entity: spec.entity.clone(),
            rows: rows.len(),
            groups: m.0.len(),
        });
    }
    Ok(PanelOutcome::Panel(PanelMatrix::from_triples(grouped)))
}

/// Mean members per group over `groups` (the ones the panel holds); `None`
/// when none of them is in the membership.
pub fn mean_group_size(m: &GroupMembership, groups: &[String]) -> Option<f64> {
    let sizes: Vec<usize> = groups
        .iter()
        .filter_map(|g| m.0.get(g))
        .map(Vec::len)
        .collect();
    (!sizes.is_empty()).then(|| sizes.iter().sum::<usize>() as f64 / sizes.len() as f64)
}

/// Resolve, query, map — synchronously, against whatever executor the caller built.
pub fn fetch_groups(
    layer: &SemanticLayer,
    executor: &QueryExecutor,
    entity: &str,
    assign_by: &str,
) -> Result<GroupMembership, OxyError> {
    let spec = resolve_group_spec(layer, entity, assign_by)?;
    let rows = executor(&build_membership_request(&spec))
        .map_err(|e| PanelRefusal::Query(e.to_string()))?;
    Ok(rows_to_membership(&rows, &spec)?)
}

/// Resolve `entity`'s group membership by `dimension` (`assign_by`) from a
/// workspace's warehouse (C2a). PR 4 calls this once, at the first check, and
/// pins the result; nothing downstream re-resolves it.
pub async fn resolve_groups(
    db: &sea_orm::DatabaseConnection,
    ctx: &super::panel::PanelContext,
    entity: &str,
    dimension: &str,
) -> Result<GroupMembership, OxyError> {
    let opened = super::panel_workspace::open(db, ctx).await?;
    let (entity, dimension) = (entity.to_string(), dimension.to_string());
    let (user, handle) = (ctx.requested_by, tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        let (layer, executor) = opened.into_executor(user, handle);
        fetch_groups(&layer, &*executor, &entity, &dimension)
    })
    .await
    .map_err(|e| OxyError::from(PanelRefusal::Panicked(e.to_string())))?
}
