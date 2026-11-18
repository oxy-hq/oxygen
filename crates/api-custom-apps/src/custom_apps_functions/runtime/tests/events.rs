//! `ctx.events.publish` and `tx.events.publish` across the boundary.

use super::*;

/// `ctx.events.publish(type, data, opts)` arrives as one payload — the type,
/// the data and the options flattened beside them — and the host's
/// `{ eventId }` comes back to the function.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_events_publish_reaches_the_host() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const r = await ctx.events.publish(
                    "order.placed",
                    { orderId: 7 },
                    { key: "order-7", delaySeconds: 30 },
                );
                return Response.json(r);
            };
        "#,
    )
    .await;

    let resp = result.expect("ctx.events.publish must resolve");
    assert!(resp.body.contains(r#""eventId":"evt-1""#), "{}", resp.body);
    assert_eq!(
        host.published(),
        vec![(
            "events",
            serde_json::json!({
                "type": "order.placed",
                "data": { "orderId": 7 },
                "key": "order-7",
                "delaySeconds": 30,
            })
        )]
    );
}

/// On an OLTP transaction handle the same publish carries the handle's id,
/// so the host writes the outbox row on the pinned connection, and the
/// bracket still commits around it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tx_events_publish_carries_the_tx_id() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const r = await ctx.oltp.tx(async (tx) => {
                    await tx.exec("UPDATE bookings SET seated = true WHERE id = $1", [7]);
                    return tx.events.publish("booking.seated", { id: 7 }, { key: "seat-7" });
                });
                return Response.json(r);
            };
        "#,
    )
    .await;

    let resp = result.expect("tx.events.publish must resolve");
    assert!(resp.body.contains(r#""eventId":"evt-tx""#), "{}", resp.body);
    assert_eq!(
        host.tx_ops(),
        vec!["begin_oltp", "exec", "publish", "commit"]
    );
    assert_eq!(
        host.published(),
        vec![(
            "tx",
            serde_json::json!({
                "id": 1,
                "type": "booking.seated",
                "data": { "id": 7 },
                "key": "seat-7",
            })
        )]
    );
}

/// Only an OLTP handle can carry an outbox row: a warehouse `ctx.tx` handle
/// has no `events` at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ctx_tx_handle_has_no_events() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const t = await ctx.tx("db", async (tx) => typeof tx.events);
                return Response.json({ t });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(resp.body.contains(r#""t":"undefined""#), "{}", resp.body);
}

/// A function that declared no `events` is refused by the broker in the
/// host's words, before the host method runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_without_the_capability_is_refused() {
    let host = Arc::new(MockHost {
        caps: Some(FunctionCapabilities::default()),
        ..Default::default()
    });
    let resp = run_on(
        &attempt(r#"ctx.events.publish("order.placed", {}, { key: "k" })"#),
        test_ctx(),
        host.clone(),
    )
    .await
    .unwrap();
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.starts_with("ctx.events: EventsCapabilityMissing: this function has not declared"),
        "{error}"
    );
    assert!(host.published().is_empty(), "never dispatched");
    assert_eq!(
        host.host_call_failures(),
        [("events.publish", "not_allowed")]
    );
}

/// R21: the payload is built from named fields alone. Options carrying a
/// `type`, an `id` or anything else change nothing the host reads, and an
/// option left out is absent rather than `undefined`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_events_publish_sends_only_the_named_fields() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const r = await ctx.events.publish(
                    "order.placed",
                    { orderId: 7 },
                    { key: "order-7", type: "order.ghost", id: 999, extra: true },
                );
                return Response.json(r);
            };
        "#,
    )
    .await;

    result.expect("ctx.events.publish must resolve");
    assert_eq!(
        host.published(),
        vec![(
            "events",
            serde_json::json!({
                "type": "order.placed",
                "data": { "orderId": 7 },
                "key": "order-7",
            })
        )]
    );
}

/// R21 on a handle: `tx.events.publish("x", d, { id: 999 })` still publishes
/// in the handle's own transaction, under its own type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tx_events_publish_keeps_the_handles_id_and_type() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const r = await ctx.oltp.tx((tx) =>
                    tx.events.publish(
                        "booking.seated",
                        { id: 7 },
                        { key: "seat-7", delaySeconds: 5, id: 999, type: "other" },
                    ),
                );
                return Response.json(r);
            };
        "#,
    )
    .await;

    result.expect("tx.events.publish must resolve");
    assert_eq!(host.tx_ops(), vec!["begin_oltp", "publish", "commit"]);
    assert_eq!(
        host.published(),
        vec![(
            "tx",
            serde_json::json!({
                "id": 1,
                "type": "booking.seated",
                "data": { "id": 7 },
                "key": "seat-7",
                "delaySeconds": 5,
            })
        )]
    );
}
