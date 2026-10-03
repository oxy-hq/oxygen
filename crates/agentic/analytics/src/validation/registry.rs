//! Rule registry — maps rule name strings to constructor functions.
//!
//! [`RuleRegistry::default_registry`] registers all 17 built-in rules, each under
//! the one stage it checks.
//! Call [`RuleRegistry::build_specified`] / [`build_solvable`] / [`build_solved`]
//! to instantiate a rule from a [`RuleEntry`].

use std::collections::HashMap;

use serde_json::Value;

use super::config::RuleEntry;
use super::rule::{SolvableRule, SolvedRule, SpecifiedRule};

// Error

/// Error constructing a rule from config.
#[derive(Debug)]
pub enum RegistryError {
    /// The rule name is not registered for this stage.
    UnknownRule { stage: &'static str, name: String },
    /// The rule's `params` could not be deserialized into the expected struct.
    InvalidParams { name: String, reason: String },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::UnknownRule { stage, name } => {
                write!(f, "unknown {stage} rule: '{name}'")
            }
            RegistryError::InvalidParams { name, reason } => {
                write!(f, "invalid params for rule '{name}': {reason}")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

// Constructor type aliases

type SpecifiedCtor = fn(&Value) -> Result<Box<dyn SpecifiedRule>, RegistryError>;
type SolvableCtor = fn(&Value) -> Result<Box<dyn SolvableRule>, RegistryError>;
type SolvedCtor = fn(&Value) -> Result<Box<dyn SolvedRule>, RegistryError>;

// Registry

/// Maps rule name strings to constructor function pointers.
///
/// Using `fn` pointers (not closures) keeps the registry itself allocation-free
/// and `const`-constructable.
pub struct RuleRegistry {
    specified: HashMap<&'static str, SpecifiedCtor>,
    solvable: HashMap<&'static str, SolvableCtor>,
    solved: HashMap<&'static str, SolvedCtor>,
}

impl RuleRegistry {
    /// Build the registry with all built-in rules registered.
    pub fn default_registry() -> Self {
        use super::solvable::{
            ColumnRefsValidRule, SpecTablesPresentRule, SqlSyntaxRule, TablesExistRule,
            TimeseriesOrderByCheckRule,
        };
        use super::solved::{
            DuplicateRowCheckRule, FreshnessCheckRule, NoNanInfRule, NonEmptyRule,
            NullRatioCheckRule, OutlierDetectionRule, ShapeMatchRule, TimeseriesDateCheckRule,
            TruncationWarningRule,
        };
        use super::specified::{FilterUnambiguousRule, JoinKeyExistsRule, MetricResolvesRule};

        let mut r = Self {
            specified: HashMap::new(),
            solvable: HashMap::new(),
            solved: HashMap::new(),
        };

        // ── specified ────────────────────────────────────────────────────────
        r.specified
            .insert("metric_resolves", MetricResolvesRule::from_params);
        r.specified
            .insert("join_key_exists", JoinKeyExistsRule::from_params);
        r.specified
            .insert("filter_unambiguous", FilterUnambiguousRule::from_params);

        // ── solvable ─────────────────────────────────────────────────────────
        r.solvable.insert("sql_syntax", SqlSyntaxRule::from_params);
        r.solvable
            .insert("tables_exist_in_catalog", TablesExistRule::from_params);
        r.solvable
            .insert("spec_tables_present", SpecTablesPresentRule::from_params);
        r.solvable
            .insert("column_refs_valid", ColumnRefsValidRule::from_params);
        r.solvable.insert(
            "timeseries_order_by_check",
            TimeseriesOrderByCheckRule::from_params,
        );

        // ── solved ───────────────────────────────────────────────────────────
        r.solved.insert("non_empty", NonEmptyRule::from_params);
        r.solved.insert("shape_match", ShapeMatchRule::from_params);
        r.solved.insert("no_nan_inf", NoNanInfRule::from_params);
        r.solved
            .insert("outlier_detection", OutlierDetectionRule::from_params);
        r.solved.insert(
            "timeseries_date_check",
            TimeseriesDateCheckRule::from_params,
        );
        r.solved
            .insert("truncation_warning", TruncationWarningRule::from_params);
        r.solved
            .insert("null_ratio_check", NullRatioCheckRule::from_params);
        r.solved
            .insert("duplicate_row_check", DuplicateRowCheckRule::from_params);
        r.solved
            .insert("freshness_check", FreshnessCheckRule::from_params);

        r
    }

    /// Instantiate a `specified`-stage rule from a [`RuleEntry`].
    pub fn build_specified(
        &self,
        entry: &RuleEntry,
    ) -> Result<Box<dyn SpecifiedRule>, RegistryError> {
        let ctor =
            self.specified
                .get(entry.name.as_str())
                .ok_or_else(|| RegistryError::UnknownRule {
                    stage: "specified",
                    name: entry.name.clone(),
                })?;
        ctor(&entry.params)
    }

    /// Instantiate a `solvable`-stage rule from a [`RuleEntry`].
    pub fn build_solvable(
        &self,
        entry: &RuleEntry,
    ) -> Result<Box<dyn SolvableRule>, RegistryError> {
        let ctor =
            self.solvable
                .get(entry.name.as_str())
                .ok_or_else(|| RegistryError::UnknownRule {
                    stage: "solvable",
                    name: entry.name.clone(),
                })?;
        ctor(&entry.params)
    }

    /// Instantiate a `solved`-stage rule from a [`RuleEntry`].
    pub fn build_solved(&self, entry: &RuleEntry) -> Result<Box<dyn SolvedRule>, RegistryError> {
        let ctor =
            self.solved
                .get(entry.name.as_str())
                .ok_or_else(|| RegistryError::UnknownRule {
                    stage: "solved",
                    name: entry.name.clone(),
                })?;
        ctor(&entry.params)
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::config::RuleEntry;

    fn entry(name: &str) -> RuleEntry {
        RuleEntry {
            name: name.to_string(),
            enabled: true,
            params: serde_json::Value::Null,
        }
    }

    #[test]
    fn registry_builds_all_specified_rules() {
        let reg = RuleRegistry::default_registry();
        for name in ["metric_resolves", "join_key_exists", "filter_unambiguous"] {
            assert!(reg.build_specified(&entry(name)).is_ok(), "failed: {name}");
        }
    }

    #[test]
    fn registry_builds_all_solvable_rules() {
        let reg = RuleRegistry::default_registry();
        for name in [
            "sql_syntax",
            "tables_exist_in_catalog",
            "spec_tables_present",
            "column_refs_valid",
            "timeseries_order_by_check",
        ] {
            assert!(reg.build_solvable(&entry(name)).is_ok(), "failed: {name}");
        }
    }

    #[test]
    fn registry_builds_all_solved_rules() {
        let reg = RuleRegistry::default_registry();
        for name in [
            "non_empty",
            "shape_match",
            "no_nan_inf",
            "outlier_detection",
            "timeseries_date_check",
            "truncation_warning",
            "null_ratio_check",
            "duplicate_row_check",
        ] {
            assert!(reg.build_solved(&entry(name)).is_ok(), "failed: {name}");
        }
    }

    #[test]
    fn registry_unknown_specified_rule() {
        let reg = RuleRegistry::default_registry();
        assert!(matches!(
            reg.build_specified(&entry("ghost_rule")),
            Err(RegistryError::UnknownRule {
                stage: "specified",
                ..
            })
        ));
    }

    #[test]
    fn registry_unknown_solvable_rule() {
        let reg = RuleRegistry::default_registry();
        assert!(matches!(
            reg.build_solvable(&entry("ghost_rule")),
            Err(RegistryError::UnknownRule {
                stage: "solvable",
                ..
            })
        ));
    }

    #[test]
    fn registry_unknown_solved_rule() {
        let reg = RuleRegistry::default_registry();
        assert!(matches!(
            reg.build_solved(&entry("ghost_rule")),
            Err(RegistryError::UnknownRule {
                stage: "solved",
                ..
            })
        ));
    }

    #[test]
    fn registry_outlier_rule_with_custom_params() {
        let reg = RuleRegistry::default_registry();
        let params = serde_json::json!({ "threshold_sigma": 3.0, "min_rows": 10 });
        let e = RuleEntry {
            name: "outlier_detection".into(),
            enabled: true,
            params,
        };
        assert!(reg.build_solved(&e).is_ok());
    }
}
