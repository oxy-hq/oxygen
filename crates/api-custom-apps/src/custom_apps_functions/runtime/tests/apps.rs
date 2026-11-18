//! `ctx.apps.call`, `ctx.apps.send` and `ctx.caller` across the boundary.

use super::*;

/// A call arrives as named fields, and the host's answer comes back as is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_apps_call_reaches_the_host_and_returns_its_answer() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) =>
                Response.json(await ctx.apps.call("other-app", "ping", { n: 1 }, { timeoutMs: 2500, app: "evil" }));
        "#,
    )
    .await;
    let resp = result.expect("ctx.apps.call must resolve");
    assert_eq!(resp.body, r#"{"echo":{"n":1}}"#);
    assert_eq!(
        host.apps_calls(),
        vec![(
            "call".to_string(),
            serde_json::json!({
                "app": "other-app", "function": "ping", "body": { "n": 1 }, "timeoutMs": 2500,
            })
        )],
        "options reach the host as `timeoutMs` alone"
    );
}

/// A send carries its own options and answers `{ eventId }`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_apps_send_carries_delay_and_key() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) =>
                Response.json(await ctx.apps.send("other-app", "ping", undefined, { delaySeconds: 30, key: "k1", timeoutMs: 9 }));
        "#,
    )
    .await;
    assert_eq!(result.expect("resolves").body, r#"{"eventId":"evt-send"}"#);
    assert_eq!(
        host.apps_calls(),
        vec![(
            "send".to_string(),
            serde_json::json!({
                "app": "other-app", "function": "ping", "body": null, "delaySeconds": 30, "key": "k1",
            })
        )]
    );
}

/// The host's `AppCall<Kind>: text` failure is thrown as an `AppCallError`
/// carrying the kind, the text and the whole host message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_call_throws_an_app_call_error_with_its_kind() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                try {
                    await ctx.apps.call("other-app", "boom", {});
                    return Response.json({ threw: false });
                } catch (e) {
                    return Response.json({ name: e.name, kind: e.kind, message: e.message, host: e.hostMessage });
                }
            };
        "#,
    )
    .await;
    let body: serde_json::Value =
        serde_json::from_str(&result.expect("handled").body).expect("json");
    assert_eq!(
        body,
        serde_json::json!({
            "name": "AppCallError",
            "kind": "failed",
            "message": "the callee broke",
            "host": "AppCallFailed: the callee broke",
        })
    );
}

/// An error that is not an `AppCall…` one is the host's own, untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_call_error_is_not_retyped() {
    let host = Arc::new(MockHost {
        caps: Some(FunctionCapabilities::default()),
        ..Default::default()
    });
    let resp = run_on(
        &attempt(r#"ctx.apps.call("a", "b", {})"#),
        test_ctx(),
        host.clone(),
    )
    .await
    .unwrap();
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.starts_with("ctx.apps: AppsCapabilityMissing: this function has not declared"),
        "{error}"
    );
    assert!(host.apps_calls().is_empty(), "never dispatched");
    assert_eq!(host.host_call_failures(), [("apps.call", "not_allowed")]);
}

/// `ctx.caller` is `null` unless the host set it, and then is frozen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_caller_is_the_hosts_fact() {
    let src = r#"
        export default async (req, ctx) => {
            let frozen = Object.isFrozen(ctx.caller);
            try { ctx.caller.app = "x"; } catch {}
            return Response.json({ caller: ctx.caller, frozen });
        };
    "#;
    let (none, _) = run_with_mock(src).await;
    let none: serde_json::Value = serde_json::from_str(&none.unwrap().body).unwrap();
    assert_eq!(none["caller"], serde_json::Value::Null);

    let id = uuid::Uuid::from_u128(7);
    let ctx = InvocationCtx {
        caller: Some(CtxCaller {
            app: "bookkeeping".into(),
            function: "poster".into(),
            invocation_id: id,
            actor: Some("user:ada".into()),
        }),
        ..test_ctx()
    };
    let (result, _) = run_with_mock_ctx(src, ctx).await;
    let body: serde_json::Value = serde_json::from_str(&result.unwrap().body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "caller": {
                "app": "bookkeeping", "function": "poster",
                "invocationId": id.to_string(), "actor": "user:ada",
            },
            "frozen": true,
        })
    );
}
