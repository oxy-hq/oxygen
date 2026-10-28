//! Per-step content hashing for "resume only unchanged steps."
//!
//! This module owns *what goes into* a step's identity hash. The hash function
//! itself lives in [`crate::hash`] (canonical SHA-256). The choice of inputs
//! is the load-bearing decision — false hits (reusing a stale output for a
//! step whose effective inputs changed) are silent correctness bugs, so the
//! input set is intentionally conservative.
//!
//! # Inputs
//!
//! Every step hashes:
//!
//! - **`step_config`** — the parsed [`TaskConfig`]. Canonicalized via JCS so
//!   YAML reformatting and key reordering don't invalidate the cache, while
//!   any semantic edit does.
//! - **`render_context`** — the full accumulated jinja context at the moment
//!   the step would run. Conservative on purpose: a future read-set tracker
//!   could narrow this, but missing a key here causes false hits.
//! - **`variables`** — automation-level variables (CLI args, sub-automation
//!   inputs).
//! - **`loop_idx`** — the iteration index, when this step is one iteration of
//!   a `loop_sequential` parent.
//! - **`sub_workflow_yaml_hash`** — for `SubAutomation` steps, the canonical
//!   hash of the child automation config. Editing the child must invalidate the
//!   parent step.
//! - **`code_version`** — the executor-side version constant for this step
//!   type. Bumped manually when executor logic changes meaningfully (a bug
//!   fix in the SQL renderer, a new templating extension, etc.). Keeps the
//!   cache safe across executor upgrades without forcing user action.
//!
//! # Per-step-type code versions
//!
//! Each task type has its own constant so a fix to (say) the omni executor
//! does not invalidate sql cache entries. Bump these when you change behavior
//! that affects step output.

use serde_json::Value;

use crate::config::TaskConfig;
use crate::hash::{HashError, canonical_hash_pairs};

// ── Per-step-type code versions ─────────────────────────────────────────────

pub const AGENT_CODE_VERSION: u32 = 1;
pub const SQL_CODE_VERSION: u32 = 1;
pub const SEMANTIC_QUERY_CODE_VERSION: u32 = 1;
pub const OMNI_QUERY_CODE_VERSION: u32 = 1;
pub const LOOKER_QUERY_CODE_VERSION: u32 = 1;
pub const FORMATTER_CODE_VERSION: u32 = 1;
pub const CONDITIONAL_CODE_VERSION: u32 = 1;
pub const LOOP_SEQUENTIAL_CODE_VERSION: u32 = 1;
pub const SUB_WORKFLOW_CODE_VERSION: u32 = 1;
pub const HTTP_REQUEST_CODE_VERSION: u32 = 1;
pub const AIRWAY_CODE_VERSION: u32 = 1;
pub const UNKNOWN_CODE_VERSION: u32 = 1;

/// Inputs that fully determine a step's effective identity.
#[derive(Debug)]
pub struct StepHashInputs<'a> {
    pub step_config: &'a TaskConfig,
    pub render_context: &'a Value,
    pub variables: Option<&'a Value>,
    pub loop_idx: Option<usize>,
    /// For `SubAutomation` steps only — canonical hash of the child config.
    pub sub_workflow_yaml_hash: Option<&'a str>,
}

/// Compute the canonical hash for a step.
///
/// Deterministic across runs and Rust versions; compares as a hex string.
pub fn compute_step_hash(inputs: &StepHashInputs<'_>) -> Result<String, HashError> {
    let code_version = code_version_for(&inputs.step_config.task_type);

    let step_config_value = serde_json::to_value(inputs.step_config)?;
    let variables_value = inputs.variables.cloned().unwrap_or(Value::Null);
    let loop_idx_value = inputs
        .loop_idx
        .map(|i| Value::from(i as u64))
        .unwrap_or(Value::Null);
    let sub_workflow_value = inputs
        .sub_workflow_yaml_hash
        .map(|s| Value::from(s.to_string()))
        .unwrap_or(Value::Null);
    let code_version_value = Value::from(code_version);

    canonical_hash_pairs(&[
        ("step_config", &step_config_value),
        ("render_context", inputs.render_context),
        ("variables", &variables_value),
        ("loop_idx", &loop_idx_value),
        ("sub_workflow_yaml_hash", &sub_workflow_value),
        ("code_version", &code_version_value),
    ])
}

