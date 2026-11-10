//! How many pure `decide(state, params)` calls one invocation can make.
//!
//! A simulation steps a policy many times. Whether one isolate can serve those
//! steps — rather than one invocation per step — turns on how fast plain JS
//! calls run inside it, with no host op in the loop. This measures that.
//!
//! A measurement, not a budget: the assertions are on conditions (the loop ran
//! to the exact expected result; the heap did not grow without bound), never
//! on a duration. Run it with `--no-capture` to see the rate:
//!
//! ```text
//! cargo nextest run -p oxy-api-custom-apps --lib -E 'test(decide_throughput)' --no-capture
//! ```
//!
//! V8 is a prebuilt, optimized library even in a debug Rust build, so the
//! rate reflects real JS throughput, not the debug profile.

use super::*;

const CALLS: u64 = 100_000;

/// The 100k-call loop runs this many times back to back in the one
/// invocation: one round takes a millisecond or two once warm, below what
/// `Date.now()` resolves, so the rate is taken over all of them.
const ROUNDS: u64 = 20;

/// Generous: the isolate's own ceiling is 128 MiB, and a pure loop that
/// allocates only short-lived objects should leave the heap near where it
/// started once the young generation is scavenged.
const HEAP_GROWTH_BOUND_BYTES: f64 = 32.0 * 1024.0 * 1024.0;

/// The loop the function runs, restated in Rust so the test can assert the
/// exact accumulator. An exact match is also what proves V8 did not elide
/// the calls: the result depends on every one of them.
fn expected_accumulator(calls: u64) -> i64 {
    let (reorder_point, order_qty, high, base, step) = (20i64, 40i64, 80i64, 100i64, 5i64);
    let mut stock = 50i64;
    let mut acc = 0i64;
    for i in 0..calls as i64 {
        let order = if stock < reorder_point { order_qty } else { 0 };
        let price = if stock > high { base - step } else { base };
        stock = stock - (i % 7) + order;
        acc += order + price + stock;
    }
    acc
}

const ARTIFACT: &str = r#"
    const decide = (state, params) => ({
        order: state.stock < params.reorderPoint ? params.orderQty : 0,
        price: state.stock > params.high ? params.base - params.step : params.base,
    });

    const heap = () => {
        const op = globalThis.Deno?.core?.ops?.op_memory_usage;
        return typeof op === "function" ? op().heapUsed : null;
    };

    export default async (req, ctx) => {
        const { n, rounds } = JSON.parse(req.body);
        const params = { reorderPoint: 20, orderQty: 40, high: 80, base: 100, step: 5 };
        const heapBefore = heap();
        const started = Date.now();
        const accs = [];
        for (let r = 0; r < rounds; r++) {
            let stock = 50;
            let acc = 0;
            for (let i = 0; i < n; i++) {
                const d = decide({ stock }, params);
                stock = stock - (i % 7) + d.order;
                acc += d.order + d.price + stock;
            }
            accs.push(acc);
        }
        const elapsedMs = Date.now() - started;
        return Response.json({ accs, elapsedMs, heapBefore, heapAfter: heap() });
    };
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn decide_throughput_many_pure_calls_in_one_invocation() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let started = std::time::Instant::now();
    let result = run_request(
        ARTIFACT,
        test_ctx(),
        Arc::new(MockHost::default()),
        FnRequest::from_body(format!("{{\"n\":{CALLS},\"rounds\":{ROUNDS}}}").into_bytes()),
    )
    .await;
    let invocation_ms = started.elapsed().as_secs_f64() * 1e3;

    let body = result.expect("the invocation must complete").body;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    let expected = expected_accumulator(CALLS);
    let accs = parsed["accs"].as_array().expect("accs");
    assert_eq!(accs.len() as u64, ROUNDS, "every round must have run");
    for acc in accs {
        assert_eq!(
            acc.as_i64(),
            Some(expected),
            "every decide() call must have run"
        );
    }

    let loop_ms = parsed["elapsedMs"].as_f64().expect("elapsedMs");
    let total = CALLS * ROUNDS;
    let per_sec = |ms: f64| total as f64 / (ms.max(1.0) / 1e3);
    let heap_before = parsed["heapBefore"].as_f64();
    let heap_after = parsed["heapAfter"].as_f64();
    if let (Some(before), Some(after)) = (heap_before, heap_after) {
        assert!(
            after - before < HEAP_GROWTH_BOUND_BYTES,
            "heap grew {} bytes over {total} pure calls",
            after - before
        );
    }
    tracing::info!(
        calls = total,
        loop_ms,
        loop_calls_per_sec = per_sec(loop_ms),
        invocation_ms,
        invocation_calls_per_sec = per_sec(invocation_ms),
        heap_before_bytes = heap_before,
        heap_after_bytes = heap_after,
        "decide() throughput in one invocation"
    );
}
