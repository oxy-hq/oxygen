//! Telling the host that a root run stopped to ask the person something.
//!
//! The third sibling of [`super::failure_hook`] and [`super::done_hook`]. A
//! run that needs the person's input (a clarifying question) is written
//! `awaiting_input` with its prompt in one place (the coordinator's
//! `handle_suspended`), and the host (`oxy-app`) may want to tell somebody who
//! left — the person who asked to hear about the run. This crate cannot know
//! who, so the host installs one [`RunSuspendedObserver`] at boot and the
//! coordinator calls [`root_run_suspended`] after the suspension is written.
//!
//! The observer is handed the run id only and reads the row and its
//! suspension itself. A run can stop to ask more than once; each suspension is
//! a separate call. Unset, every call is a no-op — which is what tests and the
//! CLI get.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

/// What the host does when a root run has stopped for the person's input.
#[async_trait]
pub trait RunSuspendedObserver: Send + Sync {
    /// Called after the run's `awaiting_input` status and its suspension are
    /// committed. Must not fail the caller: whatever goes wrong is the
    /// observer's to log.
    async fn root_run_suspended(&self, run_id: &str);
}

static OBSERVER: OnceLock<Arc<dyn RunSuspendedObserver>> = OnceLock::new();

/// Install the observer. First call wins; the host does this once at boot.
pub fn set_run_suspended_observer(observer: Arc<dyn RunSuspendedObserver>) {
    if OBSERVER.set(observer).is_err() {
        tracing::warn!("run suspended observer already installed; ignoring the second");
    }
}

/// Tell the installed observer, if any, that root run `run_id` is waiting on
/// the person. Awaited rather than spawned, as the other run hooks are.
pub async fn root_run_suspended(run_id: &str) {
    if let Some(observer) = OBSERVER.get() {
        observer.root_run_suspended(run_id).await;
    }
}