fn code_version_for(t: &crate::config::TaskType) -> u32 {
    use crate::config::TaskType::*;
    match t {
        Agent(_) => AGENT_CODE_VERSION,
        Formatter(_) => FORMATTER_CODE_VERSION,
        Conditional(_) => CONDITIONAL_CODE_VERSION,
        LoopSequential(_) => LOOP_SEQUENTIAL_CODE_VERSION,
        SubAutomation(_) => SUB_WORKFLOW_CODE_VERSION,
        ExecuteSql(_) => SQL_CODE_VERSION,
        SemanticQuery(_) => SEMANTIC_QUERY_CODE_VERSION,
        OmniQuery(_) => OMNI_QUERY_CODE_VERSION,
        LookerQuery(_) => LOOKER_QUERY_CODE_VERSION,
        HttpRequest(_) => HTTP_REQUEST_CODE_VERSION,
        Airway(_) => AIRWAY_CODE_VERSION,
        Unknown => UNKNOWN_CODE_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FormatterConfig, TaskConfig, TaskType};
    use serde_json::json;

    fn formatter_step(name: &str, template: &str) -> TaskConfig {
        TaskConfig {
            name: name.into(),
            task_type: TaskType::Formatter(FormatterConfig {
                template: template.into(),
            }),
        }
    }

    #[test]
    fn deterministic() {
        let step = formatter_step("greet", "hello {{ x }}");
        let ctx = json!({"x": 1});
        let inputs = StepHashInputs {
            step_config: &step,
            render_context: &ctx,
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        };
        let h1 = compute_step_hash(&inputs).unwrap();
        let h2 = compute_step_hash(&inputs).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn config_change_invalidates() {
        let ctx = json!({});
        let a = formatter_step("greet", "hello {{ x }}");
        let b = formatter_step("greet", "HOLA {{ x }}");

        let h_a = compute_step_hash(&StepHashInputs {
            step_config: &a,
            render_context: &ctx,
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        let h_b = compute_step_hash(&StepHashInputs {
            step_config: &b,
            render_context: &ctx,
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        assert_ne!(h_a, h_b);
    }

    #[test]
    fn render_context_change_invalidates() {
        let step = formatter_step("greet", "hello {{ x }}");
        let h_a = compute_step_hash(&StepHashInputs {
            step_config: &step,
            render_context: &json!({"x": 1}),
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        let h_b = compute_step_hash(&StepHashInputs {
            step_config: &step,
            render_context: &json!({"x": 2}),
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        assert_ne!(h_a, h_b);
    }

    #[test]
    fn loop_idx_distinguishes_iterations() {
        let step = formatter_step("each", "i={{ loop_idx }}");
        let ctx = json!({});
        let h0 = compute_step_hash(&StepHashInputs {
            step_config: &step,
            render_context: &ctx,
            variables: None,
            loop_idx: Some(0),
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        let h1 = compute_step_hash(&StepHashInputs {
            step_config: &step,
            render_context: &ctx,
            variables: None,
            loop_idx: Some(1),
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        assert_ne!(h0, h1);
    }

    #[test]
    fn sub_automation_hash_propagates() {
        // Same parent step config; child automation hash differs → step hash differs.
        let step = TaskConfig {
            name: "child".into(),
            task_type: TaskType::SubAutomation(crate::config::SubAutomationConfig {
                src: "child.automation.yml".into(),
                variables: None,
                resolved_tasks: vec![],
            }),
        };
        let ctx = json!({});
        let h_v1 = compute_step_hash(&StepHashInputs {
            step_config: &step,
            render_context: &ctx,
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: Some("aaaa"),
        })
        .unwrap();
        let h_v2 = compute_step_hash(&StepHashInputs {
            step_config: &step,
            render_context: &ctx,
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: Some("bbbb"),
        })
        .unwrap();
        assert_ne!(h_v1, h_v2);
    }
}
