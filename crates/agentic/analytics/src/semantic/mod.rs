//! [`SemanticCatalog`] — Oxy semantic model backed by [`oxy_airlayer_compat::SemanticEngine`].
//!
//! All catalog operations (search, metric definitions, join paths, and query
//! compilation) delegate to airlayer's data structures.  YAML parsing goes
//! through a thin shim in [`crate::airlayer_compat`] that handles minor format
//! differences between oxy's `.view.yml` files and airlayer's strict schema.
//!
//! Implementation is split across sibling modules by concern:
//! - [`helpers`]: view/measure/dimension lookup + qualify_names + fuzzy-normalize.
//! - [`filters`]: parse raw filter strings into structured airlayer `QueryFilter`.
//! - [`translation`]: translate airlayer `QueryRequest` → raw-schema context.
//! - [`trait_impl`]: `impl Catalog for SemanticCatalog` (trait surface for tools).
//!
//! # Oxy YAML format
//!
//! **views/<name>.view.yml**
//! ```yaml
//! name: orders_view
//! description: Order analytics
//! table: orders          # OR: sql: "SELECT …"
//! entities:
//!   - name: order_id
//!     type: primary
//!     key: order_id
//!   - name: customer_id
//!     type: foreign
//!     key: customer_id
//! dimensions:
//!   - name: status
//!     type: string
//!     expr: status
//!     description: Order status
//!     samples: [completed, pending, cancelled]
//!   - name: order_date
//!     type: date
//!     expr: order_date
//! measures:
//!   - name: revenue
//!     type: sum
//!     expr: amount
//!     description: Total revenue
//!   - name: order_count
//!     type: count
//! ```
//!
//! **topics/<name>.topic.yml**
//! ```yaml
//! name: sales_analytics
//! description: Sales analytics domain
//! views:
//!   - orders_view
//!   - customers_view
//! ```

use std::path::{Path, PathBuf};

use crate::airlayer_compat;
use crate::catalog::QueryContext;

pub mod filters;
pub mod helpers;
pub mod trait_impl;
pub mod translation;

#[cfg(test)]
mod tests;

// ── SemanticCatalog ───────────────────────────────────────────────────────────

/// Semantic model catalog backed by [`oxy_airlayer_compat::SemanticEngine`].
///
/// # Behavior
///
/// - **`search_catalog`**: batch-search metrics and dimensions across all views.
/// - **`list_metrics`**: searches measure names and descriptions across all views.
/// - **`list_dimensions`**: returns dimensions from the metric's view plus views
///   joinable via entity relationships.
/// - **`try_compile`**: delegates to `engine.compile_query()` — supports multi-hop
///   joins, CTE fan-out protection, and dialect-aware SQL generation.  Returns
///   [`crate::catalog::CatalogError::TooComplex`] for queries that airlayer cannot compile.
pub struct SemanticCatalog {
    pub(super) engine: oxy_airlayer_compat::SemanticEngine,
    /// Memoized [`SemanticCatalog::live_rollups`]. The views behind it are
    /// fixed at construction, so the resolve is worth doing once — the solver
    /// asks for this set once per query spec.
    live_rollups: std::sync::OnceLock<oxy_airlayer_compat::preagg::LiveRollups>,
}

impl std::fmt::Debug for SemanticCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticCatalog")
            .field("views", &self.engine.views().len())
            .finish()
    }
}

