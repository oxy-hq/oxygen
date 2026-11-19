//! Frozen reader for builder runs recorded before the builder was removed.
//!
//! Old `agentic_runs` rows with `source_type = 'builder'` still render through
//! the event registry. Nothing writes these events any more: this module only
//! deserialises and re-serialises what is already stored, and carries the two
//! summary mappings the core row processor uses for those runs.

use std::sync::Arc;

use agentic_runtime::event_registry::{DomainHandler, domain_row_processor};
use serde::{Deserialize, Serialize};

/// The builder copilot's stored events. Serde shape only.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
enum BuilderEvent {
    FileChangePending {
        file_path: String,
        description: String,
        new_content: String,
        #[serde(default)]
        old_content: String,
    },
    FileChanged {
        file_path: String,
        description: String,
        new_content: String,
        #[serde(default)]
        old_content: String,
        #[serde(default)]
        is_deletion: bool,
    },
    ToolUsed {
        tool_name: String,
        summary: String,
    },
}

/// Map a lower-cased builder pipeline state name to a step summary.
fn builder_step_summary(state: &str) -> Option<String> {
    let s = match state {
        "clarifying" => "Preparing the builder request",
        "specifying" => "Structuring the builder task",
        "solving" => "Working",
        "executing" => "Applying output",
        "interpreting" => "Summarizing",
        "diagnosing" => "Recovering from an error",
        _ => return None,
    };
    Some(s.to_string())
}

/// Map a builder tool name to a step summary.
fn builder_tool_summary(tool: &str, input: &serde_json::Value) -> Option<String> {
    if matches!(tool, "write_file" | "edit_file" | "delete_file")
        && let Some(desc) = input["description"].as_str().filter(|s| !s.is_empty())
    {
        return Some(desc.to_string());
    }

    let s = match tool {
        "search_files" => "Searching files",
        "read_file" => "Reading a file",
        "search_text" => "Searching file contents",
        "file_change" => "Making a file change",
        "write_file" => "Writing a file",
        "edit_file" => "Editing a file",
        "delete_file" => "Deleting a file",
        "manage_directory" => "Managing a directory",
        "validate_project" => "Validating objects",
        "lookup_schema" => "Looking up schema details",
        "run_tests" => "Running tests",
        "execute_sql" => "Running SQL",
        "semantic_query" => "Running a semantic query",
        "ask_user" => "Asking for user input",
        _ => return None,
    };
    Some(s.to_string())
}

/// The `"builder"` registration for the runtime's event registry.
pub(crate) fn event_handler() -> DomainHandler {
    DomainHandler {
        processor: domain_row_processor::<BuilderEvent>(),
        summary_fn: Arc::new(builder_step_summary),
        tool_summary_fn: Arc::new(builder_tool_summary),
        // Builder events stream as standalone events; no StepEnd enrichment.
        should_accumulate: Some(Arc::new(|_| false)),
    }
}

#[cfg(test)]
mod tests {
    use super::event_handler;
    use serde_json::{Value, json};

    /// Golden expectations taken from the builder crate's `ui.rs` and
    /// `events.rs` at 0921917c6, before the crate was deleted. Each row is `(event_type, stored payload,
    /// expected processed payload)`, in the shape `crud::batch_insert_events`
    /// writes: the event type plus the payload with the tag split off.
    fn golden_rows() -> Vec<(&'static str, Value, Value)> {
        vec![
            (
                "file_change_pending",
                json!({"file_path": "a.yml", "description": "add a", "new_content": "new"}),
                json!({
                    "file_path": "a.yml", "description": "add a",
                    "new_content": "new", "old_content": ""
                }),
            ),
            (
                "file_changed",
                json!({
                    "file_path": "b.yml", "description": "drop b", "new_content": "",
                    "old_content": "old", "is_deletion": true
                }),
                json!({
                    "file_path": "b.yml", "description": "drop b", "new_content": "",
                    "old_content": "old", "is_deletion": true
                }),
            ),
            (
                "file_changed",
                json!({"file_path": "c.yml", "description": "edit c", "new_content": "x"}),
                json!({
                    "file_path": "c.yml", "description": "edit c", "new_content": "x",
                    "old_content": "", "is_deletion": false
                }),
            ),
            (
                "tool_used",
                json!({"tool_name": "read_file", "summary": "Read c.yml"}),
                json!({"tool_name": "read_file", "summary": "Read c.yml"}),
            ),
        ]
    }

    #[test]
    fn frozen_handler_renders_stored_builder_rows_to_the_golden_output() {
        let handler = event_handler();
        for (ty, stored, expected) in golden_rows() {
            assert_eq!(
                (handler.processor)(ty, &stored),
                Some(vec![(ty.to_string(), expected)]),
                "row {ty}"
            );
        }
        assert!(!(handler.should_accumulate.as_ref().unwrap())("tool_used"));
    }

    #[test]
    fn frozen_step_summaries_match_the_golden_table() {
        let summary = event_handler().summary_fn;
        for (state, expected) in [
            ("clarifying", Some("Preparing the builder request")),
            ("specifying", Some("Structuring the builder task")),
            ("solving", Some("Working")),
            ("executing", Some("Applying output")),
            ("interpreting", Some("Summarizing")),
            ("diagnosing", Some("Recovering from an error")),
            ("other", None),
        ] {
            assert_eq!(summary(state), expected.map(String::from), "{state}");
        }
    }

    #[test]
    fn frozen_tool_summaries_match_the_golden_table() {
        let tool_summary = event_handler().tool_summary_fn;
        let with_desc = json!({"description": "do it"});
        for (tool, expected) in [
            ("search_files", Some("Searching files")),
            ("read_file", Some("Reading a file")),
            ("search_text", Some("Searching file contents")),
            ("file_change", Some("Making a file change")),
            ("write_file", Some("Writing a file")),
            ("edit_file", Some("Editing a file")),
            ("delete_file", Some("Deleting a file")),
            ("manage_directory", Some("Managing a directory")),
            ("validate_project", Some("Validating objects")),
            ("lookup_schema", Some("Looking up schema details")),
            ("run_tests", Some("Running tests")),
            ("execute_sql", Some("Running SQL")),
            ("semantic_query", Some("Running a semantic query")),
            ("ask_user", Some("Asking for user input")),
            ("unknown", None),
        ] {
            assert_eq!(
                tool_summary(tool, &json!({})),
                expected.map(String::from),
                "{tool}"
            );
            // Only the three file-writing tools take the LLM's description.
            let described = if matches!(tool, "write_file" | "edit_file" | "delete_file") {
                Some("do it")
            } else {
                expected
            };
            assert_eq!(
                tool_summary(tool, &with_desc),
                described.map(String::from),
                "{tool} with description"
            );
        }
        assert_eq!(
            tool_summary("write_file", &json!({"description": ""})),
            Some("Writing a file".to_string())
        );
    }

    #[test]
    fn an_unknown_builder_event_kind_is_skipped_not_fatal() {
        let payload = json!({"x": 1});
        assert!((event_handler().processor)("no_such_event", &payload).is_none());
    }
}
