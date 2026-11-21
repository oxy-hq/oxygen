//! Compiling `query`'s `semantic` argument, with did-you-mean on bad names.

use agentic_core::react::ToolFailure;
use agentic_semantic::compile::compile_with_engine;
use agentic_semantic::config::SemanticQueryConfig;

use super::query::closest;
use crate::SemanticCatalog;

/// The SQL for `config`. A member the catalog does not define fails before
/// compiling, with the closest `view.member` names as candidates.
pub(super) fn compile_semantic(
    catalog: &SemanticCatalog,
    config: &SemanticQueryConfig,
) -> Result<String, ToolFailure> {
    let known = members(catalog);
    let unknown: Vec<&str> = requested(config)
        .filter(|m| !known.iter().any(|k| k == m))
        .collect();
    if !unknown.is_empty() {
        let mut candidates: Vec<String> = Vec::new();
        for u in &unknown {
            for c in closest(u, known.iter().map(String::as_str)) {
                if !candidates.contains(&c) {
                    candidates.push(c);
                }
            }
        }
        return Err(ToolFailure {
            error: format!("unknown semantic members: {}", unknown.join(", ")),
            hint: Some("name members as `view.member`; search the views".into()),
            candidates,
        });
    }
    compile_with_engine(catalog.engine(), config).map_err(|e| ToolFailure {
        error: e.to_string(),
        hint: None,
        candidates: Vec::new(),
    })
}

/// Every `view.dimension` and `view.measure` the catalog defines.
fn members(catalog: &SemanticCatalog) -> Vec<String> {
    let mut out = Vec::new();
    for view in catalog.engine().views() {
        let dims = view.dimensions.iter().map(|d| &d.name);
        let measures = view.measures_list().iter().map(|m| &m.name);
        out.extend(dims.chain(measures).map(|n| format!("{}.{n}", view.name)));
    }
    out
}

/// The member names a query asks for: measures, dimensions, time dimensions.
fn requested(config: &SemanticQueryConfig) -> impl Iterator<Item = &str> {
    config
        .measures
        .iter()
        .chain(&config.dimensions)
        .map(String::as_str)
        .chain(config.time_dimensions.iter().map(|t| t.dimension.as_str()))
}

/// The datasource every view of `config` names: `None` when none names one.
/// Two views on different datasources cannot run as one statement, so the
/// failure names both.
pub(super) fn semantic_datasource(
    catalog: &SemanticCatalog,
    config: &SemanticQueryConfig,
) -> Result<Option<String>, ToolFailure> {
    let mut found: Option<(String, String)> = None;
    let fields = requested(config)
        .chain(config.filters.iter().map(|f| f.field.as_str()))
        .chain(config.orders.iter().map(|o| o.field.as_str()));
    for field in fields {
        let view = field.split('.').next().unwrap_or(field);
        let ds = catalog
            .engine()
            .views()
            .iter()
            .find(|v| v.name == view)
            .and_then(|v| v.datasource.clone());
        let Some(ds) = ds else { continue };
        match &found {
            None => found = Some((view.to_owned(), ds)),
            Some((first, first_ds)) if !first_ds.eq_ignore_ascii_case(&ds) => {
                return Err(ToolFailure {
                    error: format!(
                        "this query spans two datasources: {first} is on {first_ds}, \
                         {view} is on {ds}"
                    ),
                    hint: Some("query one datasource at a time".into()),
                    candidates: Vec::new(),
                });
            }
            Some(_) => {}
        }
    }
    Ok(found.map(|(_, ds)| ds))
}
