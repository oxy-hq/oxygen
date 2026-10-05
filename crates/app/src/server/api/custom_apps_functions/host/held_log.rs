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
//!
//! The row itself is built by the one `app.staging.held` writer,
//! [`custom_apps_staging_held::record_held`], which the console's held list
//! reads back; this log decides only *when* it is written.

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use sentry::SentryFutureExt;
use uuid::Uuid;

use super::super::data_audit::{InvocationIdentity, WriteBuffer, WriteRecord};
use crate::server::api::custom_apps_staging_held::{self, HeldActor, HeldRow};

/// Everything a held row is written from, cloneable onto a task.
#[derive(Clone)]
struct RowWriter {
    db: DatabaseConnection,
    identity: InvocationIdentity,
    app_id: Uuid,
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
    fn row(&self, writes: Vec<WriteRecord>, trace_id: Option<String>) -> HeldRow {
        let id = &self.identity;
        HeldRow {
            app_id: self.app_id,
            app_slug: id.app_slug.clone(),
            org_id: self.org_id,
            project_id: self.project_id,
            environment: self.environment.clone(),
            actor: match id.user_id {
                Some(user) => HeldActor::User {
                    id: user,
                    email: id.user_email.clone(),
                },
                None => HeldActor::System,
            },
            function_or_surface: id.function_name.clone(),
            mode: id.mode.clone(),
            request_id: id.request_id,
            writes,
            invocation_id: Some(id.invocation_id),
            trace_id,
        }
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
                let row = writer.row(held, trace_id);
                custom_apps_staging_held::record_held(&writer.db, row).await;
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
        app_id: Uuid,
        org_id: Uuid,
        project_id: Uuid,
        environment: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            buffer: tokio::sync::Mutex::new(WriteBuffer::new()),
            writer: RowWriter {
                db,
                identity,
                app_id,
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
                let row = log.writer.row(held, trace_id);
                custom_apps_staging_held::record_held(&log.writer.db, row).await;
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
        let row = self.writer.row(held, None);
        let db = self.writer.db.clone();
        handle.spawn(
            async move { custom_apps_staging_held::record_held(&db, row).await }
                // The invocation's hub, captured when the log was made (see
                // `RowWriter::hub`), not whatever is current where it drops.
                .bind_hub(Arc::clone(&self.writer.hub)),
        );
    }
}
