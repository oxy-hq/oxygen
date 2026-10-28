//! Automation features the platform no longer runs, refused at compile.
//!
//! Three step shapes only ever worked on the one process that held a working
//! copy of the repository (the Factory), and a step that works on one node and
//! writes into a directory nobody can read on every other is worse than a step
//! that is refused with its name:
//!
//! - `export:` wrote a task's result into the workspace directory.
//! - `cache:` wrote a task's result into the workspace directory and, on a
//!   later run, read it back in place of running the step.
//! - `agent_ref: __builder__` runs the file-editing Builder, which edits the
//!   working copy. Nothing edits the working copy once the IDE goes
//!   (`internal-docs/factory-retirement.md`, decision 1).
//!
//! The check runs on the parsed YAML, not on a typed config: this crate does
//! not depend on the automation crate, and the compiled definition is stored
//! as the same JSON the file held.
//!
//! With [`REFUSE_RETIRED_STEPS_ENV`] set, a failed file fails the revision
//! (it is recorded as a [`FailureKind::Validation`] failure), so the previous
//! revision keeps serving. That is the point: the alternative is promoting a
//! revision whose steps silently stop writing a file a downstream step reads.
//!
//! Without it the compile only warns, naming each step. The runtime already
//! ignores the keys, so the file compiles and runs without writing. It is a
//! rollout gate, as `OXY_COMPILE_REFUSE_FILE_CREDENTIALS` is: a workspace that
//! still carries one of these keys (every Demo workspace seeded from the
//! demo project before its `export:` blocks were removed) would otherwise stop
//! promoting on its first compile after deploy.
//!
//! [`FailureKind::Validation`]: crate::FailureKind::Validation

use serde_json::Value;

/// The environment variable that turns the refusal on. Off (unset, or any value
/// but `1`) the compile warns instead.
pub(crate) const REFUSE_RETIRED_STEPS_ENV: &str = "OXY_COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS";

/// Whether [`REFUSE_RETIRED_STEPS_ENV`] is set to `1`.
pub(crate) fn refusal_enabled() -> bool {
    std::env::var(REFUSE_RETIRED_STEPS_ENV).is_ok_and(|v| v == "1")
}

/// The agent id that routes a step to the Builder.
const BUILDER_AGENT_REF: &str = "__builder__";

/// One refused step, named so the author can find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetiredFeature {
    /// `tasks[1].tasks[0]` style location of the step in the file.
    pub location: String,
    /// The step's `name:`, when it has one.
    pub task: Option<String>,
    /// Why it is refused, as one sentence for the author.
    pub reason: &'static str,
}

impl RetiredFeature {
    fn describe(&self) -> String {
        match &self.task {
            Some(name) => format!("step '{name}' ({}): {}", self.location, self.reason),
            None => format!("step {}: {}", self.location, self.reason),
        }
    }
}

const EXPORT_REASON: &str = "`export:` writes into the workspace directory, which the platform no \
     longer keeps. Remove it; to hand a result to another system use an `http_request` step, or \
     keep the result in the warehouse with an `execute_sql` or `airway` step";

const CACHE_REASON: &str = "`cache:` reads and writes a file in the workspace directory, which the \
     platform no longer keeps. Remove it; a result worth keeping belongs in the warehouse";

const BUILDER_REASON: &str = "`agent_ref: __builder__` runs the file-editing Builder, which has no \
     working copy to edit once the IDE is gone. Remove the step; content is edited in the \
     repository and pushed";

/// Every step in an automation file that uses a retired feature.
///
/// Walks `tasks`, and the task lists nested in a `loop_sequential` step
/// (`tasks`) and a `conditional` step (`conditions[].tasks`, `else`). A step
/// that delegates to another automation (`type: workflow`) is that file's
/// business: it is compiled, and checked, on its own.
pub(crate) fn retired_features(automation: &Value) -> Vec<RetiredFeature> {
    let mut found = Vec::new();
    if let Some(tasks) = automation.get("tasks") {
        walk_tasks(tasks, "tasks", &mut found);
    }
    found
}

