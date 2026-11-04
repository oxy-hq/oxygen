//! The warm pool: isolates built ahead of demand, each used once.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use super::tests::{MockHost, test_ctx};
use super::warm_pool::{Job, WarmPool, size_from_env, start_isolate};
use super::*;

const TRIVIAL: &str = "export default async () => Response.json({ ok: true });";

/// `run_in` against `pool`, with the helper defaults the other runtime tests use.
async fn invoke(pool: &Arc<WarmPool>, artifact: &str) -> Result<FnResponse, RuntimeError> {
    let (_cancel_tx, cancel_rx) = oneshot::channel::<()>();
    run_in(
        pool,
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        Arc::new(MockHost::default()),
        cancel_rx,
        Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
}

/// Wait for the refill thread to bring the pool to `n` ready entries.
async fn wait_until_ready(pool: &WarmPool, n: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while pool.ready_len() < n {
        assert!(
            Instant::now() < deadline,
            "the pool never reached {n} ready"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Start `TRIVIAL` on `pool` and return how long it took, from asking for an
/// isolate until the engine exists and the caller has its terminate handle.
/// This is the moment the tenant's module can start loading.
async fn time_to_ready(pool: &Arc<WarmPool>) -> f64 {
    let (call_tx, _call_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = oneshot::channel();
    let (handle_tx, handle_rx) = oneshot::channel();
    let job = Job {
        run: IsolateRun {
            artifact_js: TRIVIAL.to_string(),
            ctx: test_ctx(),
            req: FnRequest::from_body(b"{}".to_vec()),
            call_tx,
            cancelled: Arc::new(AtomicBool::new(false)),
            logs: Arc::new(std::sync::Mutex::new(Vec::new())),
            meters: InvocationMeters::start(),
        },
        handle_tx,
        done_tx,
        hub: sentry::Hub::current(),
        span: tracing::Span::none(),
    };
    let started = Instant::now();
    start_isolate(pool, job).expect("the isolate starts");
    handle_rx.await.expect("the isolate reports ready");
    let ready_ms = started.elapsed().as_secs_f64() * 1e3;
    let response = done_rx
        .await
        .expect("the job finishes")
        .expect("and succeeds");
    assert_eq!(response.status, 200);
    ready_ms
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    samples[samples.len() / 2]
}

/// A measurement, not a budget: the assertions are on which path ran, never
/// on a duration.
#[tokio::test]
async fn time_to_a_ready_isolate_is_measured_with_the_pool_on_and_off() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    const ROUNDS: usize = 20;
    let off = WarmPool::new(0);
    let on = WarmPool::new(2);

    // Builds the snapshot and starts the refill thread, outside the timings.
    time_to_ready(&off).await;
    time_to_ready(&on).await;

    let mut pool_off = Vec::new();
    let mut pool_on = Vec::new();
    for _ in 0..ROUNDS {
        pool_off.push(time_to_ready(&off).await);
        wait_until_ready(&on, 1).await;
        pool_on.push(time_to_ready(&on).await);
    }
    assert!(!off.was_touched(), "an off pool must never be touched");
    assert_eq!(
        on.served.lock().expect("lock").len(),
        ROUNDS,
        "every timed round ran on a pool entry"
    );
    tracing::info!(
        rounds = ROUNDS,
        median_pool_off_ms = median(pool_off),
        median_pool_on_ms = median(pool_on),
        "time from asking for an isolate to a ready engine"
    );
}

/// No isolate may serve two invocations. Asserted on what the pool's threads
/// actually received, by pool id and by OS thread, across more invocations
/// than the pool holds so refilled entries are included.
#[tokio::test]
async fn no_isolate_is_handed_out_twice() {
    let pool = WarmPool::new(2);
    invoke(&pool, TRIVIAL)
        .await
        .expect("the cold first invocation");
    const INVOCATIONS: usize = 8;
    for _ in 0..INVOCATIONS {
        wait_until_ready(&pool, 1).await;
        invoke(&pool, TRIVIAL).await.expect("a pooled invocation");
    }
    let served = pool.served.lock().expect("lock").clone();
    assert_eq!(served.len(), INVOCATIONS, "each ran on a pool entry");
    let ids: std::collections::BTreeSet<_> = served.iter().map(|(id, _)| *id).collect();
    let threads: std::collections::HashSet<_> = served.iter().map(|(_, t)| *t).collect();
    assert_eq!(
        ids.len(),
        served.len(),
        "an isolate id was served twice: {served:?}"
    );
    assert_eq!(
        threads.len(),
        served.len(),
        "a thread served twice: {served:?}"
    );
}

/// What a handler leaves in the global scope is gone in the next invocation.
#[tokio::test]
async fn a_global_set_by_one_invocation_is_absent_in_the_next() {
    let pool = WarmPool::new(2);
    let handler = r#"
        export default async () => {
            const seen = {
                global: String(globalThis.__left_behind),
                proto: String(Array.prototype.__left_behind),
                response: typeof Response.__left_behind,
            };
            globalThis.__left_behind = "from the previous invocation";
            Array.prototype.__left_behind = "from the previous invocation";
            Response.__left_behind = true;
            return Response.json(seen);
        };
    "#;
    // The first call starts the refill thread and runs cold; the next two are pooled.
    invoke(&pool, TRIVIAL).await.expect("warm-up");
    let mut bodies = Vec::new();
    for _ in 0..2 {
        wait_until_ready(&pool, 1).await;
        bodies.push(invoke(&pool, handler).await.expect("invocation").body);
    }
    assert_eq!(pool.served.lock().expect("lock").len(), 2);
    let clean = r#"{"global":"undefined","proto":"undefined","response":"undefined"}"#;
    assert_eq!(bodies, vec![clean.to_string(), clean.to_string()]);
}

#[tokio::test]
async fn a_pool_of_size_zero_is_never_touched() {
    let pool = WarmPool::new(0);
    for _ in 0..2 {
        assert_eq!(invoke(&pool, TRIVIAL).await.expect("runs cold").status, 200);
    }
    assert!(
        !pool.was_touched(),
        "an off pool started a thread or built an entry"
    );
    assert!(pool.served.lock().expect("lock").is_empty());
}

#[test]
fn the_pool_size_defaults_to_two_and_zero_turns_it_off() {
    assert_eq!(size_from_env(None), 2);
    assert_eq!(size_from_env(Some("0".into())), 0);
    assert_eq!(size_from_env(Some(" 3 ".into())), 3);
    // A typo is the default, not "off".
    assert_eq!(size_from_env(Some("two".into())), 2);
}
