//! Telling the host that a root run finished with an answer.
//!
//! The success sibling of [`super::failure_hook`]. The coordinator writes a
//! root run `done` in one place (`handle_done`), and the host (`oxy-app`) may
//! want to tell somebody — a person who asked to hear when their answer is
//! ready. This crate cannot know who, so the host installs one
//! [`RunDoneObserver`] at boot and the coordinator calls [`root_run_done`]
//! after the `done` status is written.
//!
//! The observer is handed the run id only and reads the row itself, so it
//! decides what counts (which domain, whether anyone asked) from the record.
//! Unset, every call is a no-op — which is what tests and the CLI get.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

/// What the host does when a root run has finished with an answer.
#[async_trait]
pub trait RunDoneObserver: Send + Sync {
    /// Called after the run's `done` status is committed. Must not fail the
    /// caller: whatever goes wrong is the observer's to log.
    async fn root_run_done(&self, run_id: &str);
}

static OBSERVER: OnceLock<Arc<dyn RunDoneObserver>> = OnceLock::new();

/// Install the observer. First call wins; the host does this once at boot.
pub fn set_run_done_observer(observer: Arc<dyn RunDoneObserver>) {
    if OBSERVER.set(observer).is_err() {
        tracing::warn!("run done observer already installed; ignoring the second");
    }
}

/// Tell the installed observer, if any, that root run `run_id` is done.
///
/// Awaited rather than spawned, as [`super::failure_hook::root_run_failed`]
/// is: the observer only records and queues, and a spawn would be lost with
/// the pod between the terminal write and the record.
pub async fn root_run_done(run_id: &str) {
    if let Some(observer) = OBSERVER.get() {
        observer.root_run_done(run_id).await;
    }
}
