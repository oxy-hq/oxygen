use serde_json::json;

use super::super::{AppEmailer, EmailSendInput};

fn emailer() -> AppEmailer {
    AppEmailer {
        app_name: "Acme Reports".to_string(),
        from_mailbox: "noreply@oxygen-hq.com".to_string(),
        aws_region: None,
        local_test: false,
    }
}

fn parse(value: serde_json::Value) -> EmailSendInput {
    serde_json::from_value(value).expect("valid input")
}

/// The message comes back as `send` would have normalized it: trimmed
/// addresses, the body as written, the attachments by name.
#[test]
fn a_valid_message_is_rendered_as_send_would_have_sent_it() {
    let rendered = emailer()
        .render(parse(json!({
            "to": [" cfo@customer.example "], "cc": "controller@customer.example",
            "replyTo": "ap@customer.example", "subject": " JE posted ",
            "html": "<p>posted</p>", "text": "posted",
            "attachments": [{ "filename": "je.csv", "content": "YSxi", "contentType": "text/csv" }],
        })))
        .expect("renders");
    assert_eq!(rendered.subject, "JE posted");
    assert_eq!(rendered.to, ["cfo@customer.example"]);
    assert_eq!(rendered.cc, ["controller@customer.example"]);
    assert!(rendered.bcc.is_empty());
    assert_eq!(rendered.reply_to.as_deref(), Some("ap@customer.example"));
    assert_eq!(rendered.html.as_deref(), Some("<p>posted</p>"));
    assert_eq!(rendered.text.as_deref(), Some("posted"));
    assert_eq!(rendered.attachments, ["je.csv"]);
}

/// A payload `send` refuses is refused here, by the same words: capture does
/// not make a bad message pass.
#[test]
fn a_message_send_would_refuse_is_refused_by_the_same_words() {
    let cases = [
        (
            json!({ "to": "a@b.com", "text": "x" }),
            "`subject` is required",
        ),
        (
            json!({ "to": "a@b.com", "subject": "s" }),
            "provide `html` or `text`",
        ),
        (
            json!({ "subject": "s", "text": "x" }),
            "at least one `to` recipient",
        ),
        (
            json!({ "to": "a@b.com", "subject": "s", "text": "x", "from": "me@b.com" }),
            "`from` is not settable",
        ),
    ];
    for (bad, words) in cases {
        let refused = emailer().render(parse(bad.clone())).expect_err("refused");
        assert!(refused.contains(words), "{bad}: {refused}");
        let by_send = emailer().validate(parse(bad.clone())).err();
        assert_eq!(by_send.as_deref(), Some(refused.as_str()), "{bad}");
    }
}
