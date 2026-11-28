//! The kernel driven port for writing `audit_events` rows: [`AuditSink`].
//!
//! A domain says *what* happened ([`AuditRow`]: the action, the org, the
//! target, the metadata, the change). *Who* did it and *where from* (the
//! request's user and credential, or the system) is the sink's. The sink is
//! also bound to the caller's connection, so a row commits or rolls back with
//! the change it records.
//!
//! The one production implementation is `oxy_app_core::audit::TxnAuditSink`.
//! A writer takes `&dyn AuditSink` as an argument, never an optional or
//! process-wide one, so a write with no sink does not compile.

use std::future::Future;
use std::pin::Pin;

use sea_orm::DbErr;
use sea_orm::prelude::Uuid;
use serde_json::Value;

/// The `target_type` every token lifecycle event carries. One definition: the
/// writers set it and `oxy_app_core::audit::events_for_token` reads by it.
pub const TOKEN_TARGET_TYPE: &str = "api_token";

/// One audit row, as a domain states it. The field names are the
/// `audit_events` columns they fill.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditRow {
    /// Versioned action name, e.g. `token.created`.
    pub action: &'static str,
    /// `None` writes an unchained row with no org.
    pub org_id: Option<Uuid>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub target_label: Option<String>,
    pub metadata: Value,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

/// The future [`AuditSink::record`] returns.
pub type RecordFuture<'a> = Pin<Box<dyn Future<Output = Result<(), DbErr>> + Send + 'a>>;

/// Writes audit rows for one actor, on one connection.
pub trait AuditSink: Send + Sync {
    /// Writes `row`. An error means the row was not written, and the caller
    /// must fail what it was recording: nothing is dropped quietly.
    fn record(&self, row: AuditRow) -> RecordFuture<'_>;
}
