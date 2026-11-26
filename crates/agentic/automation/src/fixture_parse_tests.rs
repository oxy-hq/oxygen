//! The `oxy_example` fixture automations parse as the server runs them.
//!
//! Ported from `crates/core/tests/integration/semantic_variables.rs` and the
//! parse half of `crates/app/tests/platform/run.rs`'s automation case, which
//! spawned `oxy run` and asserted stderr held no parse failure ("Failed to
//! deserialize", "missing field", "invalid type", "Invalid variable"). `oxy run`
//! is `oxyc run` now (it starts the run on the deployment; the Rust stub was
//! deleted 2026-11-26), and the parse
//! those tests leaned on is this one: the server reads the compiled definition
//! back as YAML and deserializes it into [`AutomationConfig`]. `-v name=value`
//! is the run's `variables` now, so the override tests assert the precedence
//! [`effective_variables`] gives it (the Rust command dropped `-v` for an
//! automation, so those tests could only assert that nothing broke).
//!
//! Two of the old tests named `semantic-with-variables/workflow-example.automation.yml`,
//! which is not in the fixture: they passed on the "File not found" error. They
//! have no file to port to.

use serde_json::{Value, json};

use crate::config::{AutomationConfig, TaskType};
use crate::variables::effective_variables;

const SEMANTIC_VARIABLES: &str = include_str!(
    "../../../../tests/fixtures/oxy_example/procedures/semantic_variables.automation.yml"
);
const TABLE_VALUES: &str =
    include_str!("../../../../tests/fixtures/oxy_example/procedures/table_values.automation.yml");

fn parse(yaml: &str) -> AutomationConfig {
    serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("the fixture must parse: {e}"))
}

fn declared(config: &AutomationConfig) -> Value {
    serde_json::to_value(config.variables.clone().unwrap_or_default()).expect("a JSON map")
}

/// `run_automation_with_semantic_variables_validates`.
#[test]
fn semantic_variables_fixture_parses() {
    let config = parse(SEMANTIC_VARIABLES);
    assert_eq!(config.name, "simple_sales_report");
    assert_eq!(config.tasks.len(), 1);
    let task = &config.tasks[0];
    assert_eq!(task.name, "sales_data");
    let TaskType::SemanticQuery(body) = &task.task_type else {
        panic!(
            "sales_data is a semantic_query, got {}",
            task.task_type.name()
        );
    };
    assert_eq!(body["topic"], "fruit_business");
    assert_eq!(body["variables"]["orders_table"], "{{orders_table}}");
}

/// The declared default applies when the run names no value.
#[test]
fn semantic_variables_default_applies() {
    let config = parse(SEMANTIC_VARIABLES);
    let vars = effective_variables(Some(&declared(&config)), None);
    assert_eq!(vars, json!({ "orders_table": "orders" }));
}

/// `run_automation_with_semantic_variables_override_validates` and
/// `run_semantic_query_with_variable_precedence_validates`: a run's
/// `variables` (`oxyc run … -v orders_table=…`) win over the declared default.
#[test]
fn run_variables_override_the_declared_default() {
    let config = parse(SEMANTIC_VARIABLES);
    for table in ["custom_orders", "priority_orders"] {
        let run = json!({ "orders_table": table });
        let vars = effective_variables(Some(&declared(&config)), Some(&run));
        assert_eq!(vars, json!({ "orders_table": table }));
    }
}

/// `run_example_automation_ok`'s parse half: the tasks, in the order `oxyc run`
/// prints their results.
#[test]
fn table_values_fixture_parses() {
    let config = parse(TABLE_VALUES);
    let tasks: Vec<(&str, &str)> = config
        .tasks
        .iter()
        .map(|t| (t.name.as_str(), t.task_type.name()))
        .collect();
    assert_eq!(
        tasks,
        [
            ("intervals", "execute_sql"),
            ("schedules", "loop_sequential"),
            ("formatter", "formatter"),
        ]
    );
}
