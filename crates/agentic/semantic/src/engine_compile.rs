//! Compiling a semantic query against a pre-built engine. No preagg check is
//! performed here; a caller that wants one passes the returned request to
//! [`crate::compile::try_resolve_preagg`].

use oxy_airlayer_compat::engine::query::QueryRequest;
use oxy_airlayer_compat::{request_dialect, substitute_params};

use crate::compile::{build_query_request, collect_date_fields, resolve_topic};
use crate::config::SemanticQueryConfig;
use crate::error::SemanticError;

/// Compile a semantic query using a pre-built `SemanticEngine`.
///
/// Use this when compiling multiple queries against the same schema — build
/// the engine once with [`oxy_airlayer_compat::SemanticEngine::from_semantic_layer`] and
/// call this for each query so the expensive engine-build cost is paid once.
/// Returns only the SQL string; no preagg check is performed (see
/// [`compile_request_with_engine`] for a caller that does its own).
pub fn compile_with_engine(
    engine: &oxy_airlayer_compat::SemanticEngine,
    task: &SemanticQueryConfig,
) -> Result<String, SemanticError> {
    compile_request_with_engine(engine, task).map(|(sql, _)| sql)
}

/// [`compile_with_engine`], also returning the [`QueryRequest`] the SQL was
/// compiled from (topic default filters included): what
/// [`crate::compile::try_resolve_preagg`] judges a rollup's coverage by.
pub fn compile_request_with_engine(
    engine: &oxy_airlayer_compat::SemanticEngine,
    task: &SemanticQueryConfig,
) -> Result<(String, QueryRequest), SemanticError> {
    let semantic_layer = engine.semantic_layer();
    let topic = resolve_topic(semantic_layer, task)?;
    let views: Vec<&oxy_airlayer_compat::View> = semantic_layer
        .views
        .iter()
        .filter(|v| topic.views.contains(&v.name))
        .collect();
    let date_fields = collect_date_fields(&views);
    let request = build_query_request(
        task,
        &topic.name,
        topic.base_view.as_ref(),
        topic.default_filters.as_ref(),
        &date_fields,
    )?;
    let result = engine
        .compile_query(&request)
        .map_err(|e| SemanticError::Runtime(format!("query compilation error: {e}")))?;
    let sql = substitute_params(
        &request_dialect(engine, &request),
        &result.sql,
        &result.params,
    );
    Ok((sql, request))
}
