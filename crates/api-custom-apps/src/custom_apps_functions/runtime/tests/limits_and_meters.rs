//! Heap ceiling, request-body cost and invocation meters.

use super::*;

/// The whole premise of the heap ceiling, asserted end to end.
///
/// Before it, `create_params` was unset and V8's own default limit sat far
/// above the pod's cgroup limit, so a runaway allocation was reaped by the
/// kernel OOM killer — which kills the **process**, taking down every other
/// app and every product route it was serving. This test allocates without
/// bound and asserts the isolate dies alone.
///
/// The test process surviving to make the assertion *is* half the
/// assertion: without a working ceiling this test does not fail, it takes
/// the test binary with it.
#[tokio::test]
async fn a_runaway_allocation_kills_the_isolate_not_the_process() {
    // Set before the first `heap_limit_bytes()` call in this process —
    // nextest gives each test its own process, so the OnceLock is ours.
    // 16 MiB rather than the 128 MiB default so it trips in well under the
    // 10s helper timeout; the mechanism under test is identical.
    unsafe { std::env::set_var(HEAP_LIMIT_MB_ENV, "16") };
    assert_eq!(heap_limit_bytes(), Some(16 * 1024 * 1024));

    let (result, _host) = run_with_mock_ctx(
        r#"
            export default async function () {
              const held = [];
              // Retained, so GC cannot reclaim any of it — an unretained loop
              // would spin forever instead of breaching the ceiling.
              for (;;) { held.push(new Array(1_000_000).fill(7)); }
            }
            "#,
        test_ctx(),
    )
    .await;

    assert!(
        matches!(result, Err(RuntimeError::OutOfMemory)),
        "a heap breach must be attributable, not a generic failure: {result:?}"
    );
}

/// The ceiling must be switchable off, and `0` is the off switch. An
/// operator debugging a memory-hungry function needs a way to lift it
/// without a rebuild, and "unset" already means the default.
#[test]
fn a_zero_heap_limit_disables_the_ceiling() {
    unsafe { std::env::set_var(HEAP_LIMIT_MB_ENV, "0") };
    assert_eq!(heap_limit_bytes(), None);
}

/// Delivering the request body must cost the isolate's heap one copy of it.
///
/// The body used to be spliced into the invoke script as a string literal,
/// and V8 keeps a script's source for as long as the script runs. So the
/// body was on the heap twice before the handler's first line — once as
/// source, once as the literal's value — and both counted against the
/// ceiling the handler then had to work under.
///
/// Measured that way, under a 64 MiB ceiling: with no body a handler held
/// 40 MiB, and beside a 28 MiB body it was terminated holding 2. Here it
/// holds 12 MiB beside the same body, in pieces small enough that V8 has to
/// find room for each — the ceiling is enforced by collections that free
/// nothing, so one large allocation that crosses it and returns would not
/// show the difference.
#[tokio::test]
async fn delivering_the_request_body_costs_the_heap_one_copy_of_it() {
    // Set before the first `heap_limit_bytes()` call in this process, as in
    // `a_runaway_allocation_kills_the_isolate_not_the_process`.
    unsafe { std::env::set_var(HEAP_LIMIT_MB_ENV, "64") };
    assert_eq!(heap_limit_bytes(), Some(64 * 1024 * 1024));

    const BODY_BYTES: usize = 28 * 1024 * 1024;
    let result = run_request(
        r#"
            export default async (req) => {
              const held = [];
              // 64 KiB a piece, 12 MiB in all.
              for (let i = 0; i < 12 * 16; i++) held.push(new Array(8192).fill(i));
              return Response.json({ chars: req.body.length, pieces: held.length });
            };
            "#,
        test_ctx(),
        Arc::new(MockHost::default()),
        FnRequest::from_body(vec![b'A'; BODY_BYTES]),
    )
    .await;

    let resp = result.expect("12 MiB of work fits beside one copy of a 28 MiB body");
    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(parsed["chars"], BODY_BYTES);
    assert_eq!(parsed["pieces"], 12 * 16);
}

// ── Invocation meters ────────────────────────────────────────────────

/// The distinction the `Option` exists for. An invocation that never
/// reached tenant code — a module that failed to compile, a timeout during
/// setup — must not report that setup took 0 ms, which would read as
/// "instant" and hide exactly the case worth seeing.
#[test]
fn init_ms_is_absent_until_tenant_code_is_reached_not_zero() {
    let meters = InvocationMeters::start();
    assert_eq!(meters.init_ms(), None, "unmarked must be absent, not 0");

    meters.mark_tenant_code_entered();
    // Elapsed can legitimately round to 0 ms on a fast machine; what this
    // pins is that the *stored* value is no longer the sentinel.
    meters
        .init_ms
        .fetch_max(1, std::sync::atomic::Ordering::Relaxed);
    assert!(meters.init_ms().is_some(), "marked must be present");
}

/// The counter is a shared handle precisely because the isolate thread can
/// outlive `run`; a clone must observe the same count, or the caller reads
/// zero for a function that made a hundred calls.
#[test]
fn host_call_count_is_shared_across_clones() {
    let meters = InvocationMeters::start();
    let thread_side = meters.clone();
    thread_side
        .host_calls
        .fetch_add(3, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(meters.host_calls(), 3);
}

/// Healthy is zero, and the accessor has to read the same counter the
/// grace arm increments — a gauge wired to the wrong static reports calm
/// forever, which is the failure it exists to catch.
#[test]
fn the_abandoned_isolate_gauge_starts_at_zero_and_observes_increments() {
    let before = abandoned_isolates();
    let reported = oxy_telemetry::metrics::sources::isolate_abandoned();
    assert_eq!(
        reported,
        before + 1,
        "the incrementer returns the new total"
    );
    assert_eq!(
        abandoned_isolates(),
        before + 1,
        "the accessor must read the same counter the grace arm increments"
    );
    oxy_telemetry::metrics::sources::ISOLATES_ABANDONED
        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
}

/// The live gauge has to come back to where it started once the guard is
/// dropped. A drifting gauge is worse than no gauge: it is the number a
/// concurrency cap would be sized from.
#[test]
fn the_isolate_guard_balances() {
    use oxy_telemetry::metrics::sources::{ISOLATES_LIVE, IsolateGuard};
    use std::sync::atomic::Ordering;

    let before = ISOLATES_LIVE.load(Ordering::Relaxed);
    {
        let _a = IsolateGuard::enter();
        let _b = IsolateGuard::enter();
        assert_eq!(ISOLATES_LIVE.load(Ordering::Relaxed), before + 2);
    }
    assert_eq!(ISOLATES_LIVE.load(Ordering::Relaxed), before);
}
