//! Measure filters (`outcome.target_filter`, `outcome.driver_filter`,
//! `substitution_check.filter`): each narrows its own measure's view.

use super::ConfigError;
use crate::config::ExperimentConfig;

fn view_of(member: &str) -> &str {
    member.split_once('.').map_or("", |(view, _)| view)
}

/// Each filter narrows its own measure's view, with at least one value; a filter
/// on a measure the design does not name is refused.
pub(super) fn check_filters(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let mut filtered = vec![
        (
            "outcome.target_filter",
            Some(&c.outcome.target),
            &c.outcome.target_filter,
        ),
        (
            "outcome.driver_filter",
            c.outcome.driver.as_ref(),
            &c.outcome.driver_filter,
        ),
    ];
    if let Some(s) = &c.substitution_check {
        filtered.push(("substitution_check.filter", Some(&s.sibling), &s.filter));
    }
    for (field, measure, filters) in filtered {
        if filters.is_empty() {
            continue;
        }
        let Some(measure) = measure else {
            out.push(ConfigError::FilterWithoutMeasure {
                field: field.to_string(),
            });
            continue;
        };
        for (i, f) in filters.iter().enumerate() {
            let field = format!("{field}[{i}]");
            if view_of(&f.dimension).is_empty() || view_of(&f.dimension) != view_of(measure) {
                out.push(ConfigError::FilterOutsideView {
                    field,
                    dimension: f.dimension.clone(),
                    measure: measure.clone(),
                });
            } else if f.values.is_empty() {
                out.push(ConfigError::FilterNoValues { field });
            }
        }
    }
}
