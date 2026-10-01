//! The held-write log of one invocation: every call its environment policy
//! held or refused, written as **one** `app.staging.held` audit row.
//!
//! Three exits, none of which may lose a held call (Tay's review condition):
//!
//! - **The invocation ends** (`end_of_invocation`, on success, error, timeout
//!   and cancel alike): [`HeldLog::flush`] drains and writes the row **inside
//!   a spawned task**, so a handler future dropped mid-flush — a client gone —
//!   cannot take the row with it. The buffer closes on the drain.
//! - **A call is noted after that** (a detached host call finishing after a
//!   timeout): the closed buffer hands it back and it is written on its own
//!   row, again from a spawned task.
//! - **The invocation never reaches its end** (the host dropped first): the
//!   last reference's `Drop` writes whatever is still buffered.
//!
//! Production notes nothing, so a production invocation's flush finds the
//! buffer empty and closes it without spawning anything.

use std::sync::Arc;

use oxy_app_core::audit::AuditEntry;
use sea_orm::DatabaseConnection;
use sentry::SentryFutureExt;
use uuid::Uuid;

use super::super::data_audit::{self, InvocationIdentity, WriteBuffer, WriteRecord};

/// Everything a held row is written from, cloneable onto a task.
#[derive(Clone)]
struct RowWriter {
    db: DatabaseConnection,
    identity: InvocationIdentity,
    org_id: Uuid,
    project_id: Uuid,
    environment: String,
    /// The invocation's Sentry hub — the one `tag_custom_app_surface` tagged,
    /// which the staging no-page filter recognises a custom app's capture by —
    /// captured when the log is made. [`HeldLog`]'s `Drop` cannot read it with
    /// `Hub::current()`: a dropped future's destructor runs with no hub
    /// switched in (`SentryFuture` binds only while polling), so there it is
    /// the pool thread's.
    hub: Arc<sentry::Hub>,
}

impl RowWriter {
    fn entry(&self, held: &[WriteRecord], trace_id: Option<&str>) -> AuditEntry {
        data_audit::entry(
            data_audit::ACTION_STAGING_HELD,
            &self.identity,
            self.org_id,
            self.project_id,
            held,
            trace_id,
        )
        .environment(self.environment.clone())
    }

    /// Write `held` as one row from a task of its own, and wait for it. The
    /// write finishes even if the caller's future is dropped while waiting.
    async fn record_detached(&self, held: Vec<WriteRecord>, trace_id: Option<String>) {
        if held.is_empty() {
            return;
        }
        let writer = self.clone();
        let task = tokio::spawn(
            async move {
                let entry = writer.entry(&held, trace_id.as_deref());
                oxy_app_core::audit::record_best_effort(&writer.db, entry).await;
            }
            // Outlives the invocation's request hub, so it carries that hub.
            .bind_hub(sentry::Hub::current()),
        );
        let _ = task.await;
    }
}

pub(super) struct HeldLog {
    buffer: tokio::sync::Mutex<WriteBuffer>,
    writer: RowWriter,
}

impl HeldLog {
    pub(super) fn new(
        db: DatabaseConnection,
        identity: InvocationIdentity,
        org_id: Uuid,
        project_id: Uuid,
        environment: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            buffer: tokio::sync::Mutex::new(WriteBuffer::new()),
            writer: RowWriter {
                db,
                identity,
                org_id,
                project_id,
                environment,
                hub: sentry::Hub::current(),
            },
        })
    }

    /// Buffer one held call, or write it on its own row when the log has
    /// already been flushed.
    pub(super) async fn note(&self, record: WriteRecord, trace_id: Option<String>) {
        let late = self.buffer.lock().await.note(record);
        if let Some(record) = late {
            self.writer.record_detached(vec![record], trace_id).await;
        }
    }

    /// Close the log and write its row. The drain happens inside the spawned
    /// task, so nothing buffered is lost if this future is dropped; an empty
    /// log — every production invocation — closes in place.
    pub(super) async fn flush(self: &Arc<Self>, trace_id: Option<String>) {
        {
            let mut buffer = self.buffer.lock().await;
            if buffer.holds_nothing() {
                buffer.drain();
                return;
            }
        }
        let log = Arc::clone(self);
        let task = tokio::spawn(
            async move {
                let held = log.buffer.lock().await.drain();
                if held.is_empty() {
                    return;
                }
                let entry = log.writer.entry(&held, trace_id.as_deref());
                oxy_app_core::audit::record_best_effort(&log.writer.db, entry).await;
            }
            .bind_hub(sentry::Hub::current()),
        );
        let _ = task.await;
    }
}

impl Drop for HeldLog {
    /// The row for what is still buffered when the invocation never flushed.
    fn drop(&mut self) {
        let held = self.buffer.get_mut().drain();
        if held.is_empty() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                held = held.len(),
                "held-write log dropped with no runtime to record it"
            );
            return;
        };
        let entry = self.writer.entry(&held, None);
        let db = self.writer.db.clone();
        handle.spawn(
            async move { oxy_app_core::audit::record_best_effort(&db, entry).await }
                // The invocation's hub, captured when the log was made (see
                // `RowWriter::hub`), not whatever is current where it drops.
                .bind_hub(Arc::clone(&self.writer.hub)),
        );
    }
}
