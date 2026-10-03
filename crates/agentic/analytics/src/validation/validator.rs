//! [`Validator`] — holds the active rule lists for all three pipeline stages
//! and exposes the `validate_*` entry points.
//!
//! Build from a [`ValidationConfig`] parsed out of the agent YAML — which then
//! runs exactly the rules it lists — or use [`Validator::default_validator`] to
//! run the 15 default rules with their default params.
//!
//! # Example
//! ```rust,ignore
//! // From agent YAML config (`validation:` absent → the default set):
//! let validator = match &agent_config.validation {
//!     Some(cfg) => Validator::from_config(cfg)?,
//!     None => Validator::default_validator(),
//! };
//! validator.validate_specified(&spec, &catalog)?;
//! validator.validate_solvable(&sql, &spec, &catalog)?;
//! validator.validate_solved(&result, &spec)?;
//! ```

use crate::semantic::SemanticCatalog;
use crate::{AnalyticsError, AnalyticsResult, QuerySpec};

use super::config::ValidationConfig;
use super::registry::{RegistryError, RuleRegistry};
use super::rule::{SolvableCtx, SolvableRule, SolvedCtx, SolvedRule, SpecifiedCtx, SpecifiedRule};

// Validator

/// Holds the active, ordered rule lists for all three validation stages.
///
/// Rules are instantiated once at build time from a [`ValidationConfig`] and
/// then reused for every call.  Rules run in declaration order; the first
/// error encountered is returned immediately (fail-fast).
pub struct Validator {
    specified: Vec<Box<dyn SpecifiedRule>>,
    solvable: Vec<Box<dyn SolvableRule>>,
    solved: Vec<Box<dyn SolvedRule>>,
}

impl Validator {
    /// Build from a [`ValidationConfig`] using the default rule registry.
    pub fn from_config(cfg: &ValidationConfig) -> Result<Self, RegistryError> {
        Self::from_config_with_registry(cfg, &RuleRegistry::default_registry())
    }