impl SemanticCatalog {
    /// Create an empty catalog with no views.
    ///
    /// Useful for the "no semantic model" case — the LLM relies on database
    /// lookup tools instead.
    pub fn empty() -> Self {
        let layer = oxy_airlayer_compat::SemanticLayer::new(vec![], None);
        let dialects = oxy_airlayer_compat::DatasourceDialectMap::new();
        let engine = oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer, dialects)
            .expect("empty semantic model should always be valid");
        Self::from_engine(engine)
    }

    /// Return `true` when this catalog has no views.
    pub fn is_empty(&self) -> bool {
        self.engine.views().is_empty()
    }

    /// Wrap a pre-built engine (useful for testing).
    pub fn from_engine(engine: oxy_airlayer_compat::SemanticEngine) -> Self {
        Self {
            engine,
            live_rollups: std::sync::OnceLock::new(),
        }
    }

    /// The `(view_name, rollup_hash)` pairs this catalog's schema declares
    /// right now — airlayer's `live` set, which is what stops a rollup built
    /// from a since-edited definition answering a query.
    ///
    /// The coverage check that picks a rollup (`covers`) compares member
    /// *names* only, and never looks at the stored artifact. An edit that
    /// renames nothing — `expr: amount` to `expr: amount - refunds`,
    /// `type: sum` to `type: avg`, a new measure `filters:` entry, a repointed
    /// `table:` — moves the rollup's hash while leaving every name identical,
    /// so the old row keeps matching and the old numbers keep answering under
    /// the Pre-aggregated badge. Handing this set to `try_resolve_preagg` is
    /// what declines that row.
    ///
    /// This is the airlayer view list, not the vendor (cube/looker)
    /// abstraction: `SemanticCatalog` IS an airlayer engine, so the set is
    /// always derivable here.
    pub fn live_rollups(&self) -> &oxy_airlayer_compat::preagg::LiveRollups {
        self.live_rollups.get_or_init(|| {
            let views: Vec<&oxy_airlayer_compat::View> = self.engine.views().iter().collect();
            oxy_airlayer_compat::preagg::live_rollups(&views)
        })
    }

    /// Load from a `semantics/` directory containing `views/` and `topics/`
    /// subdirectories.
    pub fn load(
        semantics_dir: &Path,
        dialects: oxy_airlayer_compat::DatasourceDialectMap,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let views_dir = semantics_dir.join("views");
        let topics_dir = semantics_dir.join("topics");

        let topics_path = if topics_dir.exists() {
            Some(topics_dir.as_path())
        } else {
            None
        };

        let engine = oxy_airlayer_compat::SemanticEngine::load(&views_dir, topics_path, dialects)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
            Box::new(std::io::Error::other(e.to_string()))
        })?;

        Ok(Self::from_engine(engine))
    }

    /// Load from an explicit list of `.view.yml` and `.topic.yml` paths.
    ///
    /// Unlike [`load`], this does not assume a directory structure — each path
    /// is classified by its filename suffix and loaded directly.
    ///
    /// [`load`]: SemanticCatalog::load
    pub fn load_files(
        paths: &[PathBuf],
        dialects: oxy_airlayer_compat::DatasourceDialectMap,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Parsing goes through the canonical infra shim (re-exported as
        // `airlayer_compat::build_layer`) so analytics agrees with every
        // other semantic-file consumer. Dialect-aware engine construction
        // stays here — it is analytics-specific.
        let layer = airlayer_compat::build_layer(paths)?;
        let engine = oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer, dialects)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                Box::new(std::io::Error::other(e.to_string()))
            })?;

        Ok(Self::from_engine(engine))
    }

    // ── Validation helpers (used by validation rules) ────────────────────────

    /// Return `true` if `table` matches a view name (case-insensitive).
    pub fn table_exists(&self, table: &str) -> bool {
        self.engine
            .views()
            .iter()
            .any(|v| v.name.eq_ignore_ascii_case(table))
    }

    /// Return `true` if `column` is a dimension or measure of the view named
    /// `table` (case-insensitive match on both view name and field name).
    pub fn column_exists(&self, table: &str, column: &str) -> bool {
        self.field_exists(table, column)
    }

    /// Return all view names that contain a dimension or measure named `column`.
    pub fn column_tables(&self, column: &str) -> Vec<String> {
        let col_lc = column.to_lowercase();
        self.engine
            .views()
            .iter()
            .filter(|v| {
                v.dimensions.iter().any(|d| d.name.to_lowercase() == col_lc)
                    || v.measures_list()
                        .iter()
                        .any(|m| m.name.to_lowercase() == col_lc)
            })
            .map(|v| v.name.clone())
            .collect()
    }

    /// Return all dimension + measure names for the view named `table`.
    pub fn columns_of(&self, table: &str) -> Vec<String> {
        let Some(view) = self
            .engine
            .views()
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(table))
        else {
            return vec![];
        };
        let mut cols: Vec<String> = view.dimensions.iter().map(|d| d.name.clone()).collect();
        for m in view.measures_list() {
            if !cols.iter().any(|c| c.eq_ignore_ascii_case(&m.name)) {
                cols.push(m.name.clone());
            }
        }
        cols.sort();
        cols
    }

    /// Return the join key between two views, if an entity relationship exists.
    pub fn join_key(&self, a: &str, b: &str) -> Option<String> {
        use crate::catalog::Catalog;
        self.get_join_path(a, b).map(|jp| {
            // Extract the key from the path string "A JOIN B ON A.key = B.key".
            jp.path
                .split("ON ")
                .nth(1)
                .and_then(|s| s.split('=').next())
                .and_then(|s| s.split('.').nth(1))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        })
    }

    /// Return `true` if `metric` is recognized as a measure, a SQL expression
    /// containing view.column refs, or a dotted "view.measure" name.
    pub fn metric_resolves_in_semantic(&self, metric: &str) -> bool {
        // Bare measure name lookup.
        use crate::catalog::Catalog;
        if self.get_metric_definition(metric).is_some() {
            return true;
        }
        // SQL expression with table.column refs.
        if metric.contains('(') {
            let refs = crate::validation::extract_table_column_refs(metric);
            if !refs.is_empty() {
                return refs.iter().all(|(t, c)| self.field_exists(t, c));
            }
        }
        // Dotted "view.measure" format.
        if let Some(pos) = metric.find('.') {
            let (view_part, field_part) = (&metric[..pos], &metric[pos + 1..]);
            return self.field_exists(view_part, field_part);
        }
        false
    }

    /// Return `true` when a join path exists between two views via entities.
    pub fn join_exists_in_semantic(&self, left: &str, right: &str) -> bool {
        use crate::catalog::Catalog;
        self.get_join_path(left, right).is_some()
    }

    /// Prompt-ready description of the semantic model views.
    pub fn to_prompt_string(&self) -> String {
        if self.engine.views().is_empty() {
            return "No semantic model configured. Use list_tables and describe_table tools \
                    to discover database schema."
                .to_string();
        }
        use crate::catalog::Catalog;
        let dummy = crate::types::AnalyticsIntent {
            raw_question: String::new(),
            summary: String::new(),
            question_type: crate::types::QuestionType::SingleValue,
            metrics: vec![],
            dimensions: vec![],
            filters: vec![],
            history: vec![],
            spec_hint: None,
            selected_automation: None,
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        };
        let ctx: QueryContext = self.get_context(&dummy);
        ctx.schema_description
    }

    /// Compact table-level summary for decompose prompts.
    pub fn to_table_summary(&self) -> String {
        use crate::catalog::Catalog;
        let names = self.table_names();
        if names.is_empty() {
            return "No semantic views.".to_string();
        }
        format!("Semantic views ({}): {}", names.len(), names.join(", "))
    }

    /// Expose the underlying airlayer engine for direct compilation.
    pub fn engine(&self) -> &oxy_airlayer_compat::SemanticEngine {
        &self.engine
    }

    /// Return a concise summary of all topics in the semantic model.
    ///
    /// Each topic is rendered as `"- <name>: <description> (views: v1, v2, …)"`.
    /// Returns an empty string when no topics are defined.
    pub fn topics_summary(&self) -> String {
        let topics = self.engine.semantic_layer().topics_list();
        if topics.is_empty() {
            return String::new();
        }
        let lines: Vec<String> = topics
            .iter()
            .map(|t| {
                let views: Vec<String> = t
                    .views
                    .iter()
                    .map(|v_name| {
                        if let Some(view) = self.engine.view(v_name) {
                            // Seam 3 of the freshness design: surface the
                            // declared meta contract at discovery time so
                            // cadence/caveats reach triage without a tool
                            // call and without duplicating them into the
                            // description prose.
                            let freshness = view.meta.as_ref().and_then(|m| {
                                let first = |k: &str| {
                                    m.get(k)
                                        .and_then(|v| v.first())
                                        .map(|s| s.trim())
                                        .filter(|s| !s.is_empty())
                                };
                                let parts: Vec<&str> = [
                                    first("freshness_expected_cadence"),
                                    first("freshness_complete_through"),
                                    first("freshness_caveat"),
                                ]
                                .into_iter()
                                .flatten()
                                .collect();
                                (!parts.is_empty()).then(|| parts.join("; "))
                            });
                            match (view.description.as_deref(), freshness) {
                                (Some(desc), Some(f)) if !desc.is_empty() => {
                                    format!("{} ({}) [freshness: {}]", v_name, desc, f)
                                }
                                (Some(desc), None) if !desc.is_empty() => {
                                    format!("{} ({})", v_name, desc)
                                }
                                (_, Some(f)) => format!("{} [freshness: {}]", v_name, f),
                                _ => v_name.clone(),
                            }
                        } else {
                            v_name.clone()
                        }
                    })
                    .collect();
                format!(
                    "- {}: {} (views: {})",
                    t.name,
                    t.description.as_deref().unwrap_or(""),
                    views.join(", ")
                )
            })
            .collect();
        format!("<topics>\n{}\n</topics>", lines.join("\n"))
    }
}
