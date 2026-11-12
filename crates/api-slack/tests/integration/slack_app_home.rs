//! Smoke tests for the Slack App Home view builder functions.
//!
//! These are pure-function tests — no database or Slack API calls.
//!
//! Run with: cargo nextest run -p oxy-api-slack --test integration -E 'test(slack_app_home)'

use oxy_api_slack::home::view::{LinkedHomeInput, linked_view, unlinked_view};
use oxy_api_slack::resolution::workspace_agent::WorkspaceSummary;
use uuid::Uuid;

#[test]
fn unlinked_view_type_is_home() {
    let v = unlinked_view("https://app.oxygen-hq.com/slack/link?token=tok123");
    assert_eq!(v["type"].as_str(), Some("home"), "type must be 'home'");
}

#[test]
fn unlinked_view_has_connect_action() {
    let connect_url = "https://app.oxygen-hq.com/slack/link?token=tok123";
    let v = unlinked_view(connect_url);
    let blocks = v["blocks"].as_array().expect("blocks must be array");
    let found = blocks.iter().any(|b| {
        b["elements"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|el| el["action_id"].as_str())
            == Some("slack_home_connect")
    });
    assert!(
        found,
        "unlinked view must contain slack_home_connect button"
    );
}

#[test]
fn unlinked_view_embeds_connect_url() {
    let connect_url = "https://app.oxygen-hq.com/slack/link?token=unique_tok";
    let v = unlinked_view(connect_url);
    let raw = serde_json::to_string(&v).expect("serialize");
    assert!(
        raw.contains(connect_url),
        "connect URL must appear in view JSON"
    );
}

#[test]
fn linked_view_type_is_home() {
    let workspaces = vec![WorkspaceSummary {
        id: Uuid::new_v4(),
        name: "My Workspace".into(),
        agents: vec![],
        default_agent: None,
    }];
    let v = linked_view(LinkedHomeInput {
        email: "alice@example.com",
        org_name: "Acme Corp",
        workspaces: &workspaces,
        default_workspace_id: None,
        default_agent_path: None,
        app_base_url: "https://app.oxygen-hq.com",
    });
    assert_eq!(v["type"].as_str(), Some("home"), "type must be 'home'");
}

#[test]
fn linked_view_shows_email_and_org() {
    let workspaces = vec![];
    let v = linked_view(LinkedHomeInput {
        email: "bob@example.com",
        org_name: "My Org",
        workspaces: &workspaces,
        default_workspace_id: None,
        default_agent_path: None,
        app_base_url: "https://app.oxygen-hq.com",
    });
    let raw = serde_json::to_string(&v).expect("serialize");
    assert!(raw.contains("bob@example.com"), "email must appear in view");
    assert!(raw.contains("My Org"), "org name must appear in view");
}

#[test]
fn linked_view_has_disconnect_action() {
    let workspaces = vec![];
    let v = linked_view(LinkedHomeInput {
        email: "carol@example.com",
        org_name: "Acme",
        workspaces: &workspaces,
        default_workspace_id: None,
        default_agent_path: None,
        app_base_url: "https://app.oxygen-hq.com",
    });
    let raw = serde_json::to_string(&v).expect("serialize");
    assert!(
        raw.contains("slack_home_disconnect"),
        "linked view must contain slack_home_disconnect action"
    );
}

#[test]
fn linked_view_has_save_defaults_action() {
    // Save defaults is only meaningful when there's at least one workspace
    // to choose from — the empty-workspace branch hides it (and the picker)
    // and shows an "Open Oxy" CTA instead.
    let workspaces = vec![WorkspaceSummary {
        id: Uuid::new_v4(),
        name: "Acme".into(),
        agents: vec![],
        default_agent: None,
    }];
    let v = linked_view(LinkedHomeInput {
        email: "dave@example.com",
        org_name: "Acme",
        workspaces: &workspaces,
        default_workspace_id: None,
        default_agent_path: None,
        app_base_url: "https://app.oxygen-hq.com",
    });
    let raw = serde_json::to_string(&v).expect("serialize");
    assert!(
        raw.contains("slack_home_save_defaults"),
        "linked view must contain slack_home_save_defaults action when workspaces exist"
    );
}

#[test]
fn linked_view_with_workspaces_includes_picker() {
    let ws_id = Uuid::new_v4();
    let workspaces = vec![
        WorkspaceSummary {
            id: ws_id,
            name: "Alpha".into(),
            agents: vec![],
            default_agent: None,
        },
        WorkspaceSummary {
            id: Uuid::new_v4(),
            name: "Beta".into(),
            agents: vec![],
            default_agent: None,
        },
    ];
    let v = linked_view(LinkedHomeInput {
        email: "eve@example.com",
        org_name: "Acme",
        workspaces: &workspaces,
        default_workspace_id: Some(ws_id),
        default_agent_path: None,
        app_base_url: "https://app.oxygen-hq.com",
    });
    let raw = serde_json::to_string(&v).expect("serialize");
    assert!(
        raw.contains("slack_home_pick_workspace"),
        "linked view with workspaces must have workspace picker"
    );
    assert!(
        raw.contains("Alpha"),
        "workspace name must appear in picker"
    );
    assert!(raw.contains("Beta"), "workspace name must appear in picker");
    // Default workspace should be pre-selected.
    assert!(
        raw.contains(&ws_id.to_string()),
        "default workspace id must appear in initial_option"
    );
}

#[test]
fn linked_view_includes_open_oxy_link() {
    let base_url = "https://app.oxygen-hq.com";
    let workspaces = vec![];
    let v = linked_view(LinkedHomeInput {
        email: "frank@example.com",
        org_name: "Acme",
        workspaces: &workspaces,
        default_workspace_id: None,
        default_agent_path: None,
        app_base_url: base_url,
    });
    let raw = serde_json::to_string(&v).expect("serialize");
    assert!(
        raw.contains(base_url),
        "linked view must include app_base_url for 'Open Oxy' context link"
    );
}

#[test]
fn unlinked_view_includes_examples() {
    let v = unlinked_view("https://app.oxygen-hq.com/slack/link?token=tok123");
    let raw = serde_json::to_string(&v).expect("serialize");
    assert!(
        raw.contains("@Oxy"),
        "unlinked view must include @Oxy example"
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
        email: "grace@example.com",
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

#[test]
fn linked_view_has_workspace_and_disconnect() {
    let workspaces = vec![WorkspaceSummary {
        id: Uuid::new_v4(),
        name: "My Workspace".into(),
        agents: vec![],
        default_agent: None,
    }];
    let v = linked_view(LinkedHomeInput {
        email: "henry@example.com",
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
