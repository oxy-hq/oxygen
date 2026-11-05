//! A run whose caller drops it — a route caller hanging up drops `run`'s
//! future, and nothing after the await runs.

use super::*;

/// Logs a tick every 20ms for ten seconds, longer than the test waits. A log
/// line lands in the caller's buffer straight from the isolate thread, so the
/// buffer growing is the isolate still running.
const TICKER: &str = r#"
export default async () => {
  const until = Date.now() + 10000;
  let next = Date.now();
  while (Date.now() < until) {
    if (Date.now() >= next) { console.log("tick"); next += 20; }
  }
  return Response.json({ finished: true });
};
"#;

/// Dropping the run terminates its isolate. It used to run on to completion,
/// with its admission permit already released, so the per-org cap counted it
/// as gone while it still held a thread and a heap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_run_terminates_its_isolate() {
    let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let running = run(
        TICKER.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        Arc::new(MockHost::default()),
        cancel_rx,
        std::time::Duration::from_secs(30),
        logs.clone(),
        InvocationMeters::start(),
        tracing::Span::none(),
    );
    let dropped = tokio::time::timeout(std::time::Duration::from_millis(500), running).await;
    assert!(
        dropped.is_err(),
        "the ticker must still be running when it is dropped"
    );

    let ticks = || logs.lock().unwrap().len();
    // Let an in-flight tick land, then nothing more may.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let at_drop = ticks();
    assert!(at_drop > 0, "the ticker ran before the drop");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        ticks(),
        at_drop,
        "the isolate kept running after its run was dropped"
    );
}
