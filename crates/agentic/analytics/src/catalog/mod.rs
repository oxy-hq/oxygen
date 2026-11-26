//! Unified catalog interface and schema-only implementation.
//!
//! Three catalog implementations live across three areas:
//!
//! | File | Type | Description |
//! |---|---|---|
//! | [`schema`] (this module) | [`SchemaCatalog`] | Raw database schema — no business logic |
//! | `semantic.rs` | [`SemanticCatalog`] | Oxy .view.yml/.topic.yml semantic model |
//! | `hybrid.rs` | `HybridCatalog` | Semantic + schema fallback (primary runtime type) |
//!
//! The FSM, tools, and Specifying handler work through the [`Catalog`] trait
//! exclusively — they never inspect the concrete type behind it.
//!
//! [`SemanticCatalog`]: crate::semantic::SemanticCatalog

pub mod helpers;
pub mod relation;
pub mod schema;
pub mod schema_trait;
pub mod traits;
pub mod types;

#[allow(unused_imports)]
pub use schema::{SchemaCatalog, SchemaMergeError};
pub use traits::Catalog;
#[allow(unused_imports)]
pub use types::{
    CatalogError, CatalogSearchResult, ColumnRange, DimensionSummary, FreshnessTarget, JoinPath,
    MetricDef, MetricSummary, QueryContext, SampleTarget,
};

// ── Fuzzy matching ───────────────────────────────────────────────────────────

/// Minimum Jaro-Winkler similarity score for a fuzzy match.
///
/// 0.8 is conservative enough to avoid matching unrelated short names
/// (e.g. `"min"` vs `"max"` scores ~0.67) while catching common typos and
/// abbreviations (e.g. `"revnue"` vs `"revenue"` scores ~0.95).
const FUZZY_THRESHOLD: f64 = 0.8;

/// Check whether `query` fuzzy-matches `candidate` using Jaro-Winkler similarity.
///
/// Operates on lowercase strings.  Returns `true` when the similarity exceeds
/// [`FUZZY_THRESHOLD`].  Short queries (< 3 chars) are excluded to avoid
/// noise from very short strings where Jaro-Winkler is unreliable.
pub fn fuzzy_matches(query: &str, candidate: &str) -> bool {
    if query.len() < 3 {
        return false;
    }
    let q = query.to_lowercase();
    let c = candidate.to_lowercase();
    strsim::jaro_winkler(&q, &c) >= FUZZY_THRESHOLD
}

// ── Backward-compat alias ─────────────────────────────────────────────────────

/// Kept for backward compatibility.
///
/// New code should use [`CatalogError::TooComplex`] instead.
#[derive(Debug)]
pub enum SemanticLayerError {
    /// The query is too complex for the semantic model to compile directly;
    /// the caller should fall back to LLM-based specification.
    TooComplex,
}

impl From<SemanticLayerError> for CatalogError {
    fn from(_: SemanticLayerError) -> Self {
        CatalogError::TooComplex("semantic model error".into())
    }
}
