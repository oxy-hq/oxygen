use axum::http::StatusCode;
use oxy_app_core::function_answer::Outcome;
use oxy_app_core::widgets::model::{ActionDef, Size, WidgetDef};
use serde_json::{Value, json};

use super::run::{Press, Pressed, pressed};

const APP: &str = "https://acme--books.apps.example.com/";

fn widget() -> WidgetDef {
    WidgetDef {
        id: "close".into(),
        title: "Close".into(),
        function: "close-widget".into(),
        size: Size::Regular,
        actions: vec![ActionDef {
            id: "post-ready".into(),
            function: "post-ready-jes".into(),
            confirm: true,
            destructive: false,
        }],
    }
}

#[test]
fn a_press_sends_the_widget_the_action_and_the_input() {
    let w = widget();
    let press = Press {
        widget: &w,
        action: &w.actions[0],
        input: json!({ "period": "2026-09" }),
        key: Some("2026-09".into()),
    };
    let body: Value = serde_json::from_slice(&press.body()).unwrap();
    assert_eq!(
        body,
        json!({ "widget": "close", "action": "post-ready", "input": { "period": "2026-09" } })
    );
    let key = press.idempotency_key();
    assert!(key.starts_with("widget:close:post-ready:k-"), "{key}");
    let again = Press {
        input: json!({ "period": "2026-09", "note": "different input, same key" }),
        ..press
    };
    assert_eq!(
        again.idempotency_key(),
        key,
        "the key, not the input, names the press"
    );
}

#[test]
fn without_a_key_the_same_input_is_the_same_press() {
    let w = widget();
    let press = |input: Value| Press {
        widget: &w,
        action: &w.actions[0],
        input,
        key: None,
    };
    let a = press(json!({ "period": "2026-09" })).idempotency_key();
    assert_eq!(a, press(json!({ "period": "2026-09" })).idempotency_key());
    assert_ne!(a, press(json!({ "period": "2026-10" })).idempotency_key());
    assert!(a.starts_with("widget:close:post-ready:in-"));
    assert!(a.len() < 200, "under the function route's key cap");
}

#[test]
fn any_key_becomes_a_header_the_function_route_reads() {
    let w = widget();
    for key in [
        "Café-2026-09",
        "2026–09",
        "tab\there",
        "a b",
        &"x".repeat(120),
    ] {
        let press = Press {
            widget: &w,
            action: &w.actions[0],
            input: Value::Null,
            key: Some(key.to_string()),
        };
        let header = press.idempotency_key();
        let value = axum::http::HeaderValue::from_str(&header).expect("a valid header");
        // What `custom_apps_functions` does with it: absent unless visible ASCII.
        assert_eq!(value.to_str().ok(), Some(header.as_str()), "{key}");
        assert!(header.len() < 200);
    }
}

#[test]
fn a_done_answer_carries_its_message_and_a_checked_view() {
    let got = pressed(
        Outcome::Done(
            json!({ "message": " Posted 19 ", "view": { "headline": "All posted", "level": "ok" } }),
        ),
        &widget(),
        APP,
    );
    let Pressed::Done { message, view } = got else {
        panic!("expected done");
    };
    assert_eq!(message.as_deref(), Some("Posted 19"));
    assert_eq!(view.unwrap().headline, "All posted");
    assert_eq!(
        pressed(Outcome::Done(Value::Null), &widget(), APP),
        Pressed::Done {
            message: None,
            view: None
        }
    );
}

#[test]
fn a_refused_credential_is_403_never_401() {
    let Pressed::Failed { status, error, .. } = pressed(
        Outcome::Refused(StatusCode::UNAUTHORIZED, Value::Null),
        &widget(),
        APP,
    ) else {
        panic!("expected failed");
    };
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error, "FunctionRefused");
    let Pressed::Failed {
        status, message, ..
    } = pressed(Outcome::Failed("boom".into()), &widget(), APP)
    else {
        panic!("expected failed");
    };
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(message, "boom");
}
