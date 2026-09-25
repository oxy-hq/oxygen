use serde::Deserialize;
use std::path::Path;

/// Top-level config parsed from `.world-model.yml` at the workspace root.
/// If the file is absent the world model falls back to showing all entities.
#[derive(Debug, Default, Deserialize)]
pub struct WorldModelConfig {
    #[serde(default)]
    pub entities: Vec<WmEntityConfig>,
}

#[derive(Debug, Deserialize)]
pub struct WmEntityConfig {
    /// Matches the entity `id` (primary entity name) in the semantic model.
    pub id: String,
    /// Display name shown in the graph node and detail panel header.
    /// Falls back to `id` when absent.
    pub label: Option<String>,
    /// Description shown in the detail panel.
    /// Falls back to `view.description` from `.view.yml` when absent.
    pub description: Option<String>,
    /// Which dimension column to use as the human-readable display label for
    /// instances (e.g. shown in the instance picker and fiber samples).
    /// Falls back to the PK column(s) when absent.
    pub display_field: Option<String>,
    /// Allowlist of dimensions to show. `None` = show all (key absent from YAML).
    /// `Some([])` = show none.
    pub dimensions: Option<Vec<WmFieldConfig>>,
    /// Allowlist of measures to show. `None` = show all. `Some([])` = show none.
    pub measures: Option<Vec<WmFieldConfig>>,
}

#[derive(Debug, Deserialize)]
pub struct WmFieldConfig {
    pub name: String,
    /// Display label override. Falls back to `name` when absent.
    pub label: Option<String>,
    /// Description override. Falls back to the semantic model description when absent.
    pub description: Option<String>,
}

impl WorldModelConfig {
    /// The world-model config, from whichever source the manager reads.
    ///
    /// `Ok(None)` means "no config" → show all entities. It is NOT the same as
    /// a replica with nothing to read: `ConfigManager` returns `NoSource`
    /// there, and mapping that to `None` would report "the tenant configured no
    /// display overrides" for a node that simply could not look.
    pub async fn resolve<S: oxy::config::DiskSlot>(
        config_manager: &oxy::config::ConfigManager<S>,
    ) -> Result<Option<Self>, String> {
        match config_manager.world_model_config().await {
            Ok(Some(value)) => serde_json::from_value::<Self>(value)
                .map(Some)
                .map_err(|e| format!("Failed to parse .world-model.yml: {e}")),
            Ok(None) => Ok(None),
            Err(e) if e.retryable() => {
                tracing::debug!(error = %e, "world-model config unavailable here");
                Ok(None)
            }
            Err(e) => Err(format!("world-model config read failed: {e}")),
        }
    }

    /// Load from `.world-model.yml` at the workspace root.
    /// Returns `Ok(None)` when the file does not exist — callers treat this as no-op.
    pub fn load(workspace_path: &Path) -> Result<Option<Self>, String> {
        let path = workspace_path.join(".world-model.yml");
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read .world-model.yml: {e}"))?;
        let config = serde_yaml::from_str::<Self>(&content)
            .map_err(|e| format!("Failed to parse .world-model.yml: {e}"))?;
        Ok(Some(config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_entity_config() {
        let yaml = r#"
entities:
  - id: orders
    label: "Orders"
    description: "Customer purchases"
    display_field: customer_name
    dimensions:
      - name: order_date
        label: "Order Date"
      - name: status
    measures:
      - name: revenue
        label: "Revenue"
        description: "Total amount"
"#;
        let cfg: WorldModelConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.entities.len(), 1);
        let e = &cfg.entities[0];
        assert_eq!(e.id, "orders");
        assert_eq!(e.label.as_deref(), Some("Orders"));
        assert_eq!(e.display_field.as_deref(), Some("customer_name"));
        let dims = e.dimensions.as_ref().unwrap();
        assert_eq!(dims.len(), 2);
        assert_eq!(dims[0].label.as_deref(), Some("Order Date"));
        assert!(dims[1].label.is_none());
    }

    #[test]
    fn absent_dimensions_is_none_not_empty() {
        // When the YAML has no `dimensions:` key, the field must be None (show all),
        // not Some([]) (show none). The Option<Vec> type enforces this.
        let yaml = "entities:\n  - id: orders\n    label: \"Orders\"\n";
        let cfg: WorldModelConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.entities[0].dimensions.is_none());
        assert!(cfg.entities[0].measures.is_none());
    }

    #[test]
    fn empty_entities_list_is_valid() {
        let cfg: WorldModelConfig = serde_yaml::from_str("entities: []\n").unwrap();
        assert!(cfg.entities.is_empty());
    }

    #[test]
    fn missing_entities_key_uses_default() {
        let cfg: WorldModelConfig = serde_yaml::from_str("{}\n").unwrap();
        assert!(cfg.entities.is_empty());
    }
}
