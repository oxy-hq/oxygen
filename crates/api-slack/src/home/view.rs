use crate::resolution::workspace_agent::WorkspaceSummary;
use serde_json::{Value, json};

pub fn unlinked_view(connect_url: &str) -> Value {
    json!({
        "type": "home",
        "blocks": [
            {"type": "header", "text": {"type": "plain_text", "text": "👋 Welcome to Oxygen"}},
            {"type": "section", "text": {"type": "mrkdwn",
                "text": "Oxygen brings your data analytics to Slack. Ask questions, get SQL-backed answers, and share results in-thread."}},
            {"type": "divider"},
            {"type": "section", "text": {"type": "mrkdwn",
                "text": "*What you'll be able to do*\n\
                         • Mention Oxygen in any channel: `@Oxygen what was revenue last quarter?`\n\
                         • Ask for visualizations: `@Oxygen build a chart of weekly active users`\n\
                         • Chat privately: open a DM with Oxygen and ask in your own thread\n\
                         • Iterate in thread: reply to any Oxygen response to keep the conversation going"}},
            {"type": "divider"},
            {"type": "section", "text": {"type": "mrkdwn",
                "text": "*Get started* — connect your Oxygen account to begin."}},
            {"type": "actions", "elements": [{
                "type": "button", "action_id": "slack_home_connect",
                "text": {"type": "plain_text", "text": "Connect to Oxygen"},
                "url": connect_url, "style": "primary"
            }]}
        ]
    })
}

pub struct LinkedHomeInput<'a> {
    pub email: &'a str,
    pub org_name: &'a str,
    pub workspaces: &'a [WorkspaceSummary],
    pub default_workspace_id: Option<uuid::Uuid>,
    pub default_agent_path: Option<&'a str>,
    pub app_base_url: &'a str,
}

