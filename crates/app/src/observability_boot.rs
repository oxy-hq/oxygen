//! Boot-time observability wiring.
//!
//! Observability needs the tracing subscriber installed *early* (before CLI
//! dispatch) so every span from startup is captured, but the backend store
//! needs its ClickHouse endpoint, which `oxy start` only boots *after*
//! startup begins. We bridge this gap by:
//!
//! 1. In `main.rs`, create the SpanCollectorLayer + its channel and install
//!    the layer into the subscriber — for the server commands only (`serve`,
//!    `start`, `worker`). Stash the receiver in [`stash_receiver`].
//! 2. Later, in each of those commands' boot, call [`finalize`] to resolve the
//!    backend, spawn the bridge task, and register the global store:
//!    - `serve.rs` (`start_server_and_web_app`, which `oxy start` delegates
//!      to) — by which point `OXY_CLICKHOUSE_*` is set for both paths
//!      (externally for `oxy serve`, by `oxy start` once its container is
//!      ready);
//!    - `worker.rs` (`run_worker`), before the run drivers start.
//!
//! Spans emitted between step 1 and step 2 accumulate in the unbounded channel
//! and get flushed as soon as the bridge spawns.
//!
//! Step 2 is not optional. A process that installs the layer and never calls
//! [`finalize`] keeps every span it closes, for as long as it lives: the
//! channel is unbounded and nothing reads it. `oxy worker` was that process
//! until 2026-09 — its latency worker closes a span every poll, and prod
//! worker pods grew ~10 MiB/h until the 1 GiB limit OOM-killed them. Every
//! other command (one-shot CLI, `oxy mcp`) never reaches a `finalize`, which
//! is why `main.rs` does not give them the layer. `entry_point_tests` holds
//! `main.rs`'s list and the `finalize` call sites together.
//!
//! If ClickHouse is unavailable at step 2, [`finalize`] keeps retrying in the
//! background ([`retry`]) instead of giving up. It used to drop the receiver on
//! the first failure, which disabled span capture for the life of the process:
//! on 2026-09-13 an oxy-prod node roll restarted the obs ClickHouse at the same
//! moment as the oxy pods, and every pod that booted in that window wrote zero
//! spans until it was restarted by hand ~35 minutes later.
//!
//! A store installed by the retry is only in the global, so request handlers
//! must read it through `AppState::observability()`, never the boot-time field.

#[cfg(test)]
mod entry_point_tests;
mod retry;

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use once_cell::sync::OnceCell;
use oxy::theme::StyledText;
use oxy_observability::backends::clickhouse::ClickHouseObservabilityStorage;
use oxy_observability::{ObservabilityStore, SpanRecord};
use tokio::sync::mpsc::UnboundedReceiver;

use retry::{OpenError, RETRY_POLICY, retry_until_open};

static PENDING_RECEIVER: OnceCell<Mutex<Option<UnboundedReceiver<SpanRecord>>>> = OnceCell::new();

/// How long the connectivity probe may take before ClickHouse counts as
/// unavailable. ClickHouse answers `SELECT 1` in milliseconds; this only fires
/// when it accepts the connection and then never replies — a terminating pod
/// still listed in the Service's endpoints, which is the 2026-09-13 node-roll
/// shape. The default client sets no timeout, so without this the open would
/// park: inline, it would hold boot; in the retry, it would stop retrying.
///
/// The probe is timed; `ensure_schema` after it deliberately is not. On a first
/// deploy it backfills the execution rollup from up to 90 days of spans, and
/// aborting that partway leaves a partial seed that its own guard ("any rollup
/// row older than an hour") then refuses to finish.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Stash the `SpanCollectorLayer` receiver created in `main.rs` so the serve
/// path can pick it up once the store is ready.
pub fn stash_receiver(rx: UnboundedReceiver<SpanRecord>) {
    let cell = PENDING_RECEIVER.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().expect("observability receiver mutex poisoned");
    if guard.is_some() {
        tracing::warn!("observability receiver already stashed; replacing");
    }
    *guard = Some(rx);
}

