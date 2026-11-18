//! What an event run's isolate sees and leaves behind: a thrown error's
//! `name` survives into the invocation's error, which is how the executor
//! tells the SDK's `NonRetryableError` from any other failure.

use super::*;

/// The run's error text, for a handler that must throw.
async fn thrown(artifact: &str) -> String {
    let (result, _) = run_with_mock(artifact).await;
    match result {
        Err(RuntimeError::Js(message)) => message,
        other => panic!("the handler must throw: {other:?}"),
    }
}

/// The SDK's class, as `@oxy-hq/sdk/events` compiles it: `name` is a class
/// field, so it is set on the instance only after `super()` has built the
/// error (and captured its stack).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thrown_error_keeps_its_class_field_name() {
    let error = thrown(
        r#"
            class NonRetryableError extends Error {
                name = "NonRetryableError";
                constructor(message) {
                    super(message);
                    Object.setPrototypeOf(this, NonRetryableError.prototype);
                }
            }
            export default async () => { throw new NonRetryableError("malformed statement"); };
        "#,
    )
    .await;
    assert!(
        error.starts_with("NonRetryableError: malformed statement"),
        "{error}"
    );
}

/// A name given after the stack was already read: V8 formatted the stack's
/// first line then, under the old name, and it keeps that text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thrown_error_keeps_a_name_set_after_its_stack_was_read() {
    let error = thrown(
        r#"
            export default async () => {
                const e = new Error("malformed statement");
                void e.stack;
                e.name = "NonRetryableError";
                throw e;
            };
        "#,
    )
    .await;
    assert!(
        error.starts_with("NonRetryableError: malformed statement"),
        "{error}"
    );
}

/// The frames stay: a name restored is not a stack lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_renamed_error_keeps_its_frames() {
    let error = thrown(
        r#"
            export default async () => {
                const e = new Error("malformed statement");
                void e.stack;
                e.name = "NonRetryableError";
                throw e;
            };
        "#,
    )
    .await;
    assert!(error.contains("\n    at "), "{error}");
}

/// An error whose stack already names it is left exactly as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_error_is_unchanged() {
    let error =
        thrown(r#"export default async () => { throw new TypeError("x is undefined"); };"#).await;
    assert!(
        error.starts_with("TypeError: x is undefined\n    at "),
        "{error}"
    );
}

/// `ctx.delivery` is the attempt the host hands in, under the SDK's keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_delivery_is_the_attempt_the_host_hands_in() {
    let ctx = InvocationCtx {
        delivery: Some(crate::app_events::deliveries::DeliveryInfo {
            attempt: 2,
            max_attempts: 5,
            listener: "billing/on-upload".into(),
        }),
        ..test_ctx()
    };
    let (result, _) = run_with_mock_ctx(
        "export default async (req, ctx) => Response.json(ctx.delivery);",
        ctx,
    )
    .await;
    let body: serde_json::Value =
        serde_json::from_str(&result.expect("the handler answers").body).expect("json");
    assert_eq!(
        body,
        serde_json::json!({ "attempt": 2, "maxAttempts": 5, "listener": "billing/on-upload" })
    );
}

/// Off an event run there is no delivery: `ctx.delivery` is `undefined`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_delivery_is_absent_off_an_event_run() {
    let (result, _) = run_with_mock(
        "export default async (req, ctx) => Response.json({ absent: ctx.delivery === undefined });",
    )
    .await;
    assert_eq!(
        result.expect("the handler answers").body,
        r#"{"absent":true}"#
    );
}

/// The host's own facts: a handler cannot relabel its attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_delivery_is_frozen() {
    let ctx = InvocationCtx {
        delivery: Some(crate::app_events::deliveries::DeliveryInfo {
            attempt: 1,
            max_attempts: 5,
            listener: "billing/on-upload".into(),
        }),
        ..test_ctx()
    };
    let (result, _) = run_with_mock_ctx(
        "export default async (req, ctx) => {
            try { ctx.delivery.attempt = 5; } catch (_) {}
            return Response.json(ctx.delivery.attempt);
        };",
        ctx,
    )
    .await;
    assert_eq!(result.expect("the handler answers").body, "1");
}