/// The failure message for a file, or `None` when it uses none of them.
pub(crate) fn failure_message(automation: &Value) -> Option<String> {
    let found = retired_features(automation);
    if found.is_empty() {
        return None;
    }
    Some(
        found
            .iter()
            .map(RetiredFeature::describe)
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn walk_tasks(tasks: &Value, location: &str, found: &mut Vec<RetiredFeature>) {
    let Some(tasks) = tasks.as_array() else {
        return;
    };
    for (i, task) in tasks.iter().enumerate() {
        let here = format!("{location}[{i}]");
        let Some(map) = task.as_object() else {
            continue;
        };
        let name = map.get("name").and_then(Value::as_str).map(str::to_string);
        let mut refuse = |reason: &'static str| {
            found.push(RetiredFeature {
                location: here.clone(),
                task: name.clone(),
                reason,
            });
        };
        if map.get("export").is_some_and(|v| !v.is_null()) {
            refuse(EXPORT_REASON);
        }
        if map.get("cache").is_some_and(|v| !v.is_null()) {
            refuse(CACHE_REASON);
        }
        if map
            .get("agent_ref")
            .and_then(Value::as_str)
            .is_some_and(|r| r.trim() == BUILDER_AGENT_REF)
        {
            refuse(BUILDER_REASON);
        }

        if let Some(nested) = map.get("tasks") {
            walk_tasks(nested, &format!("{here}.tasks"), found);
        }
        if let Some(conditions) = map.get("conditions").and_then(Value::as_array) {
            for (c, condition) in conditions.iter().enumerate() {
                if let Some(nested) = condition.get("tasks") {
                    walk_tasks(nested, &format!("{here}.conditions[{c}].tasks"), found);
                }
            }
        }
        if let Some(nested) = map.get("else") {
            walk_tasks(nested, &format!("{here}.else"), found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn yaml(s: &str) -> Value {
        serde_yaml::from_str(s).expect("test yaml parses")
    }

    #[test]
    fn a_plain_automation_passes() {
        let v = yaml(
            r#"
name: ok
tasks:
  - name: q
    type: execute_sql
    database: db
    sql_query: select 1
  - name: ask
    type: agent
    agent_ref: agents/sales.agentic.yml
    prompt: hi
"#,
        );
        assert_eq!(failure_message(&v), None);
    }

    #[test]
    fn export_is_refused_and_the_step_is_named() {
        let v = yaml(
            r#"
tasks:
  - name: dump
    type: execute_sql
    database: db
    sql_query: select 1
    export:
      path: out/x.csv
      format: csv
"#,
        );
        let msg = failure_message(&v).expect("refused");
        assert!(msg.contains("step 'dump' (tasks[0])"), "{msg}");
        assert!(msg.contains("`export:`"), "{msg}");
    }

    #[test]
    fn cache_is_refused() {
        let v = yaml(
            r#"
tasks:
  - name: gen
    type: agent
    agent_ref: agents/a.agentic.yml
    prompt: x
    cache:
      path: out/x.sql
"#,
        );
        let msg = failure_message(&v).expect("refused");
        assert!(msg.contains("`cache:`"), "{msg}");
    }

    #[test]
    fn a_builder_step_is_refused() {
        let v = yaml(
            r#"
tasks:
  - name: edit
    type: agent
    agent_ref: __builder__
    prompt: fix the file
"#,
        );
        let msg = failure_message(&v).expect("refused");
        assert!(msg.contains("step 'edit'"), "{msg}");
        assert!(msg.contains("__builder__"), "{msg}");
    }

    /// A step nested in a loop, a branch of a conditional, or an `else` is
    /// still a step; refusing only the top level would let the same feature
    /// back in one indent deeper.
    #[test]
    fn nested_steps_are_checked_and_located() {
        let v = yaml(
            r#"
tasks:
  - name: each
    type: loop_sequential
    values: [1, 2]
    tasks:
      - name: inner
        type: formatter
        template: x
        export: { path: out/a.txt, format: txt }
  - name: branch
    type: conditional
    conditions:
      - if: "{{ true }}"
        tasks:
          - name: b1
            type: agent
            agent_ref: __builder__
            prompt: p
    else:
      - name: b2
        type: formatter
        template: y
        cache: { path: out/b.txt }
"#,
        );
        let found = retired_features(&v);
        let at: Vec<_> = found.iter().map(|f| f.location.as_str()).collect();
        assert_eq!(
            at,
            [
                "tasks[0].tasks[0]",
                "tasks[1].conditions[0].tasks[0]",
                "tasks[1].else[0]"
            ]
        );
    }

    #[test]
    fn every_problem_in_a_file_is_reported_not_just_the_first() {
        let v = yaml(
            r#"
tasks:
  - name: a
    type: formatter
    template: x
    export: { path: o, format: txt }
    cache: { path: c }
"#,
        );
        assert_eq!(retired_features(&v).len(), 2);
    }

    /// A null value is "not set"; it is not a use of the feature.
    #[test]
    fn a_null_export_or_cache_is_not_a_use() {
        let v = yaml(
            r#"
tasks:
  - name: a
    type: formatter
    template: x
    export: null
    cache: null
"#,
        );
        assert_eq!(failure_message(&v), None);
    }

    #[test]
    fn a_key_named_export_inside_step_data_is_not_a_use() {
        let v = json!({
            "tasks": [{
                "name": "call",
                "type": "http_request",
                "url": "https://example.com",
                "body": { "export": "yes", "cache": "no" }
            }]
        });
        assert_eq!(failure_message(&v), None);
    }

    /// A sub-automation step points at another file; that file is checked when
    /// it is compiled, so this one is not refused for what it points at.
    #[test]
    fn a_sub_automation_reference_is_not_followed() {
        let v = yaml(
            r#"
tasks:
  - name: child
    type: workflow
    src: other.automation.yml
"#,
        );
        assert_eq!(failure_message(&v), None);
    }
}
