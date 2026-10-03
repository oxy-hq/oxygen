//! Declarative validation configuration.
//!
//! Lives as an optional `validation:` section inside the agent YAML file:
//!
//! ```yaml
//! validation:
//!   rules:
//!     specified:
//!       - name: metric_resolves
//!         enabled: true
//!       - name: join_key_exists
//!         enabled: true
//!       - name: filter_unambiguous
//!         enabled: true
//!     solvable:
//!       - name: sql_syntax
//!         enabled: true
//!         dialect: postgresql
//!       - name: tables_exist_in_catalog
//!         enabled: true
//!       - name: spec_tables_present
//!         enabled: true
//!       - name: column_refs_valid
//!         enabled: true
//!     solved:
//!       - name: non_empty
//!         enabled: true
//!       - name: shape_match
//!         enabled: true
//!       - name: no_nan_inf
//!         enabled: true
//!       - name: outlier_detection
//!         enabled: true
//!         threshold_sigma: 5.0
//!         min_rows: 4
//!       - name: timeseries_date_check
//!         enabled: true
//! ```
//!
//! When the `validation:` key is absent from the YAML, [`ValidationConfig::default_all_rules`]
//! is used: the 15 default rules (every registered rule except `shape_match` and
//! `timeseries_date_check`) with their default parameters. When the key is present it
//! replaces that set — only the rules it lists run, and a stage with no entries runs
//! none. The example above lists two rules that are not in the default set and omits
//! five that are.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Top-level

/// Validation section of the agent YAML config.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ValidationConfig {
    #[serde(default)]
    pub rules: RuleSetConfig,
}

/// Rules grouped by pipeline stage.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct RuleSetConfig {
    /// Rules applied after the **Specify** stage.
    #[serde(default)]
    pub specified: Vec<RuleEntry>,
    /// Rules applied after the **Solve** stage.
    #[serde(default)]
    pub solvable: Vec<RuleEntry>,
    /// Rules applied after the **Execute** stage.
    #[serde(default)]
    pub solved: Vec<RuleEntry>,
}

/// One entry in a rule list.
///
/// Extra TOML/YAML keys beyond `name` and `enabled` are collected into
/// `params` via `#[serde(flatten)]`.  Each rule's constructor deserialises
/// `params` into its own typed struct using [`serde_json::from_value`].
///
/// # Example YAML
/// ```yaml
/// - name: outlier_detection
///   enabled: true
///   threshold_sigma: 3.0
///   min_rows: 6
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuleEntry {
    pub name: String,
    #[serde(default = "bool_true")]
    pub enabled: bool,
    /// Rule-specific configuration keys (flattened from the same YAML map).
    #[serde(flatten, default)]
    pub params: serde_json::Value,
}

fn bool_true() -> bool {
    true
}

// Per-rule parameter structs

/// Parameters for the `sql_syntax` rule.
#[derive(Debug, Clone, Deserialize)]
pub struct SqlSyntaxParams {
    /// SQL dialect for the `sqlparser` parser.
    /// Accepted values: `"generic"` (default), `"ansi"`, `"postgresql"`,
    /// `"mysql"`, `"bigquery"`, `"duckdb"`, `"snowflake"`.
    #[serde(default = "default_dialect")]
    pub dialect: String,
}

fn default_dialect() -> String {
    "generic".to_string()
}

impl Default for SqlSyntaxParams {
    fn default() -> Self {
        Self {
            dialect: default_dialect(),
        }
    }
}

/// Parameters for the `outlier_detection` rule.
#[derive(Debug, Clone, Deserialize)]
pub struct OutlierDetectionParams {
    /// Number of standard deviations from the column mean that triggers a
    /// [`ValueAnomaly`](crate::AnalyticsError::ValueAnomaly).
    /// Default: `5.0`.
    #[serde(default = "default_sigma")]
    pub threshold_sigma: f64,
    /// Minimum number of numeric rows required before outlier detection runs.
    /// Default: `4`.
    #[serde(default = "default_min_rows")]
    pub min_rows: usize,
}

fn default_sigma() -> f64 {
    5.0
}
fn default_min_rows() -> usize {
    4
}

impl Default for OutlierDetectionParams {
    fn default() -> Self {
        Self {
            threshold_sigma: default_sigma(),
            min_rows: default_min_rows(),
        }
    }
}