/// Take the stashed receiver, if any. Returns `None` when no layer was
/// installed (OXY_OBSERVABILITY_BACKEND was unset at startup) or when the
/// receiver was already taken. Panics on poison, symmetric with
/// [`stash_receiver`] — silently swallowing poison would make `finalize()`
/// a no-op and hide the underlying bug.
fn take_receiver() -> Option<UnboundedReceiver<SpanRecord>> {
    let cell = PENDING_RECEIVER.get()?;
    cell.lock()
        .expect("observability receiver mutex poisoned")
        .take()
}

/// Whether observability is enabled, from env. Strictly honors
/// `OXY_OBSERVABILITY_BACKEND` — no default, no silent fallbacks. When the env
/// var is unset, observability is disabled entirely. ClickHouse is the sole
/// backend; removed labels get a migration error via
/// [`oxy_observability::backends::validate_backend_label`].
fn backend_enabled() -> bool {
    let Ok(backend) = std::env::var("OXY_OBSERVABILITY_BACKEND") else {
        return false;
    };

    if let Err(e) = oxy_observability::backends::validate_backend_label(&backend) {
        // Deliberately non-fatal: a stale telemetry label should not take the
        // product down. But an explicitly-set-yet-invalid value is a stronger
        // signal than an unset one, so it goes to the structured log (where
        // cloud alerting can see it) as well as stderr.
        tracing::error!(backend = %backend, "{e}");
        eprintln!("{}", e.to_string().error());
        return false;
    }

    true
}

/// Open the ClickHouse observability store from `OXY_CLICKHOUSE_*` env.
async fn try_open_clickhouse_store() -> Result<Arc<dyn ObservabilityStore>, OpenError> {
    let storage = ClickHouseObservabilityStorage::from_env()
        .await
        .map_err(|e| OpenError::Config(format!("ClickHouse init failed: {e}")))?;
    open_store(storage, PROBE_TIMEOUT).await
}

/// Probe the server (bounded by `probe_timeout`), ensure the schema, and apply
/// retention TTL. A TTL failure is logged and does not fail the open.
async fn open_store(
    storage: ClickHouseObservabilityStorage,
    probe_timeout: Duration,
) -> Result<Arc<dyn ObservabilityStore>, OpenError> {
    match tokio::time::timeout(probe_timeout, storage.ping()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            return Err(OpenError::Unavailable(format!(
                "ClickHouse schema init failed: {e}"
            )));
        }
        Err(_) => {
            return Err(OpenError::Unavailable(format!(
                "ClickHouse schema init failed: no response to the connectivity probe within {probe_timeout:?}"
            )));
        }
    }

    storage
        .ensure_schema()
        .await
        .map_err(|e| OpenError::Unavailable(format!("ClickHouse schema init failed: {e}")))?;

    let retention_days = oxy_observability::RETENTION_DAYS;
    match storage.apply_retention_ttl(retention_days).await {
        // Retention is ClickHouse's job from here on: the TTL is enforced by
        // background merges, so there is no purge loop to run or monitor.
        Ok(()) => tracing::info!("Observability retention: {retention_days} days (ClickHouse TTL)"),
        // Structured, not just stderr: this exact failure went unnoticed for
        // months because `eprintln!` alone never reaches log-based alerting.
        // Retention silently not applying is how observability tables grow
        // without bound.
        Err(e) => {
            tracing::error!(error = %e, "ClickHouse TTL apply failed");
            eprintln!("{}", format!("ClickHouse TTL apply failed: {e}").error());
        }
    }

    Ok(Arc::new(storage) as Arc<dyn ObservabilityStore>)
}

/// Open the ClickHouse observability store once, for standalone CLI commands
/// ([`crate::observability_setup`]). Errors are printed loudly and yield
/// `None` — callers decide whether that is fatal. No retry: a one-shot command
/// should fail fast, unlike the long-lived server in [`finalize`].
pub(crate) async fn open_clickhouse_store() -> (Option<Arc<dyn ObservabilityStore>>, Option<String>)
{
    match try_open_clickhouse_store().await {
        Ok(store) => (
            Some(store),
            Some("Observability: clickhouse (OXY_CLICKHOUSE_URL)".to_string()),
        ),
        Err(e) => {
            eprintln!("{}", e.message().error());
            (None, None)
        }
    }
}

