//! Shared semantic-query compile + preagg execution helpers.
//!
//! Extracted out of `agentic-automation` so both the analytics domain and
//! the automation domain can call the same preagg-aware compile path
//! without violating the domain-to-domain dependency ban.

pub mod compile;
pub mod config;
mod engine_compile;
pub mod error;
pub mod preagg;
#[cfg(test)]
mod preagg_equivalence_tests;
pub mod refresh_key_cache;
mod rollup_value;

pub use compile::{
    BlobConfig, CompiledQuery, PreaggContext, PreaggSource, compile_with_engine,
    get_database_from_views, resolve_and_compile, resolve_and_compile_cached,
};
pub use error::SemanticError;
