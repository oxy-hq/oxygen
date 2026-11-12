use crate::resolution::workspace_agent::WorkspaceSummary;
use serde_json::{Value, json};

/// Build the Block Kit workspace picker for the first-time resolution flow.
///
/// The picker uses Slack `input` blocks (workspace select + optional channel-default
/// checkbox) plus an `actions` block with a Submit button.
///
/// On submit, Slack sends a `block_actions` payload where:
/// - `state.values.workspace_block.workspace_select.selected_option.value` → workspace UUID
/// - `state.values.default_block.set_as_default.selected_options` → `[{value:"set_as_default"}]` if checked
/// - `actions[0].value` → the base64-encoded original question
pub fn workspace_picker_blocks(workspaces: &[WorkspaceSummary], encoded_question: &str) -> Value {
    let options: Vec<Value> = workspaces
        .iter()
        .map(|w| {
            json!({
                "text": {"type": "plain_text", "text": truncate(&w.name, 75)},
                "value": w.id.to_string(),
            })
        })
        .collect();

    json!([
        {
            "type": "section",
            "text": {"type": "mrkdwn", "text": "*Which workspace should I use?*"}
        },
        {
            "type": "input",
            "block_id": "workspace_block",
            "label": {"type": "plain_text", "text": "Workspace"},
            "element": {
                "type": "static_select",
                "action_id": "workspace_select",
                "placeholder": {"type": "plain_text", "text": "Search workspaces..."},
                "options": options
            }
        },
        {
            "type": "input",
            "block_id": "default_block",
            "optional": true,
            "label": {"type": "plain_text", "text": " "},
            "element": {
                "type": "checkboxes",
                "action_id": "set_as_default",
                "options": [{
                    "text": {"type": "plain_text", "text": "Set as default for this channel"},
                    "value": "set_as_default"
                }]
            }
        },
        {
            "type": "actions",
            "elements": [{
                "type": "button",
                "action_id": "slack_submit_workspace_picker",
                "text": {"type": "plain_text", "text": "Submit"},
                "style": "primary",
                "value": encoded_question
            }]
        }
    ])
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_ws(name: &str) -> WorkspaceSummary {
        WorkspaceSummary {
            id: Uuid::new_v4(),
            name: name.to_string(),
            agents: vec![],
            default_agent: None,
        }
    }

    // ── workspace_picker_blocks (new submit-button layout) ───────────────────

    #[test]
    fn picker_has_correct_block_structure() {
        let ws = vec![make_ws("analytics")];
        let v = workspace_picker_blocks(&ws, "aGVsbG8=");
        let blocks = v.as_array().expect("array");
        assert_eq!(
            blocks.len(),
            4,
            "section + workspace input + default input + actions"
        );
        assert_eq!(blocks[0]["type"], "section");
        assert_eq!(blocks[1]["type"], "input");
        assert_eq!(blocks[1]["block_id"], "workspace_block");
        assert_eq!(blocks[2]["type"], "input");
        assert_eq!(blocks[2]["block_id"], "default_block");
        assert_eq!(blocks[3]["type"], "actions");
    }

    #[test]
    fn picker_submit_button_carries_encoded_question() {
        let ws = vec![make_ws("marketing")];
        let encoded = "aGVsbG8=";
        let v = workspace_picker_blocks(&ws, encoded);
        let btn = &v[3]["elements"][0];
        assert_eq!(btn["action_id"], "slack_submit_workspace_picker");
        assert_eq!(btn["value"], encoded);
        assert_eq!(btn["style"], "primary");
    }

    #[test]
    fn picker_workspace_options_use_uuid_as_value() {
        let ws_id = Uuid::new_v4();
        let ws = vec![WorkspaceSummary {
            id: ws_id,
            name: "data-team".into(),
            agents: vec![],
            default_agent: None,
        }];
        let v = workspace_picker_blocks(&ws, "enc");
        let opts = &v[1]["element"]["options"];
        assert_eq!(opts[0]["value"], ws_id.to_string());
        assert_eq!(opts[0]["text"]["text"], "data-team");
    }

    #[test]
    fn picker_checkbox_has_set_as_default_option() {
        let ws = vec![make_ws("ws")];
        let v = workspace_picker_blocks(&ws, "enc");
        let checkbox_opts = &v[2]["element"]["options"];
        assert_eq!(checkbox_opts[0]["value"], "set_as_default");
    }

    #[test]
    fn picker_truncates_long_names() {
        let long_name = "a".repeat(80);
        let ws = vec![WorkspaceSummary {
            id: Uuid::new_v4(),
            name: long_name,
            agents: vec![],
            default_agent: None,
        }];
        let v = workspace_picker_blocks(&ws, "enc");
        let name = v[1]["element"]["options"][0]["text"]["text"]
            .as_str()
            .unwrap();
        assert!(name.chars().count() <= 75);
        assert!(name.ends_with('…'));
    }
}
