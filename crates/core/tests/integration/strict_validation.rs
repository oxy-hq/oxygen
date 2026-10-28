//! Tests for strict validation with deny_unknown_fields.
//!
//! These tests verify that:
//! 1. Unknown fields are rejected with clear error messages
//! 2. Valid configs still parse correctly
//! 3. Flatten fields work correctly with deny_unknown_fields
//! 4. Empty required collections are caught by garde validation

use serde::de::DeserializeOwned;

fn parse_yaml<T: DeserializeOwned>(yaml: &str) -> Result<T, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

/// Helper to check if parsing rejects invalid config (unknown field OR missing required field)
/// When a struct has both a required field and deny_unknown_fields, using a typo
/// may result in either "unknown field" or "missing field" error depending on order.
fn assert_unknown_field_error_with_result<T>(result: Result<T, serde_yaml::Error>, field: &str) {
    match result {
        Ok(_) => panic!("Should reject unknown field '{field}', but parsing succeeded"),
        Err(err) => {
            let err_str = err.to_string();
            // Accept both "unknown field" and "missing field" errors
            // When using typo like "steps" instead of "tasks", serde may report:
            // - "unknown field `steps`" OR
            // - "missing field `tasks`" (if tasks is required)
            // Both indicate the config is invalid due to the typo
            assert!(
                err_str.contains("unknown field")
                    || err_str.contains("unknown variant")
                    || err_str.contains("missing field"),
                "Error should mention unknown or missing field for '{field}', got: {err_str}"
            );
        }
    }
}

// Config Tests

mod config_tests {
    use super::*;
    use oxy::config::model::Config;

    #[test]
    fn test_valid_config() {
        let yaml = r#"
models:
  - name: test_model
    vendor: openai
    model_ref: gpt-4
    key_var: OPENAI_API_KEY
databases: []
"#;
        let result: Result<Config, _> = parse_yaml(yaml);
        assert!(
            result.is_ok(),
            "Valid config should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_config_rejects_unknown_root_fields() {
        let yaml = r#"
models:
  - name: test_model
    vendor: openai
    model_ref: gpt-4
    key_var: OPENAI_API_KEY
databases: []
unknown_setting: true
"#;
        let result: Result<Config, _> = parse_yaml(yaml);
        assert_unknown_field_error_with_result(result, "unknown_setting");
    }

    #[test]
    fn test_config_rejects_typo_model_field() {
        let yaml = r#"
model:
  - name: test_model
    vendor: openai
    model_ref: gpt-4
    key_var: OPENAI_API_KEY
databases: []
"#;
        let result: Result<Config, _> = parse_yaml(yaml);
        // "model" instead of "models" should be caught
        assert_unknown_field_error_with_result(result, "model");
    }
}

// Automation Tests

mod automation_tests {
    use super::*;
    use oxy::config::model::Automation;

    #[test]
    fn test_valid_automation() {
        let yaml = r#"
tasks:
  - name: test_task
    type: agent
    agent_ref: test.agent.yml
    prompt: "test prompt"
"#;
        let result: Result<Automation, _> = parse_yaml(yaml);
        assert!(
            result.is_ok(),
            "Valid automation should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_automation_with_variables() {
        // Test that flatten for variables still works with deny_unknown_fields
        let yaml = r#"
tasks:
  - name: test_task
    type: agent
    agent_ref: test.agent.yml
    prompt: "test {{ my_var }}"
variables:
  my_var:
    type: string
    default: "hello"
"#;
        let result: Result<Automation, _> = parse_yaml(yaml);
        assert!(
            result.is_ok(),
            "Automation with variables should parse: {:?}",
            result.err()
        );

        let automation = result.unwrap();
        assert!(automation.variables.is_some(), "Variables should be parsed");
    }

    #[test]
    fn test_automation_rejects_steps_typo() {
        // This is the main use case - catching the common "steps" vs "tasks" typo
        let yaml = r#"
steps:
  - name: test_task
    type: agent
    agent_ref: test.agent.yml
    prompt: "test prompt"
"#;
        let result: Result<Automation, _> = parse_yaml(yaml);
        assert_unknown_field_error_with_result(result, "steps");
    }

    #[test]
    fn test_automation_rejects_unknown_fields() {
        let yaml = r#"
tasks:
  - name: test_task
    type: agent
    agent_ref: test.agent.yml
    prompt: "test prompt"
unknown_field: "should fail"
"#;
        let result: Result<Automation, _> = parse_yaml(yaml);
        assert_unknown_field_error_with_result(result, "unknown_field");
    }

    #[test]
    fn test_automation_with_all_valid_fields() {
        // Test all valid Automation fields to ensure they still work
        let yaml = r#"
tasks:
  - name: test_task
    type: agent
    agent_ref: test.agent.yml
    prompt: "test prompt"
description: "A test workflow"
consistency_prompt: "Check consistency"
"#;
        let result: Result<Automation, _> = parse_yaml(yaml);
        assert!(
            result.is_ok(),
            "Automation with all valid fields should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_automation_with_name_field() {
        // Test that the `name` field is accepted in YAML (for backwards compatibility).
        // The name is ignored during parsing and always derived from the filename at runtime.
        let yaml = r#"
name: my_workflow_name
tasks:
  - name: test_task
    type: agent
    agent_ref: test.agent.yml
    prompt: "test prompt"
"#;
        let result: Result<Automation, _> = parse_yaml(yaml);
        assert!(
            result.is_ok(),
            "Automation with name field should parse: {:?}",
            result.err()
        );

        // The name field should be parsed (defaults to empty if not set, but here it's set)
        let automation = result.unwrap();
        assert_eq!(automation.name, "my_workflow_name");
    }

    #[test]
    fn test_empty_tasks_parses_but_should_fail_validation() {
        // Empty tasks should parse at serde level, but fail garde validation
        let yaml = r#"
tasks: []
"#;
        let result: Result<Automation, _> = parse_yaml(yaml);
        // Serde parsing should succeed
        assert!(result.is_ok(), "Empty tasks should parse at serde level");

        // But garde validation should fail (tested after adding #[garde(length(min = 1))])
        // This test documents expected behavior after the garde changes
    }
}

// Integration Tests for Validation Context

mod garde_validation_tests {
    use super::*;
    use garde::Validate;
    use oxy::config::model::Config;

    /// Create a minimal valid Config for validation context
    fn create_test_config() -> Config {
        let yaml = r#"
models:
  - name: test_model
    vendor: openai
    model_ref: gpt-4
    key_var: OPENAI_API_KEY
databases:
  - name: test_db
    type: bigquery
    project: test_project
    credentials_path: /tmp/creds.json
"#;
        parse_yaml(yaml).expect("Test config should parse")
    }

    #[test]
    fn test_automation_empty_tasks_fails_garde_validation() {
        use oxy::config::model::Automation;

        let yaml = r#"
tasks: []
"#;
        let automation: Automation = parse_yaml(yaml).expect("Should parse at serde level");

        let config = create_test_config();
        let context = oxy::config::validate::ValidationContext { config };

        let result = automation.validate_with(&context);
        assert!(result.is_err(), "Empty tasks should fail garde validation");

        // Verify the error mentions length/min requirement
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("length") || err_str.contains("tasks"),
            "Error should mention tasks length validation, got: {err_str}"
        );
    }
}
