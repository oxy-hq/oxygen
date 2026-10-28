//! `BuilderSchemaProvider` implementation using oxy config types + embedded semantic schemas.
//!
//! Semantic type schemas (Dimension, View, Topic, etc.) are pre-generated from
//! `oxy-semantic` and embedded as static JSON. Non-semantic schemas (AgentConfig,
//! Workflow, etc.) are still generated at runtime from `oxy::config::model`.

use agentic_builder::BuilderSchemaProvider;
use schemars::schema_for;

const SUPPORTED_TYPES: &[&str] = &[
    // Semantic (embedded)
    "Dimension",
    "DimensionType",
    "Measure",
    "MeasureType",
    "MeasureFilter",
    "View",
    "Topic",
    "Entity",
    "SemanticLayer",
    // Workflow.
    //
    // `VisualizeTask` and `EvalConfig` are intentionally absent: the
    // agentic runner does not execute `type: visualize` steps (the
    // legacy chart-from-data task type is retired; charts come from
    // the chat agent's `visualize` tool now) and inline workflow
    // `tests:` blocks are replaced by standalone `*.agent.test.yml`
    // files. Surfacing them in the builder copilot's
    // schema palette would let it suggest options that fail at parse
    // time.
    "Workflow",
    "Task",
    "TaskType",
    "ExecuteSQLTask",
    "SemanticQueryTask",
    "FormatterTask",
    "WorkflowTask",
    "LoopSequentialTask",
    "ConditionalTask",
    // `TaskCache` (legacy name for the new `CacheConfig`) gates the
    // file-presence cache. See `agentic_automation::config::CacheConfig`
    // for the new runner; the schema name stays "TaskCache" so
    // existing YAML / IDE suggestions match.
    "TaskCache",
    "Config",
    "Database",
    "DatabaseType",
];

/// Schema provider that uses embedded JSON for semantic types and runtime
/// `schema_for!()` on oxy config types.
pub struct OxyBuilderSchemaProvider;

impl Default for OxyBuilderSchemaProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl OxyBuilderSchemaProvider {
    pub fn new() -> Self {
        Self
    }
}

impl BuilderSchemaProvider for OxyBuilderSchemaProvider {
    fn get_schema(&self, object_name: &str) -> Option<serde_json::Value> {
        // Semantic types — pre-generated, embedded as static JSON.
        let embedded = match object_name {
            "Dimension" => Some(include_str!("schemas/Dimension.json")),
            "DimensionType" => Some(include_str!("schemas/DimensionType.json")),
            "Measure" => Some(include_str!("schemas/Measure.json")),
            "MeasureType" => Some(include_str!("schemas/MeasureType.json")),
            "MeasureFilter" => Some(include_str!("schemas/MeasureFilter.json")),
            "View" => Some(include_str!("schemas/View.json")),
            "Topic" => Some(include_str!("schemas/Topic.json")),
            "Entity" | "SemanticEntity" => Some(include_str!("schemas/Entity.json")),
            "SemanticLayer" => Some(include_str!("schemas/SemanticLayer.json")),
            _ => None,
        };
        if let Some(json_str) = embedded {
            return serde_json::from_str(json_str).ok();
        }

        // Non-semantic types — generated at runtime from oxy config types.
        use oxy::config::model as cfg;

        let schema = match object_name {
            "Workflow" => serde_json::to_value(schema_for!(cfg::Workflow)),
            "Task" => serde_json::to_value(schema_for!(cfg::Task)),
            "TaskType" => serde_json::to_value(schema_for!(cfg::TaskType)),
            "ExecuteSQLTask" => serde_json::to_value(schema_for!(cfg::ExecuteSQLTask)),
            "SemanticQueryTask" => serde_json::to_value(schema_for!(cfg::SemanticQueryTask)),
            "FormatterTask" => serde_json::to_value(schema_for!(cfg::FormatterTask)),
            "WorkflowTask" => serde_json::to_value(schema_for!(cfg::WorkflowTask)),
            "LoopSequentialTask" => serde_json::to_value(schema_for!(cfg::LoopSequentialTask)),
            "ConditionalTask" => serde_json::to_value(schema_for!(cfg::ConditionalTask)),
            "TaskCache" => serde_json::to_value(schema_for!(cfg::TaskCache)),

            "Config" => serde_json::to_value(schema_for!(cfg::Config)),
            "Database" => serde_json::to_value(schema_for!(cfg::Database)),
            "DatabaseType" => serde_json::to_value(schema_for!(cfg::DatabaseType)),

            _ => return None,
        };

        schema.ok()
    }

    fn supported_types(&self) -> &[&str] {
        SUPPORTED_TYPES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_semantic::models as sem;

    /// The embedded semantic schemas are snapshots of `schema_for!` on the
    /// `oxy-semantic` types. Nothing at compile time ties the two together, so
    /// a field added to `Measure` or a variant added to `MeasureType` silently
    /// leaves the builder copilot describing a shape that no longer parses.
    /// Regenerate the file named in the failure rather than editing it by hand.
    macro_rules! assert_schema_matches {
        ($name:literal, $t:ty) => {
            let embedded = OxyBuilderSchemaProvider::new()
                .get_schema($name)
                .unwrap_or_else(|| panic!("no embedded schema for {}", $name));
            let generated = serde_json::to_value(schema_for!($t)).unwrap();
            assert_eq!(
                embedded,
                generated,
                "schemas/{}.json is out of date with oxy_semantic::models::{}; \
                 regenerate it from schema_for!()",
                $name,
                stringify!($t),
            );
        };
    }

    #[test]
    fn embedded_semantic_schemas_match_generated() {
        assert_schema_matches!("Dimension", sem::Dimension);
        assert_schema_matches!("DimensionType", sem::DimensionType);
        assert_schema_matches!("Measure", sem::Measure);
        assert_schema_matches!("MeasureType", sem::MeasureType);
        assert_schema_matches!("MeasureFilter", sem::MeasureFilter);
        assert_schema_matches!("View", sem::View);
        assert_schema_matches!("Topic", sem::Topic);
        assert_schema_matches!("Entity", sem::Entity);
        assert_schema_matches!("SemanticLayer", sem::SemanticLayer);
    }

    /// `SUPPORTED_TYPES` is what the copilot is told it may ask for; a name
    /// listed there that resolves to nothing is a dead entry.
    #[test]
    fn every_supported_type_resolves() {
        let provider = OxyBuilderSchemaProvider::new();
        for name in SUPPORTED_TYPES {
            assert!(
                provider.get_schema(name).is_some(),
                "SUPPORTED_TYPES lists {name}, but get_schema returns None"
            );
        }
    }
}
