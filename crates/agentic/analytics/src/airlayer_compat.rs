//! Compatibility layer between oxy's YAML format and airlayer types.
//!
//! This module provides:
//! - YAML parsing shims for `.view.yml` / `.topic.yml` files that may differ
//!   slightly from airlayer's expected format (e.g. optional `description`).
//! - Dialect mapping from [`agentic_connector::SqlDialect`] to [`oxy_airlayer_compat::Dialect`].
//! - Parameter substitution for airlayer's parameterised SQL output.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use oxy_airlayer_compat::DatasourceDialectMap;

// ── YAML parsing ─────────────────────────────────────────────────────────────
//
// The oxy → airlayer YAML shim is the canonical one in `oxy-airlayer-compat`
// (infrastructure) so analytics, the builder validator, the automation bridge
// and `oxy validate` cannot disagree. Re-exported here so existing
// `crate::airlayer_compat::parse_*_yaml` call sites are unchanged.
// See internal-docs/semantic-validation-standardization.md.
pub use oxy_airlayer_compat::{build_layer, parse_topic_yaml, parse_view_yaml};

// ── Dialect mapping ──────────────────────────────────────────────────────────

/// Convert an [`agentic_connector::SqlDialect`] to an [`oxy_airlayer_compat::Dialect`].
///
/// Returns `None` for unknown / `Other` dialects that airlayer does not support.
pub fn convert_dialect(
    dialect: agentic_connector::SqlDialect,
) -> Option<oxy_airlayer_compat::Dialect> {
    let result = oxy_airlayer_compat::Dialect::from_str(dialect.as_str());
    if result.is_none() {
        tracing::warn!(
            connector_dialect = %dialect.as_str(),
            "convert_dialect: unrecognized dialect, skipping connector in dialect map"
        );
    }
    result
}

/// Build an [`oxy_airlayer_compat::DatasourceDialectMap`] from the solver's connector map.
///
/// Each connector's logical name is mapped to its SQL dialect.  The `default`
/// connector's dialect is set as the map's default for views that don't specify
/// a `datasource:` field.
pub fn build_dialect_map(
    connectors: &HashMap<String, Arc<dyn DatabaseConnector>>,
    default: &str,
) -> DatasourceDialectMap {
    let mut map = DatasourceDialectMap::new();

    for (name, connector) in connectors {
        if let Some(dialect) = convert_dialect(connector.dialect()) {
            map.insert(name, dialect.clone());
            // A lowercase alias, because airlayer's `DatasourceDialectMap`
            // resolves by exact `HashMap::get` while execution now tolerates a
            // case difference. Without this, `datasource: July_Airhouse` runs
            // on the right connector and compiles with the map's DEFAULT
            // dialect -- this PR's own bug, entering through the case door the
            // case-insensitive lookup opened.
            //
            // Skipped when the alias is already a registered name, so two
            // connectors differing only in case keep their own dialects.
            let lower = name.to_ascii_lowercase();
            if lower != *name && !connectors.contains_key(&lower) {
                map.insert(&lower, dialect);
            }
        }
    }

    if let Some(connector) = connectors.get(default) {
        if let Some(dialect) = convert_dialect(connector.dialect()) {
            map.set_default(dialect);
        }
    } else if let Some((fallback_name, connector)) =
        // `min_by_key`, not `iter().next()`: HashMap order is nondeterministic,
        // so the old form picked a different dialect per process. Third and
        // last instance of this tail -- `helpers.rs` and `config/mod.rs` shed
        // theirs in this same change. Only reachable when `default` is
        // unregistered, which the routing guard now refuses outright, so this
        // is belt-and-braces.
        connectors.iter().min_by_key(|(name, _)| name.as_str())
    {
        tracing::warn!(
            missing = default,
            chosen = %fallback_name,
            "build_dialect_map: named default connector not found, using first available as fallback"
        );
        if let Some(dialect) = convert_dialect(connector.dialect()) {
            map.set_default(dialect);
        }
    }

    map
}

// ── Parameter substitution ───────────────────────────────────────────────────

pub use oxy_airlayer_compat::{request_dialect, substitute_params};
