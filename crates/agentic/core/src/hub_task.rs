//! Spawning that keeps the Sentry hub.
//!
//! A Sentry hub is bound to a task, not inherited by one: `tokio::spawn`
//! polls its future under the worker thread's default hub, so whatever the
//! spawning task's hub carried is gone from the first poll. What it carries
//! in this workspace is the scope tag `oxy-app` sets on a custom-app request
//! (`oxy.surface`), which is how `before_send` keeps a tenant's errors and
//! panics out of Sentry — and a pipeline run, an automation step or a
//! connector query hopping onto another task inside these crates was losing
//! it. The three functions here read [`Hub::current`] synchronously on the
//! spawning task and bind it, the same carry `oxy-app`'s `sentry_surface`
//! makes at its own spawns.
//!
//! This crate learns nothing about the tag: it carries whichever hub is
//! current, tagged or not, and nothing here reads or sets scope. The read is
//! a thread-local `Arc` clone and the bind one `Arc` per task — cheap, and
//! infallible. `sentry-core` is built with its `client` feature so
//! [`Hub::current`] is the real per-thread hub; without it the crate compiles
//! every hub to a no-op and there would be nothing to carry.
//!
//! These are the only request-scoped spawns the agentic crates make.
//! `oxy-app`'s `sentry_surface` scan test (`every_agentic_spawn_carries_a_hub`)
//! fails on a bare `tokio::spawn` / `spawn_blocking` under `crates/agentic`,
//! and keeps the allowlist of what legitimately stays bare: boot-time loops
//! with no request hub to inherit, and a task that outlives the request that
//! started it (the connector's memoized Postgres connection driver), where a
//! carried hub would misattribute everything it later captures.
//! `agentic-http`, which may not name this crate, reaches the same functions
//! as `agentic_runtime::hub_task`.

use std::future::Future;

use sentry_core::{Hub, SentryFuture, SentryFutureExt};
use tokio::task::JoinHandle;

/// `tokio::spawn`, with the spawning task's hub bound across every poll of
/// `future`.
pub fn spawn_with_hub<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(bind_current_hub(future))
}

/// `tokio::task::spawn_blocking`, with the spawning task's hub current for
/// the whole of `f`.
pub fn spawn_blocking_with_hub<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let hub = Hub::current();
    tokio::task::spawn_blocking(move || Hub::run(hub, f))
}

/// Bind the current hub onto `future` for an executor the two functions
/// above cannot stand in for — a `JoinSet`, a runtime `Handle`, a `LocalSet`.
pub fn bind_current_hub<F: Future>(future: F) -> SentryFuture<F> {
    future.bind_hub(Hub::current())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry_core::test::with_captured_events;
    use sentry_core::{Level, capture_message};

    /// Every message captured on the *spawning* task's hub while `body` runs
    /// on a multi-thread runtime. `with_captured_events` binds a hub with a
    /// test client on this thread; a worker thread's default hub has no client
    /// at all, so a spawn that drops the hub captures nothing — which makes
    /// the bare spawns below the control for the carried ones.
    fn captured_on_the_spawning_hub<F>(body: F) -> Vec<String>
    where
        F: FnOnce(&tokio::runtime::Runtime),
    {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime");
        with_captured_events(|| body(&runtime))
            .into_iter()
            .map(|event| event.message.unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_spawned_future_keeps_the_hub_only_through_the_wrapper() {
        let captured = captured_on_the_spawning_hub(|runtime| {
            runtime.block_on(async {
                spawn_with_hub(async { capture_message("carried", Level::Error) })
                    .await
                    .expect("carried task");
                tokio::spawn(async { capture_message("bare", Level::Error) })
                    .await
                    .expect("bare task");
            });
        });
        assert_eq!(captured, vec!["carried"]);
    }

    #[test]
    fn a_blocking_closure_keeps_the_hub_only_through_the_wrapper() {
        let captured = captured_on_the_spawning_hub(|runtime| {
            runtime.block_on(async {
                spawn_blocking_with_hub(|| capture_message("carried", Level::Error))
                    .await
                    .expect("carried closure");
                tokio::task::spawn_blocking(|| capture_message("bare", Level::Error))
                    .await
                    .expect("bare closure");
            });
        });
        assert_eq!(captured, vec!["carried"]);
    }

    #[test]
    fn a_join_set_future_keeps_the_hub_only_when_bound() {
        let captured = captured_on_the_spawning_hub(|runtime| {
            runtime.block_on(async {
                let mut set = tokio::task::JoinSet::new();
                set.spawn(bind_current_hub(async {
                    capture_message("carried", Level::Error)
                }));
                set.join_next()
                    .await
                    .expect("one task")
                    .expect("carried task");
                set.spawn(async { capture_message("bare", Level::Error) });
                set.join_next().await.expect("one task").expect("bare task");
            });
        });
        assert_eq!(captured, vec!["carried"]);
    }
}
