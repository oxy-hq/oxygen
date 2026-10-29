use serde_json::{Value, json};

use super::validate::check;

fn manifest(widgets: Value) -> Value {
    json!({
        "functions": { "close-widget": {}, "post-ready": {} },
        "widgets": widgets
    })
}

fn refused(widgets: Value) -> &'static str {
    check(Some(&manifest(widgets)))
        .expect_err("should be refused")
        .check
}

#[test]
fn a_well_formed_block_passes_and_no_block_is_fine() {
    let ok = manifest(json!({
        "close": {
            "title": "Month-end close", "function": "close-widget", "size": "wide",
            "actions": { "post-ready": { "function": "post-ready", "confirm": true, "destructive": false } }
        }
    }));
    assert!(check(Some(&ok)).is_ok());
    assert!(check(Some(&json!({ "functions": {} }))).is_ok());
    assert!(check(None).is_ok());
}

#[test]
fn the_shape_is_checked() {
    assert_eq!(refused(json!([])), "widget_invalid");
    assert_eq!(
        refused(json!({ "Close": { "title": "C", "function": "close-widget" } })),
        "widget_invalid"
    );
    assert_eq!(
        refused(json!({ "close": { "title": "C", "function": "close-widget", "colour": "red" } })),
        "widget_invalid"
    );
    assert_eq!(
        refused(json!({ "close": { "function": "close-widget" } })),
        "widget_invalid"
    );
    assert_eq!(
        refused(json!({ "close": { "title": "x".repeat(41), "function": "close-widget" } })),
        "widget_invalid"
    );
    assert_eq!(
        refused(json!({ "close": { "title": "C", "function": "close-widget", "size": "huge" } })),
        "widget_invalid"
    );
}

#[test]
fn too_many_widgets_are_refused() {
    let w = |n: u8| {
        (
            format!("w{n}"),
            json!({ "title": "W", "function": "close-widget" }),
        )
    };
    let four: serde_json::Map<String, Value> = (0..4).map(w).collect();
    assert_eq!(refused(Value::Object(four)), "widget_too_many");
}

#[test]
fn functions_must_be_declared() {
    assert_eq!(
        refused(json!({ "close": { "title": "C", "function": "nope" } })),
        "widget_function_undeclared"
    );
    assert_eq!(
        refused(json!({ "close": {
            "title": "C", "function": "close-widget",
            "actions": { "post": { "function": "nope" } }
        } })),
        "widget_action_function_undeclared"
    );
}

#[test]
fn actions_are_checked() {
    let with = |actions: Value| {
        refused(
            json!({ "close": { "title": "C", "function": "close-widget", "actions": actions } }),
        )
    };
    let a = json!({ "function": "post-ready" });
    assert_eq!(
        with(json!({ "a": a, "b": a, "c": a, "d": a })),
        "widget_actions_too_many"
    );
    assert_eq!(with(json!({ "Post": a })), "widget_action_invalid");
    assert_eq!(
        with(json!({ "post": { "function": "post-ready", "confirm": "yes" } })),
        "widget_action_invalid"
    );
    assert_eq!(
        with(json!({ "post": { "function": "post-ready", "label": "Post" } })),
        "widget_action_invalid"
    );
    assert_eq!(with(json!({ "post": {} })), "widget_action_invalid");
}