/// Parameters for the `null_ratio_check` rule.
#[derive(Debug, Clone, Deserialize)]
pub struct NullRatioCheckParams {
    /// Maximum allowed proportion of NULL values in a metric column before the
    /// check fails.  Range: `0.0` (any NULL fails) to `1.0` (only 100% NULL
    /// fails).  Default: `0.5` (50%).
    #[serde(default = "default_null_threshold")]
    pub threshold: f64,
}

fn default_null_threshold() -> f64 {
    0.5
}

impl Default for NullRatioCheckParams {
    fn default() -> Self {
        Self {
            threshold: default_null_threshold(),
        }
    }
}

/// Parameters for the `duplicate_row_check` rule.
#[derive(Debug, Clone, Deserialize)]
pub struct DuplicateRowCheckParams {
    /// Maximum fraction of rows that may be duplicates before the check fails.
    /// Range: `0.0` (any duplicate fails) to `1.0` (never fails).
    /// Default: `0.1` (10%).
    #[serde(default = "default_max_duplicate_ratio")]
    pub max_duplicate_ratio: f64,
}

fn default_max_duplicate_ratio() -> f64 {
    0.1
}

impl Default for DuplicateRowCheckParams {
    fn default() -> Self {
        Self {
            max_duplicate_ratio: default_max_duplicate_ratio(),
        }
    }
}

// ValidationConfig helpers

impl ValidationConfig {
    /// Build the default configuration: 15 of the 17 registered rules, enabled,
    /// with their default parameters. `shape_match` and `timeseries_date_check`
    /// are registered but not in it.
    ///
    /// Used by [`Validator::default_validator`](crate::validation::validator::Validator::default_validator)
    /// when no `validation:` section is present in the agent YAML.
    pub fn default_all_rules() -> Self {
        let null = serde_json::Value::Null;

        fn entry(name: &str) -> RuleEntry {
            RuleEntry {
                name: name.to_string(),
                enabled: true,
                params: serde_json::Value::Null,
            }
        }

        ValidationConfig {
            rules: RuleSetConfig {
                specified: vec![
                    entry("metric_resolves"),
                    entry("join_key_exists"),
                    entry("filter_unambiguous"),
                ],
                solvable: vec![
                    entry("sql_syntax"),
                    entry("tables_exist_in_catalog"),
                    entry("spec_tables_present"),
                    entry("column_refs_valid"),
                    entry("timeseries_order_by_check"),
                ],
                solved: vec![
                    entry("non_empty"),
                    entry("truncation_warning"),
                    entry("no_nan_inf"),
                    RuleEntry {
                        name: "outlier_detection".to_string(),
                        enabled: true,
                        params: null,
                    },
                    entry("null_ratio_check"),
                    entry("duplicate_row_check"),
                    entry("freshness_check"),
                ],
            },
        }
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_all_rules_has_15_rules() {
        let cfg = ValidationConfig::default_all_rules();
        assert_eq!(cfg.rules.specified.len(), 3);
        assert_eq!(cfg.rules.solvable.len(), 5);
        assert_eq!(cfg.rules.solved.len(), 7);
    }

    #[test]
    fn config_yaml_round_trip() {
        let yaml = r#"
rules:
  specified:
    - name: metric_resolves
      enabled: true
    - name: join_key_exists
      enabled: false
  solvable:
    - name: sql_syntax
      enabled: true
      dialect: postgresql
  solved:
    - name: outlier_detection
      enabled: true
      threshold_sigma: 3.0
      min_rows: 6
"#;
        let cfg: ValidationConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.rules.specified.len(), 2);
        assert!(!cfg.rules.specified[1].enabled);
        assert_eq!(cfg.rules.solvable.len(), 1);
        assert_eq!(cfg.rules.solved.len(), 1);

        // Check param extraction
        let params: OutlierDetectionParams =
            serde_json::from_value(cfg.rules.solved[0].params.clone()).unwrap();
        assert_eq!(params.threshold_sigma, 3.0);
        assert_eq!(params.min_rows, 6);
    }

    #[test]
    fn config_enabled_defaults_to_true() {
        let yaml = "rules:\n  specified:\n    - name: metric_resolves\n";
        let cfg: ValidationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.rules.specified[0].enabled);
    }

    #[test]
    fn config_sql_syntax_default_dialect() {
        let params = SqlSyntaxParams::default();
        assert_eq!(params.dialect, "generic");
    }

    #[test]
    fn config_outlier_defaults() {
        let params = OutlierDetectionParams::default();
        assert_eq!(params.threshold_sigma, 5.0);
        assert_eq!(params.min_rows, 4);
    }
}
