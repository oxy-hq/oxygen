use oxy_app_core::emails::app_emailer::RenderedEmail;
use oxy_app_core::notifications::input::{self, NotifyInput};
use serde_json::json;
use uuid::Uuid;

use super::{email_message, notify_message};
use crate::custom_apps_functions::data_audit::InvocationIdentity;

fn identity() -> InvocationIdentity {
    InvocationIdentity {
        invocation_id: Uuid::from_u128(3),
        function_name: "send".to_string(),
        mode: "route".to_string(),
        request_id: None,
        app_slug: "a".to_string(),
        user_id: None,
        user_email: None,
        credential_token_id: None,
    }
}

fn rendered() -> RenderedEmail {
    RenderedEmail {
        subject: "JE posted".to_string(),
        to: vec!["cfo@customer.example".to_string()],
        cc: vec!["controller@customer.example".to_string()],
        bcc: vec![],
        reply_to: Some("ap@customer.example".to_string()),
        html: Some("<p>posted</p>".to_string()),
        text: Some("posted".to_string()),
        attachments: vec!["je.csv".to_string()],
    }
}

fn body_of(message: crate::custom_apps_outbox::Message) -> serde_json::Value {
    let (_, bytes) = message
        .encode(chrono::Utc::now(), Uuid::nil())
        .expect("encodes");
    serde_json::from_slice(&bytes).expect("JSON")
}

/// Everyone the call named is kept, nobody was reached, and the invocation is
/// named so a tester can tie the message to the call that sent it.
#[test]
fn an_email_is_kept_as_the_call_named_it() {
    let kept = body_of(email_message(rendered(), &identity()));
    assert_eq!(kept["kind"], "email");
    assert_eq!(kept["to"], json!(["cfo@customer.example"]));
    assert_eq!(kept["cc"], json!(["controller@customer.example"]));
    assert_eq!(kept["subject"], "JE posted");
    assert_eq!(kept["body"], "<p>posted</p>", "html is preferred");
    assert_eq!(kept["format"], "html");
    assert_eq!(kept["reply_to"], "ap@customer.example");
    assert_eq!(kept["attachments"], json!(["je.csv"]));
    assert_eq!(kept["invocation_id"], Uuid::from_u128(3).to_string());
    assert_eq!(kept["function"], "send");
    assert!(kept.get("bcc").is_none(), "an empty list is absent");
}

#[test]
fn a_text_only_email_keeps_its_text() {
    let mut email = rendered();
    email.html = None;
    let kept = body_of(email_message(email, &identity()));
    assert_eq!(kept["body"], "posted");
    assert_eq!(kept["format"], "text");
}

fn notification() -> NotifyInput {
    input::parse(&json!({
        "to": [{ "userId": Uuid::from_u128(8).to_string() }],
        "kind": "shift_swap",
        "title": "Swap requested",
        "body": "Dana asked to swap",
        "link": "/swaps/1",
    }))
    .expect("a valid notification")
}

#[test]
fn a_notification_is_kept_with_its_targets_as_written() {
    let kept = body_of(notify_message(&notification(), &identity()));
    assert_eq!(kept["kind"], "notify");
    assert_eq!(
        kept["to"],
        json!([{ "userId": Uuid::from_u128(8).to_string() }])
    );
    assert_eq!(kept["title"], "Swap requested");
    assert_eq!(kept["body"], "Dana asked to swap");
    assert_eq!(kept["notify_kind"], "shift_swap");
    assert_eq!(kept["link"], "/swaps/1");
    assert!(kept.get("subject").is_none());
    assert_eq!(kept["function"], "send");
}
