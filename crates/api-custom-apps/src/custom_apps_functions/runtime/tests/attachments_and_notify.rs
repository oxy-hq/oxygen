//! Email attachments and `ctx.notify` across the boundary.

use super::*;

/// Regression: this isolate is bare `deno_core` (no `deno_web`) on a V8 that
/// predates `Uint8Array.prototype.toBase64`, so `btoa` was `undefined` — an
/// author could not produce the base64 that `ctx.email.send` attachments
/// require, and attaching a generated file was impossible. Proves the
/// encoder exists AND that the bytes reach the host unmangled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn isolate_can_base64_encode_attachment_bytes() {
    let artifact = r#"
            export default async (req, ctx) => {
                // Latin1 bytes as a string — what btoa is actually specified for.
                const pdf = "\x25\x50\x44\x46"; // "%PDF"
                await ctx.email.send({
                    to: "a@b.com",
                    subject: "report",
                    text: "see attached",
                    attachments: [{ filename: "r.pdf", content: btoa(pdf) }],
                });
                let wideThrew = false;
                try { btoa("日本語"); } catch { wideThrew = true; }
                // Handing btoa raw bytes must FAIL rather than encode
                // String(u8) == "37,80,68,70". Silent corruption is the thing
                // this whole change exists to prevent.
                let bytesThrew = false;
                try { btoa(new Uint8Array([0x25, 0x50])); } catch { bytesThrew = true; }
                // Padding must not terminate the decode early: concatenated
                // base64 is malformed and has to say so, not truncate.
                let concatThrew = false;
                try { atob(btoa("hello") + btoa("world")); } catch { concatThrew = true; }
                return Response.json({
                    str: btoa("hello"),
                    roundTrip: atob(btoa("hello")),
                    wideThrew,
                    bytesThrew,
                    concatThrew,
                });
            };
        "#;
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    let resp = run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("btoa/atob must exist in the isolate");

    assert!(resp.body.contains(r#""str":"aGVsbG8=""#), "{}", resp.body);
    assert!(
        resp.body.contains(r#""roundTrip":"hello""#),
        "{}",
        resp.body
    );
    // Latin1-only, exactly like a browser. Note the subtler trap this
    // guards: btoa ACCEPTS U+0080..U+00FF and encodes them as Latin1, so
    // `btoa(csv)` on accented text yields mojibake rather than an error —
    // which is why generated text should use `encoding: "utf8"` instead.
    assert!(resp.body.contains(r#""wideThrew":true"#), "{}", resp.body);
    assert!(resp.body.contains(r#""bytesThrew":true"#), "{}", resp.body);
    assert!(resp.body.contains(r#""concatThrew":true"#), "{}", resp.body);

    let email = host
        .last_email
        .lock()
        .unwrap()
        .clone()
        .expect("ctx.email.send must reach the host");
    assert_eq!(email["attachments"][0]["content"], "JVBERg==");
}

/// The other half of the fix: generated TEXT needs no encoder at all, and
/// `encoding: "utf8"` must survive the isolate boundary so the host can
/// attach the bytes verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn utf8_attachment_crosses_the_boundary_intact() {
    let artifact = r#"
            export default async (req, ctx) => {
                await ctx.email.send({
                    to: "a@b.com",
                    subject: "report",
                    text: "see attached",
                    attachments: [{
                        filename: "report.csv",
                        content: "name,total\nCafé,3\n",
                        encoding: "utf8",
                        contentType: "text/csv",
                    }],
                });
                return Response.json({ ok: true });
            };
        "#;
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler must complete");

    let email = host.last_email.lock().unwrap().clone().expect("sent");
    let att = &email["attachments"][0];
    assert_eq!(att["encoding"], "utf8");
    assert_eq!(att["content"], "name,total\nCafé,3\n");
}

/// `ctx.notify` exists in the isolate, hands the host the payload as
/// written — targets, kind, and the email block — and resolves to what the
/// host answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_reaches_the_host_with_its_payload() {
    let artifact = r#"
            export default async (req, ctx) => {
                const r = await ctx.notify({
                    to: [{ role: "Store Manager", location: "6f1c2a8e-1b7a-4c35-9d55-0c3f1f2b9a10" }],
                    kind: "list_overdue",
                    title: "Opening list is late",
                    link: "/lists/42",
                    email: true,
                    idempotencyKey: "overdue:42",
                });
                return Response.json({ id: r.notificationId, recipients: r.recipients });
            };
        "#;
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    let resp = run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler must complete");

    assert!(resp.body.contains(r#""id":"n-1""#), "{}", resp.body);
    assert!(resp.body.contains(r#""recipients":2"#), "{}", resp.body);
    let sent = host.last_notify.lock().unwrap().clone().expect("notified");
    assert_eq!(sent["to"][0]["role"], "Store Manager");
    assert_eq!(sent["kind"], "list_overdue");
    assert_eq!(sent["email"], true);
    assert_eq!(sent["idempotencyKey"], "overdue:42");
}
