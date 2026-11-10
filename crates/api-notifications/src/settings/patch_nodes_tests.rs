use super::*;
use crate::catalog::live::from_manifest;
use serde_json::json;

fn store_ops() -> Source {
    let catalogue = from_manifest(Some(&json!({
        "functions": { "f": { "notify": { "kinds": {
            "list_overdue": {}, "temp_alarm": { "critical": true } } } } },
        "notifications": {
            "categories": { "operations/food-safety": {}, "safety-only/alarms": {} },
            "kinds": {
                "list_overdue": { "category": "operations" },
                "temp_alarm": { "category": "safety-only/alarms" }
            }
        }
    })));
    Source {
        kinds: catalogue.kinds_with(std::iter::empty()),
        catalogue,
    }
}

fn oxygen_source() -> Source {
    let catalogue = oxygen();
    Source {
        kinds: catalogue.kinds_with(std::iter::empty()),
        catalogue,
    }
}

fn change(app: Option<Uuid>, body: serde_json::Value) -> Change {
    let mut body = body;
    body["app_id"] = app.map_or(serde_json::Value::Null, |a| json!(a));
    Change::parse(&serde_json::from_value(body).expect("a node patch")).expect("parses")
}

fn code(r: Result<(), PatchError>) -> Option<&'static str> {
    match r {
        Ok(()) => None,
        Err(PatchError::BadRequest(c)) => Some(c),
        Err(e) => panic!("{e:?}"),
    }
}

#[test]
fn a_node_named_twice_is_written_once_the_later_value_winning() {
    let app = Some(Uuid::new_v4());
    let changes = vec![
        change(
            app,
            json!({ "scope": "ops", "channels": { "push": false, "email": false } }),
        ),
        change(
            None,
            json!({ "scope": "ops", "channels": { "push": true } }),
        ),
        change(
            app,
            json!({ "scope": "ops", "channels": { "push": null }, "min_level": "critical" }),
        ),
        change(
            app,
            json!({ "kind": "ops", "channels": { "slack": false } }),
        ),
    ];
    let out = merged(changes);
    assert_eq!(
        out.len(),
        3,
        "same path in another source, or a kind of the same name, is another node"
    );
    assert_eq!(out[0].node, Node::Scope("ops".into()));
    assert_eq!(
        out[0].channels,
        vec![(Channel::Email, Some(false)), (Channel::Push, None)],
        "channels keep their place; a later null clears"
    );
    assert_eq!(out[0].min_level, Some(Some(Level::Critical)));
    assert_eq!(out[1].app, None);
    assert_eq!(out[2].node, Node::Kind("ops".into()));
}

#[test]
fn a_node_is_parsed_strictly() {
    let parse = |v: serde_json::Value| {
        serde_json::from_value::<NodePatch>(v)
            .map_err(|_| "invalid_body")
            .and_then(|p| {
                Change::parse(&p).map(|_| ()).map_err(|e| match e {
                    PatchError::BadRequest(c) => c,
                    _ => "other",
                })
            })
    };
    assert_eq!(parse(json!({ "app_id": null, "scope": "" })), Ok(()));
    assert_eq!(
        parse(json!({ "scope": "" })),
        Err("invalid_body"),
        "app_id is required"
    );
    assert_eq!(
        parse(json!({ "app_id": null, "scope": "", "kind": "k" })),
        Err("unknown_node")
    );
    assert_eq!(parse(json!({ "app_id": null })), Err("unknown_node"));
    assert_eq!(
        parse(json!({ "app_id": null, "scope": "", "channels": { "sms": false } })),
        Err("invalid_body")
    );
    assert_eq!(
        parse(json!({ "app_id": null, "scope": "", "min_level": "urgent" })),
        Err("bad_level")
    );
    assert_eq!(
        parse(json!({ "app_id": null, "scope": "", "mute": true })),
        Err("invalid_body")
    );
}

#[test]
fn an_app_node_needs_a_known_place_a_used_channel_and_a_mutable_kind_to_go_off() {
    let s = store_ops();
    let app = Some(Uuid::new_v4());
    let check_one = |v| code(check(&s, &change(app, v)));
    assert_eq!(
        check_one(json!({ "scope": "", "channels": { "push": false } })),
        None
    );
    assert_eq!(
        check_one(json!({ "scope": "operations/food-safety", "channels": { "push": true } })),
        None,
        "an empty declared category can be set ahead of its kinds"
    );
    assert_eq!(
        check_one(json!({ "scope": "finance", "channels": { "push": false } })),
        Some("unknown_node")
    );
    assert_eq!(
        check_one(json!({ "scope": "finance", "channels": { "push": null } })),
        None,
        "a stale setting can always be cleared"
    );
    assert_eq!(
        check_one(json!({ "scope": "safety-only", "channels": { "push": false } })),
        Some("critical_only")
    );
    assert_eq!(
        check_one(json!({ "scope": "safety-only", "channels": { "push": true } })),
        None,
        "on under a critical-only subtree is harmless"
    );
    assert_eq!(
        check_one(json!({ "kind": "temp_alarm", "channels": { "email": false } })),
        Some("critical_only")
    );
    assert_eq!(
        check_one(json!({ "kind": "list_overdue", "min_level": "critical",
                          "channels": { "slack": false } })),
        None
    );
    assert_eq!(
        check_one(json!({ "kind": "never_sent", "channels": { "push": false } })),
        Some("unknown_node")
    );
    assert_eq!(
        check_one(json!({ "kind": "oxy.run_failed", "channels": { "push": null } })),
        Some("unknown_node"),
        "an app's kind never wears the platform's prefix"
    );
}

#[test]
fn an_oxygen_node_follows_each_kinds_fixed_channels() {
    let s = oxygen_source();
    let check_one = |v| code(check(&s, &change(None, v)));
    assert_eq!(
        check_one(json!({ "kind": "oxy.workspace_ready", "channels": { "email": false } })),
        Some("channel_not_used"),
        "a ready workspace is never emailed"
    );
    assert_eq!(
        check_one(json!({ "kind": "oxy.run_failed", "channels": { "email": false } })),
        None
    );
    assert_eq!(
        check_one(json!({ "scope": "billing", "channels": { "push": false } })),
        Some("critical_only")
    );
    assert_eq!(
        check_one(json!({ "kind": "oxy.test", "channels": { "push": false } })),
        Some("unknown_node"),
        "a test notice has no switch"
    );
    assert_eq!(
        check_one(json!({ "kind": "oxy.not_a_kind", "channels": { "push": false } })),
        Some("unknown_node")
    );
    assert_eq!(
        check_one(json!({ "scope": "workspaces", "channels": { "slack": true } })),
        Some("channel_not_used")
    );
}