pub fn linked_view(input: LinkedHomeInput<'_>) -> Value {
    let ws_options: Vec<Value> = input
        .workspaces
        .iter()
        .map(|w| {
            json!({
                "text": {"type": "plain_text", "text": w.name.clone()},
                "value": w.id.to_string(),
            })
        })
        .collect();
    let default_ws_option = input.default_workspace_id.and_then(|id| {
        input.workspaces.iter().find(|w| w.id == id).map(|w| {
            json!({
                "text": {"type": "plain_text", "text": w.name.clone()},
                "value": w.id.to_string(),
            })
        })
    });

    // Build flat agent options list: "<workspace> / <agent>" → "<ws_uuid>|<agent_path>"
    let agent_options: Vec<Value> = input
        .workspaces
        .iter()
        .flat_map(|w| {
            w.agents.iter().map(|a| {
                json!({
                    "text": {"type": "plain_text", "text": format!("{} / {}", w.name, a)},
                    "value": format!("{}|{}", w.id, a),
                })
            })
        })
        .take(100) // Slack limits static_select options to 100
        .collect();

    let default_agent_option = input
        .default_workspace_id
        .zip(input.default_agent_path)
        .and_then(|(ws_id, agent_path)| {
            let ws = input.workspaces.iter().find(|w| w.id == ws_id)?;
            Some(json!({
                "text": {"type": "plain_text", "text": format!("{} / {}", ws.name, agent_path)},
                "value": format!("{}|{}", ws_id, agent_path),
            }))
        });

    let mut blocks = vec![
        json!({"type": "header", "text": {"type": "plain_text", "text": "🧠 Oxygen"}}),
        json!({"type": "context", "elements": [
            {"type": "mrkdwn", "text": format!("Connected as *{}* in *{}*", input.email, input.org_name)}
        ]}),
        // Usage help — teaches @mention + DM + iteration pattern
        json!({"type": "section", "text": {"type": "mrkdwn",
            "text": "💬 *Ask me in any channel* — `@Oxygen how many customers signed up this week?`\n\
                     📩 *Or chat privately* — open a DM to ask in your own thread\n\
                     🔄 *Iterate in thread* — reply to any Oxygen response to keep the conversation going"}}),
        json!({"type": "divider"}),
    ];

    // Workspace picker — Slack rejects `static_select` with `options: []`,
    // so when the org hasn't been wired up to any Oxy workspaces we show an
    // empty-state section pointing at the web UI instead of the picker.
    if ws_options.is_empty() {
        blocks.push(json!({"type": "section", "text": {"type": "mrkdwn",
            "text": "*No workspaces yet* — create one in the Oxygen web app, then refresh this tab \
                     to set your defaults here."}}));
        blocks.push(json!({"type": "actions", "elements": [{
            "type": "button",
            "action_id": "slack_home_open_oxy",
            "text": {"type": "plain_text", "text": "Open Oxygen"},
            "url": input.app_base_url,
            "style": "primary"
        }]}));
    } else {
        let mut workspace_select = json!({
            "type": "static_select",
            "action_id": "slack_home_pick_workspace",
            "placeholder": {"type": "plain_text", "text": "Select"},
            "options": ws_options,
        });
        if let Some(opt) = default_ws_option {
            workspace_select["initial_option"] = opt;
        }
        blocks.push(json!({"type": "section",
               "text": {"type": "mrkdwn", "text": "*Default workspace*"},
               "accessory": workspace_select}));

        // Agent picker depends on the workspace picker; only emit it when
        // both the workspace list and the (filtered) agent list are non-
        // empty. Slack would otherwise reject the section.
        if !agent_options.is_empty() {
            let mut agent_select = json!({
                "type": "static_select",
                "action_id": "slack_home_pick_agent",
                "placeholder": {"type": "plain_text", "text": "Select"},
                "options": agent_options,
            });
            if let Some(opt) = default_agent_option {
                agent_select["initial_option"] = opt;
            }
            blocks.push(json!({"type": "section",
                   "text": {"type": "mrkdwn", "text": "*Default agent*"},
                   "accessory": agent_select}));
        }

        blocks.push(json!({"type": "actions", "elements": [
            {"type": "button", "action_id": "slack_home_save_defaults",
             "text": {"type": "plain_text", "text": "Save defaults"}}
        ]}));
    }
    blocks.push(json!({"type": "divider"}));
    blocks.push(json!({"type": "actions", "elements": [
        {"type": "button", "action_id": "slack_home_disconnect", "style": "danger",
         "text": {"type": "plain_text", "text": "Disconnect my account"}}
    ]}));
    blocks.push(json!({"type": "context", "elements": [
        {"type": "mrkdwn", "text": format!("<{}|Open Oxygen>", input.app_base_url)}
    ]}));

    json!({"type": "home", "blocks": blocks})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlinked_home_has_connect_button() {
        let v = unlinked_view("https://example/slack/link?token=abc");
        let blocks = v["blocks"].as_array().unwrap();
        assert!(blocks.iter().any(|b| {
            b["elements"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|el| el["action_id"].as_str())
                == Some("slack_home_connect")
        }));
    }

    #[test]
    fn unlinked_view_includes_examples() {
        let v = unlinked_view("https://example/slack/link?token=abc");
        let raw = serde_json::to_string(&v).expect("serialize");
        assert!(
            raw.contains("@Oxygen"),
            "unlinked view must include @Oxygen example"
        );
        assert!(
            raw.contains("revenue"),
            "unlinked view must include revenue example"
        );
        assert!(
            raw.contains("weekly active users"),
            "unlinked view must include weekly active users example"
        );
        assert!(
            raw.contains("DM"),
            "unlinked view must mention DM usage pattern"
        );
    }

    #[test]
    fn linked_view_includes_usage_help() {
        let workspaces = vec![];
        let v = linked_view(LinkedHomeInput {
            email: "alice@example.com",
            org_name: "Acme",
            workspaces: &workspaces,
            default_workspace_id: None,
            default_agent_path: None,
            app_base_url: "https://app.oxygen-hq.com",
        });
        let raw = serde_json::to_string(&v).expect("serialize");
        assert!(
            raw.contains("Ask me in any channel"),
            "linked view must include channel usage tip"
        );
        assert!(
            raw.contains("chat privately"),
            "linked view must mention private DM usage"
        );
        assert!(
            raw.contains("Iterate in thread"),
            "linked view must mention thread iteration"
        );
    }

    /// Slack rejects every block in a view if any `static_select` carries
    /// `options: []`. Walk the JSON tree and assert that no static_select
    /// element ever ships with an empty options array.
    fn assert_no_empty_static_select(value: &Value) {
        match value {
            Value::Object(map) => {
                if map.get("type").and_then(Value::as_str) == Some("static_select") {
                    let options = map.get("options").and_then(Value::as_array);
                    assert!(
                        options.is_some_and(|o| !o.is_empty()),
                        "static_select must never carry an empty options array \
                         (Slack rejects the entire view)"
                    );
                }
                for v in map.values() {
                    assert_no_empty_static_select(v);
                }
            }
            Value::Array(items) => {
                for v in items {
                    assert_no_empty_static_select(v);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn linked_view_with_no_workspaces_omits_static_select() {
        let workspaces = vec![];
        let v = linked_view(LinkedHomeInput {
            email: "alice@example.com",
            org_name: "Acme",
            workspaces: &workspaces,
            default_workspace_id: None,
            default_agent_path: None,
            app_base_url: "https://app.oxygen-hq.com",
        });
        // No empty static_select sneaks through.
        assert_no_empty_static_select(&v);

        let raw = serde_json::to_string(&v).expect("serialize");
        // Empty-state CTA replaces the picker.
        assert!(
            raw.contains("No workspaces yet"),
            "empty-workspace view must surface the empty-state copy"
        );
        assert!(
            raw.contains("slack_home_open_oxy"),
            "empty-workspace view must surface an Open Oxygen CTA"
        );
        assert!(
            !raw.contains("slack_home_save_defaults"),
            "empty-workspace view must hide Save defaults (nothing to save)"
        );
    }

    #[test]
    fn linked_view_has_workspace_and_disconnect() {
        let workspaces = vec![WorkspaceSummary {
            id: uuid::Uuid::new_v4(),
            name: "My Workspace".into(),
            agents: vec![],
            default_agent: None,
        }];
        let v = linked_view(LinkedHomeInput {
            email: "bob@example.com",
            org_name: "Acme",
            workspaces: &workspaces,
            default_workspace_id: None,
            default_agent_path: None,
            app_base_url: "https://app.oxygen-hq.com",
        });
        let raw = serde_json::to_string(&v).expect("serialize");
        assert!(
            raw.contains("slack_home_pick_workspace"),
            "linked view must have workspace picker"
        );
        assert!(
            raw.contains("slack_home_disconnect"),
            "linked view must have disconnect button"
        );
    }
}