/// Spawn the bridge task against `receiver` and register the global store.
fn install(receiver: UnboundedReceiver<SpanRecord>, store: Arc<dyn ObservabilityStore>) {
    tracing::info!("Observability: clickhouse (OXY_CLICKHOUSE_URL)");
    oxy_observability::spawn_bridge(receiver, Arc::clone(&store));
    // Custom-app wide events and function logs ride their own bridges rather
    // than the span channel: they are not spans, they are far higher volume,
    // and a burst of app traffic must not be able to evict trace spans from a
    // shared buffer. Installed here, alongside the store, so `record_event` is
    // a no-op everywhere `OXY_OBSERVABILITY_BACKEND` is unset — which is every
    // developer's default `oxy serve`.
    oxy_observability::spawn_custom_app_bridges(Arc::clone(&store));
    oxy_observability::global::set_global(store);
}

/// Resolve the backend, spawn the bridge task against the stashed receiver,
/// and register the global store.
///
/// Called from `serve.rs` once `OXY_CLICKHOUSE_*` is guaranteed set, and from
/// `worker.rs` before the run drivers start — every command `main.rs`
/// installs the layer for (see the module doc). Safe to call when no receiver
/// was stashed (OXY_OBSERVABILITY_BACKEND unset) — it becomes a no-op.
///
/// The first attempt runs inline, so on a healthy ClickHouse the store is
/// registered before the router serves. Boot waits for it at most
/// [`PROBE_TIMEOUT`] when ClickHouse accepts connections but never answers,
/// plus however long `ensure_schema` legitimately takes on a reachable one.
/// Any `Unavailable` failure is logged and retried in a background task (see
/// [`retry::retry_until_open`]). Until that succeeds, spans are held up to
/// `RETRY_POLICY.max_pending` and the global store stays unset, exactly as if
/// observability were off.
///
/// Lifetime contract: if `start_server_and_web_app` bails before reaching
/// this point (e.g. migrations fail), the stashed receiver and tracing
/// sender stay alive for the rest of the process lifetime, buffering spans
/// into an unbounded channel. This is benign in practice because startup
/// failures exit the process quickly; [`shutdown`] explicitly drops the
/// receiver so the accumulated buffer is released on clean exit.
pub async fn finalize() {
    let Some(mut receiver) = take_receiver() else {
        return;
    };

    if !backend_enabled() {
        // Loud error already printed for an invalid label. Drop the receiver
        // so the unbounded channel stops buffering indefinitely.
        drop(receiver);
        return;
    }

    match try_open_clickhouse_store().await {
        Ok(store) => install(receiver, store),
        Err(OpenError::Config(msg)) => {
            tracing::error!(error = %msg, "Observability store misconfigured; span capture disabled");
            eprintln!("{}", msg.error());
            drop(receiver);
        }
        Err(OpenError::Unavailable(msg)) => {
            tracing::error!(
                error = %msg,
                "Observability store unavailable at boot; retrying in the background"
            );
            eprintln!("{}", msg.error());
            tokio::spawn(async move {
                if let Some(store) =
                    retry_until_open(&mut receiver, try_open_clickhouse_store, RETRY_POLICY).await
                {
                    install(receiver, store);
                }
            });
        }
    }
}

/// Shut down the global observability store, if set. Also drops any
/// receiver left in [`PENDING_RECEIVER`] — this only happens when startup
/// failed before [`finalize`] ran, but we release the buffered channel
/// here so it doesn't outlive the store.
pub async fn shutdown() {
    let _ = take_receiver();
    if let Some(store) = oxy_observability::global::get_global() {
        store.shutdown().await;
    }
    oxy_observability::shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review finding on #3183: a ClickHouse that accepts the TCP connection
    /// and never answers must count as unavailable within the probe timeout,
    /// not park the caller — inline that would hold boot, in the retry it
    /// would stop the retries.
    #[tokio::test]
    async fn unresponsive_clickhouse_is_unavailable_within_probe_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept connections and hold them open without ever writing a byte.
        let _server = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let storage = ClickHouseObservabilityStorage::new(
            &format!("http://{addr}"),
            "default",
            "",
            "observability",
        )
        .unwrap();

        let started = std::time::Instant::now();
        let result = open_store(storage, Duration::from_millis(200)).await;

        assert!(
            matches!(result, Err(OpenError::Unavailable(_))),
            "got {result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
