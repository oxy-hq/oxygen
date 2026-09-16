//! Non-blocking ingest for custom-app wide events and function logs.
//!
//! ## Why this is a channel and not a call
//!
//! Every emit site here is on a **hot serving path** — the HTML shell serve, an
//! asset, a function invocation. If recording telemetry could block on
//! ClickHouse, a slow or down ClickHouse would become a slow or down custom-app
//! platform. So the emit side is a `try_send` onto an unbounded channel that
//! cannot fail in a way the caller has to handle, and a background bridge owns
//! every network concern: batching, retry, backoff, bounded loss.
//!
//! That is the same shape as the span bridge ([`crate::telemetry::spawn_bridge`])
//! and it reuses the same [`FlushQueue`], deliberately: a store outage should
//! degrade custom-app telemetry the way it already degrades spans — bounded,
//! oldest-first, reported once per flush window — rather than in some second,
//! subtly different way nobody has operated before.
//!
//! ## When nothing is configured
//!
//! `OXY_OBSERVABILITY_BACKEND` unset means capture is off in every mode,
//! including local. [`record_event`] and [`record_logs`] then return
//! immediately without allocating a channel message. This must stay cheap and
//! silent: it is the default state for every developer running `oxy serve`, and
//! a warning per served request would be worse than the missing telemetry.

use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::flush_queue::FlushQueue;
use crate::store::ObservabilityStore;
use crate::types::{CustomAppClientErrorRecord, CustomAppEventRecord, CustomAppLogRecord};

/// See [`crate::telemetry`] for the cost model behind these two numbers: an
/// `INSERT` creates a MergeTree part, so a fast cadence at low volume produces
/// near-empty parts for background merges to chase.
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
const FLUSH_BATCH_SIZE: usize = 200;

/// Bounded loss under a store outage, per stream. Custom-app events are higher
/// volume than spans but individually much smaller.
const MAX_BUFFERED: usize = 20_000;

struct Sinks {
    events: UnboundedSender<CustomAppEventRecord>,
    logs: UnboundedSender<CustomAppLogRecord>,
    client_errors: UnboundedSender<CustomAppClientErrorRecord>,
}

static SINKS: OnceLock<Sinks> = OnceLock::new();

/// Wire up the process-wide sink and return the receiving ends.
///
/// First call wins, like [`crate::global::set_global`]. Returns `None` on a
/// second call so a double-install cannot start two bridges racing to insert
/// the same stream.
type Receivers = (
    UnboundedReceiver<CustomAppEventRecord>,
    UnboundedReceiver<CustomAppLogRecord>,
    UnboundedReceiver<CustomAppClientErrorRecord>,
);

fn install() -> Option<Receivers> {
    let (event_tx, event_rx) = unbounded_channel();
    let (log_tx, log_rx) = unbounded_channel();
    let (err_tx, err_rx) = unbounded_channel();
    SINKS
        .set(Sinks {
            events: event_tx,
            logs: log_tx,
            client_errors: err_tx,
        })
        .ok()?;
    Some((event_rx, log_rx, err_rx))
}

/// Record one custom-app request. Never blocks, never fails the caller.
pub fn record_event(event: CustomAppEventRecord) {
    if let Some(sinks) = SINKS.get() {
        // A closed channel means the bridge is gone (shutdown). Dropping is the
        // only correct answer — the alternative is failing a served request
        // because telemetry could not be recorded.
        let _ = sinks.events.send(event);
    }
}

/// Record a batch of function log lines. Never blocks, never fails the caller.
pub fn record_logs(logs: Vec<CustomAppLogRecord>) {
    let Some(sinks) = SINKS.get() else {
        return;
    };
    for line in logs {
        let _ = sinks.logs.send(line);
    }
}

/// Record a batch of client errors. Never blocks, never fails the caller.
///
/// Its own stream rather than riding the log one: a page in a render loop can
/// produce errors far faster than a function produces log lines, and a burst of
/// them must not evict a function's output from a shared buffer.
pub fn record_client_errors(errors: Vec<CustomAppClientErrorRecord>) {
    let Some(sinks) = SINKS.get() else {
        return;
    };
    for error in errors {
        let _ = sinks.client_errors.send(error);
    }
}

/// Whether telemetry is being collected. For call sites that would otherwise do
/// real work (cloning strings, formatting a route) to build a record that is
/// about to be dropped.
pub fn is_enabled() -> bool {
    SINKS.get().is_some()
}

/// Install the sink and spawn the two bridge tasks that drain it into `store`.
///
/// Call once, at startup, after the store is resolved. A second call is a no-op.
pub fn spawn_custom_app_bridges(store: Arc<dyn ObservabilityStore>) {
    let Some((event_rx, log_rx, err_rx)) = install() else {
        return;
    };
    spawn_stream(
        event_rx,
        store.clone(),
        "custom_app_events",
        |store, batch| Box::pin(async move { store.insert_custom_app_events(batch).await }),
    );
    spawn_stream(log_rx, store.clone(), "custom_app_logs", |store, batch| {
        Box::pin(async move { store.insert_custom_app_logs(batch).await })
    });
    spawn_stream(err_rx, store, "custom_app_client_errors", |store, batch| {
        Box::pin(async move { store.insert_custom_app_client_errors(batch).await })
    });
}

type InsertFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), oxy_shared::errors::OxyError>> + Send>,
>;

/// One batching bridge over one stream. Generic so events and logs share the
/// retry/backoff/reporting behaviour instead of having two copies of it.
fn spawn_stream<T, F>(
    mut receiver: UnboundedReceiver<T>,
    store: Arc<dyn ObservabilityStore>,
    stream: &'static str,
    insert: F,
) where
    T: Send + Sync + Clone + 'static,
    F: Fn(Arc<dyn ObservabilityStore>, Vec<T>) -> InsertFuture + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut queue: FlushQueue<T> = FlushQueue::new(
            MAX_BUFFERED,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(60),
        );
        let mut interval = tokio::time::interval(FLUSH_INTERVAL);
        interval.tick().await; // consume the immediate first tick
        let mut dropped_since_report: u64 = 0;

        loop {
            tokio::select! {
                msg = receiver.recv() => {
                    match msg {
                        Some(record) => {
                            // Accumulated, not logged per record: warning inline
                            // is loudest exactly when the store is already
                            // failing, and competes for the same resources. Same
                            // lesson as the span bridge (11k lines in 30 minutes
                            // on oxy-dev, 2026-09-03).
                            dropped_since_report = dropped_since_report
                                .saturating_add(queue.push(record) as u64);
                            if queue.len() >= FLUSH_BATCH_SIZE {
                                flush(&mut queue, &store, &insert, stream, FLUSH_BATCH_SIZE).await;
                            }
                        }
                        None => {
                            // Channel closed — final best-effort send, bypassing
                            // the backoff gate (no later attempt to defer to).
                            let batch = queue.take_all();
                            if !batch.is_empty() {
                                let _ = insert(store.clone(), batch).await;
                            }
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    if dropped_since_report > 0 {
                        tracing::warn!(
                            stream,
                            dropped = dropped_since_report,
                            window_secs = FLUSH_INTERVAL.as_secs(),
                            buffer_capacity = MAX_BUFFERED,
                            "custom-app telemetry buffer full during store outage; dropped oldest records"
                        );
                        dropped_since_report = 0;
                    }
                    flush(&mut queue, &store, &insert, stream, 1).await;
                }
            }
        }
    });
}

async fn flush<T, F>(
    queue: &mut FlushQueue<T>,
    store: &Arc<dyn ObservabilityStore>,
    insert: &F,
    stream: &'static str,
    min_len: usize,
) where
    T: Send + Sync + Clone + 'static,
    F: Fn(Arc<dyn ObservabilityStore>, Vec<T>) -> InsertFuture + Send + Sync,
{
    let Some(batch) = queue.take_ready(std::time::Instant::now(), min_len) else {
        return;
    };
    // Cloned so the batch can be requeued on failure — the insert consumes it.
    match insert(store.clone(), batch.clone()).await {
        Ok(()) => queue.on_success(),
        Err(e) => {
            let batch_len = batch.len();
            let dropped = queue.on_failure(batch, std::time::Instant::now());
            tracing::error!(
                stream,
                requeued = batch_len - dropped.min(batch_len),
                dropped,
                error = %e,
                "custom-app telemetry insert failed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default state for every developer running `oxy serve` without
    /// `OXY_OBSERVABILITY_BACKEND`. Recording must be a cheap no-op there, not
    /// a panic and not a log line per served request.
    ///
    /// Note this asserts the *uninstalled* path specifically: these tests share
    /// a process with nothing that installs the sink, so `SINKS` is empty.
    #[test]
    fn recording_without_a_sink_is_a_silent_no_op() {
        assert!(!is_enabled());
        record_event(CustomAppEventRecord {
            timestamp_ms: 0,
            org_id: "o".into(),
            app_id: "a".into(),
            build_id: String::new(),
            request_id: String::new(),
            session_id: String::new(),
            user_id: String::new(),
            kind: "serve".into(),
            route: "/".into(),
            status: 200,
            duration_ms: 1,
            host_calls: 0,
            init_ms: 0,
            bytes: 0,
            app_role: String::new(),
            outcome: "ok".into(),
            error_kind: String::new(),
            error_detail: String::new(),
            trace_id: String::new(),
            span_id: String::new(),
        });
        record_logs(vec![CustomAppLogRecord {
            timestamp_ms: 0,
            org_id: "o".into(),
            app_id: "a".into(),
            build_id: String::new(),
            invocation_id: String::new(),
            request_id: String::new(),
            function_name: "f".into(),
            mode: "route".into(),
            log_level: "info".into(),
            seq: 0,
            message: "hi".into(),
            trace_id: String::new(),
            span_id: String::new(),
        }]);
    }
}
