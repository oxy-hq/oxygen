//! `req` as the handler sees it.

use super::*;

/// However the body is handed over, `req` must read exactly as it did when
/// it was a JSON literal: the same three keys in the same order, and a body
/// that is the request's bytes as text — including the characters a string
/// literal has to escape, and U+FFFD wherever the bytes were not UTF-8.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_reads_the_same_however_the_body_is_handed_over() {
    let mut body = "quote \" backslash \\ newline \n nul \0 tag </script> \
                        separators \u{2028}\u{2029} astral \u{1F600} ${not} `a template`"
        .as_bytes()
        .to_vec();
    // A lone continuation byte: not UTF-8 on its own.
    body.extend_from_slice(b" invalid \x80 end");
    let expected = String::from_utf8_lossy(&body).into_owned();
    assert!(expected.contains('\u{FFFD}'));

    let resp = run_request(
        r#"
            export default async (req) =>
              Response.json({ keys: Object.keys(req), body: req.body, type: typeof req.body });
            "#,
        test_ctx(),
        Arc::new(MockHost::default()),
        FnRequest::from_body(body),
    )
    .await
    .expect("handler must complete");

    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        parsed["keys"],
        serde_json::json!(["method", "headers", "body"])
    );
    assert_eq!(parsed["type"], "string");
    assert_eq!(parsed["body"], expected);
}

/// `req` carries the request, not just its body. Asserts the plumbing end
/// to end — `sanitize_request_headers` decides *which* headers get here,
/// this proves the ones that do actually arrive at app code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_exposes_method_and_headers_to_the_handler() {
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    let result = run(
        r#"export default async function (req) {
                 return { status: 200, body: JSON.stringify({
                   method: req.method,
                   sig: req.headers["x-hub-signature-256"],
                   ct: req.headers["content-type"],
                   body: req.body,
                 }) };
               }"#
        .to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest {
            method: "POST".to_string(),
            headers: std::collections::BTreeMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("x-hub-signature-256".to_string(), "sha256=abc".to_string()),
            ]),
            body: br#"{"hello":"world"}"#.to_vec(),
        },
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler must complete");

    let parsed: serde_json::Value = serde_json::from_str(&result.body).unwrap();
    assert_eq!(parsed["method"], "POST");
    assert_eq!(parsed["ct"], "application/json");
    // The whole point: a webhook signature reaches the handler, so it can
    // be verified rather than trusted.
    assert_eq!(parsed["sig"], "sha256=abc");
    // And the body is unchanged by any of this.
    assert_eq!(parsed["body"], r#"{"hello":"world"}"#);
}