    /// Build with a custom registry (useful for testing with rule doubles).
    pub fn from_config_with_registry(
        cfg: &ValidationConfig,
        registry: &RuleRegistry,
    ) -> Result<Self, RegistryError> {
        let specified = cfg
            .rules
            .specified
            .iter()
            .filter(|e| e.enabled)
            .map(|e| registry.build_specified(e))
            .collect::<Result<Vec<_>, _>>()?;

        let solvable = cfg
            .rules
            .solvable
            .iter()
            .filter(|e| e.enabled)
            .map(|e| registry.build_solvable(e))
            .collect::<Result<Vec<_>, _>>()?;

        let solved = cfg
            .rules
            .solved
            .iter()
            .filter(|e| e.enabled)
            .map(|e| registry.build_solved(e))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            specified,
            solvable,
            solved,
        })
    }

    /// Build the default validator: the 15 default rules enabled with their
    /// default parameters.  Equivalent to calling `Validator::from_config` with
    /// [`ValidationConfig::default_all_rules`].
    pub fn default_validator() -> Self {
        Self::from_config(&ValidationConfig::default_all_rules())
            .expect("default_all_rules must always be valid")
    }

    // ── Stage entry points ────────────────────────────────────────────────────

    /// Run all active `specified`-stage rules.
    pub fn validate_specified(
        &self,
        spec: &QuerySpec,
        catalog: &SemanticCatalog,
    ) -> Result<(), AnalyticsError> {
        let ctx = SpecifiedCtx { spec, catalog };
        for rule in &self.specified {
            rule.check(&ctx)?;
        }
        Ok(())
    }

    /// Run all active `solvable`-stage rules.
    pub fn validate_solvable(
        &self,
        sql: &str,
        spec: &QuerySpec,
        catalog: &SemanticCatalog,
    ) -> Result<(), AnalyticsError> {
        let ctx = SolvableCtx { sql, spec, catalog };
        for rule in &self.solvable {
            rule.check(&ctx)?;
        }
        Ok(())
    }

    /// Run all active `solved`-stage rules.
    pub fn validate_solved(
        &self,
        result: &AnalyticsResult,
        spec: &QuerySpec,
    ) -> Result<(), AnalyticsError> {
        let ctx = SolvedCtx { result, spec };
        for rule in &self.solved {
            rule.check(&ctx)?;
        }
        Ok(())
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::config::{RuleEntry, RuleSetConfig, ValidationConfig};
    use crate::validation::test_fixtures::*;
    use crate::{AnalyticsError, QuerySpec, ResultShape};

    fn entry(name: &str) -> RuleEntry {
        RuleEntry {
            name: name.to_string(),
            enabled: true,
            params: serde_json::Value::Null,
        }
    }

    fn disabled_entry(name: &str) -> RuleEntry {
        RuleEntry {
            name: name.to_string(),
            enabled: false,
            params: serde_json::Value::Null,
        }
    }

    // ── Validator::default_validator ──────────────────────────────────────────

    #[test]
    fn default_validator_specified_happy_path() {
        let v = Validator::default_validator();
        assert_eq!(
            v.validate_specified(&make_spec(), &sample_catalog()),
            Ok(())
        );
    }

    #[test]
    fn default_validator_solvable_happy_path() {
        let v = Validator::default_validator();
        let sql = "SELECT customers.region, SUM(orders.revenue) \
                   FROM orders JOIN customers ON orders.customer_id = customers.customer_id \
                   GROUP BY customers.region";
        assert_eq!(
            v.validate_solvable(sql, &make_spec(), &sample_catalog()),
            Ok(())
        );
    }

    #[test]
    fn default_validator_solved_happy_path() {
        let v = Validator::default_validator();
        assert_eq!(
            v.validate_solved(
                &timeseries_result(),
                &QuerySpec {
                    expected_result_shape: ResultShape::TimeSeries,
                    ..make_spec()
                }
            ),
            Ok(())
        );
    }

    // ── Disabling rules ───────────────────────────────────────────────────────

    #[test]
    fn disabled_outlier_rule_allows_statistical_outlier() {
        // Build a config with outlier_detection disabled.
        let cfg = ValidationConfig {
            rules: RuleSetConfig {
                specified: vec![],
                solvable: vec![],
                solved: vec![
                    entry("non_empty"),
                    entry("shape_match"),
                    entry("no_nan_inf"),
                    disabled_entry("outlier_detection"), // <-- disabled
                ],
            },
        };
        let v = Validator::from_config(&cfg).unwrap();

        let spec = QuerySpec {
            expected_result_shape: ResultShape::Series,
            ..make_spec()
        };
        let mut values = vec![100.0_f64; 50];
        values.push(100_000.0); // extreme outlier

        // Would normally trigger ValueAnomaly, but rule is disabled.
        assert_eq!(v.validate_solved(&series_result(&values), &spec), Ok(()));
    }

    // ── Custom params ─────────────────────────────────────────────────────────

    #[test]
    fn custom_outlier_threshold_triggers_on_tighter_sigma() {
        let params = serde_json::json!({ "threshold_sigma": 1.0, "min_rows": 4 });
        let cfg = ValidationConfig {
            rules: RuleSetConfig {
                specified: vec![],
                solvable: vec![],
                solved: vec![
                    entry("non_empty"),
                    entry("shape_match"),
                    entry("no_nan_inf"),
                    RuleEntry {
                        name: "outlier_detection".into(),
                        enabled: true,
                        params,
                    },
                ],
            },
        };
        let v = Validator::from_config(&cfg).unwrap();

        let spec = QuerySpec {
            expected_result_shape: ResultShape::Series,
            ..make_spec()
        };
        // Values slightly spread — would pass default threshold (5σ) but not 1σ.
        let result = series_result(&[10.0, 10.0, 10.0, 10.0, 100.0]);
        assert!(matches!(
            v.validate_solved(&result, &spec),
            Err(AnalyticsError::ValueAnomaly { .. })
        ));
    }

    // ── Unknown rule name → RegistryError ─────────────────────────────────────

    #[test]
    fn unknown_rule_name_returns_registry_error() {
        let cfg = ValidationConfig {
            rules: RuleSetConfig {
                specified: vec![entry("ghost_rule")],
                solvable: vec![],
                solved: vec![],
            },
        };
        assert!(matches!(
            Validator::from_config(&cfg),
            Err(RegistryError::UnknownRule { .. })
        ));
    }

    // ── YAML round-trip ───────────────────────────────────────────────────────

    #[test]
    fn validator_built_from_yaml_config() {
        let yaml = r#"
rules:
  specified:
    - name: metric_resolves
      enabled: true
    - name: join_key_exists
      enabled: true
  solvable:
    - name: sql_syntax
      enabled: true
  solved:
    - name: non_empty
      enabled: true
    - name: outlier_detection
      enabled: true
      threshold_sigma: 3.0
      min_rows: 5
"#;
        let cfg: ValidationConfig = serde_yaml::from_str(yaml).unwrap();
        let v = Validator::from_config(&cfg).unwrap();
        assert_eq!(v.specified.len(), 2);
        assert_eq!(v.solvable.len(), 1);
        assert_eq!(v.solved.len(), 2);
    }
}
