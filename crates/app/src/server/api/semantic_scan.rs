//! Moved to `oxy::config::scan`.
//!
//! These readers ARE the compiled-vs-disk decision for the semantic scan, an
//! agent's `context:` root and `.monitor.yml`. That decision belongs to `ConfigManager` — living here made
//! it a free function every caller could reach around, and the four
//! `compiled_*` row readers it uses had to be public for it, which is what let
//! three other call sites write their own half of the choice.
//!
//! They are `pub(super)` in core now. This module re-exports the entry points
//! so the call sites read the same; there is no second door left.

pub use oxy::config::scan::{
    MaterialisedContext, MaterialisedMonitorConfig, ScanDir, SemanticEntity, SemanticTarget,
    materialise_agent_context, materialise_monitor_config, read_semantic_entity, scan_dir,
};
