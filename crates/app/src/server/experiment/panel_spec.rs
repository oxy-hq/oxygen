//! Spec resolution — the entity, the measure and the calendar, through the
//! semantic model. Pure; re-exported from [`super::panel`].

use oxy_airlayer_compat::schema::models::{DimensionType, EntityType};
use oxy_airlayer_compat::{SemanticLayer, View};
use oxy_experiment::{ConfigError, Grain, MeasureFilter};

use super::panel::{PanelRefusal, PanelRequest, PanelSpec};

/// Resolve the entity, the measure and the calendar through the semantic model.
pub fn resolve_spec(layer: &SemanticLayer, req: &PanelRequest) -> Result<PanelSpec, PanelRefusal> {
    let measure_view = measure_view(layer, &req.measure)?;
    check_filters(measure_view, &req.filters)?;
    let (owner, owner_key) = primary_owner(layer, &req.entity)?;
    Ok(PanelSpec {
        entity: req.entity.clone(),
        measure: req.measure.clone(),
        key_dimension: key_dimension(measure_view, owner, &owner_key, &req.entity),
        time_dimension: calendar(measure_view, req)?,
        grain: req.grain,
        timezone: req.timezone.clone(),
        filters: req.filters.clone(),
    })
}

/// The calendar dimension, and at hour grain the refusal of one with no time of day.
fn calendar(view: &View, req: &PanelRequest) -> Result<String, PanelRefusal> {
    let dim = time_dimension(view, &req.measure, req.time_dimension.as_deref())?;
    let is_date = view.dimensions.iter().any(|d| {
        format!("{}.{}", view.name, d.name) == dim && d.dimension_type == DimensionType::Date
    });
    if req.grain == Grain::Hour && is_date {
        return Err(PanelRefusal::HourNeedsDatetime { dimension: dim });
    }
    Ok(dim)
}

/// Every filter names a dimension of the measure's own view.
fn check_filters(view: &View, filters: &[MeasureFilter]) -> Result<(), PanelRefusal> {
    for f in filters {
        let declared = f
            .dimension
            .split_once('.')
            .is_some_and(|(v, d)| v == view.name && view.dimensions.iter().any(|x| x.name == d));
        if !declared {
            return Err(PanelRefusal::UnknownFilterDimension {
                dimension: f.dimension.clone(),
                view: view.name.clone(),
            });
        }
    }
    Ok(())
}

pub(super) fn measure_view<'a>(
    layer: &'a SemanticLayer,
    target: &str,
) -> Result<&'a View, PanelRefusal> {
    let unknown = || PanelRefusal::UnknownMeasure(target.to_string());
    let (view_name, measure_name) = target.split_once('.').ok_or_else(unknown)?;
    let view = layer
        .views
        .iter()
        .find(|v| v.name == view_name)
        .ok_or_else(unknown)?;
    let declared = view
        .measures
        .as_ref()
        .is_some_and(|ms| ms.iter().any(|m| m.name == measure_name));
    declared.then_some(view).ok_or_else(unknown)
}

fn declares_primary(view: &View, entity: &str) -> bool {
    view.entities
        .iter()
        .any(|e| e.name == entity && e.entity_type == EntityType::Primary)
}

/// The one view that owns `entity` as a primary, and its single key.
pub(super) fn primary_owner<'a>(
    layer: &'a SemanticLayer,
    entity: &str,
) -> Result<(&'a View, String), PanelRefusal> {
    let owners: Vec<&View> = layer
        .views
        .iter()
        .filter(|v| declares_primary(v, entity))
        .collect();
    match owners.as_slice() {
        [] => Err(PanelRefusal::UnknownEntity(entity.to_string())),
        [view] => {
            let keys = view
                .entities
                .iter()
                .find(|e| e.name == entity && e.entity_type == EntityType::Primary)
                .map(|e| e.get_keys())
                .unwrap_or_default();
            match keys.as_slice() {
                [] => Err(PanelRefusal::NoKey(entity.to_string())),
                [key] => Ok((view, key.clone())),
                _ => Err(ConfigError::CompositeKey {
                    entity: entity.to_string(),
                    keys: keys.clone(),
                }
                .into()),
            }
        }
        many => Err(PanelRefusal::AmbiguousEntity {
            entity: entity.to_string(),
            views: many.iter().map(|v| v.name.clone()).collect(),
        }),
    }
}

/// The dimension whose value is the unit's name.
///
/// The measure's own view when it names the entity with a single key that is
/// one of its dimensions — no join, so no cross-grain fan-out
/// (`crates/app/tests/platform/metric_tree_fit_panel.rs:200-233`); otherwise the
/// owning view's key, which airlayer joins in through the entity.
fn key_dimension(measure_view: &View, owner: &View, owner_key: &str, entity: &str) -> String {
    if owner.name != measure_view.name {
        let own = measure_view
            .entities
            .iter()
            .find(|e| e.name == entity)
            .map(|e| e.get_keys());
        if let Some(keys) = own
            && let [key] = keys.as_slice()
            && measure_view.dimensions.iter().any(|d| &d.name == key)
        {
            return format!("{}.{key}", measure_view.name);
        }
    }
    format!("{}.{owner_key}", owner.name)
}

/// The calendar dimension: `requested` (`design.time_dimension`) when it is a
/// date dimension of the measure's own view, else the view's only one.
pub(super) fn time_dimension(
    view: &View,
    measure: &str,
    requested: Option<&str>,
) -> Result<String, PanelRefusal> {
    let found: Vec<String> = view
        .dimensions
        .iter()
        .filter(|d| {
            matches!(
                d.dimension_type,
                DimensionType::Date | DimensionType::Datetime
            )
        })
        .map(|d| format!("{}.{}", view.name, d.name))
        .collect();
    let (measure, view) = (measure.to_string(), view.name.clone());
    match (requested, found.as_slice()) {
        (Some(given), _) if found.iter().any(|f| f == given) => Ok(given.to_string()),
        (Some(given), _) => Err(PanelRefusal::TimeDimensionNotADate {
            measure,
            view,
            given: given.to_string(),
            found,
        }),
        (None, [only]) => Ok(only.clone()),
        (None, []) => Err(PanelRefusal::NoTimeDimension { measure, view }),
        (None, _) => Err(PanelRefusal::TimeDimensionAmbiguous {
            measure,
            view,
            found,
        }),
    }
}
