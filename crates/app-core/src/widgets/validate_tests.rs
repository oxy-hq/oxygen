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
        "widget_size_invalid"
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
    let seven: serde_json::Map<String, Value> = (0..7).map(w).collect();
    assert_eq!(refused(Value::Object(seven)), "widget_too_many");
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

#[test]
fn sizes_are_checked() {
    let with = |extra: Value| {
        let mut w = json!({ "title": "C", "function": "close-widget" });
        w.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        check(Some(&manifest(json!({ "close": w }))))
    };
    assert!(with(json!({ "size": "wide", "sizes": ["regular", "wide", "full"] })).is_ok());
    assert!(with(json!({ "size": "full" })).is_ok());
    for bad in [
        json!({ "sizes": [] }),
        json!({ "sizes": "wide" }),
        json!({ "sizes": ["regular", "tall"] }),
        json!({ "sizes": ["regular", "regular"] }),
        json!({ "size": "wide", "sizes": ["regular"] }),
    ] {
        assert_eq!(
            with(bad.clone()).expect_err("refused").check,
            "widget_size_invalid",
            "{bad}"
        );
    }
}

#[test]
fn six_widgets_pass_and_seven_do_not() {
    let n = |count: u8| -> Value {
        Value::Object(
            (0..count)
                .map(|i| {
                    (
                        format!("w{i}"),
                        json!({ "title": "W", "function": "close-widget" }),
                    )
                })
                .collect(),
        )
    };
    assert!(check(Some(&manifest(n(6)))).is_ok());
    assert_eq!(refused(n(7)), "widget_too_many");
}

#[test]
fn a_settings_schema_is_checked_at_publish() {
    let with = |params: Value| {
        check(Some(&manifest(json!({ "close": {
            "title": "C", "function": "close-widget", "params": params
        } }))))
    };
    assert!(with(json!({ "type": "object", "properties": { "store": { "type": "string" } }, "required": ["store"] })).is_ok());
    assert_eq!(
        with(json!({ "type": "object", "properties": { "store": { "type": "object" } } }))
            .unwrap_err()
            .check,
        "widget_params_invalid"
    );
}
