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
