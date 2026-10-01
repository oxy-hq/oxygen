//! What an [`OxyProjectContext`] built for a workspace-preview request does
//! instead of writing (`server::previews::request_hold`).
//!
//! The context is the platform every request-path run executes on — a chat
//! run and the children it drives, a data app's tasks, the metric-tree tools —
//! so its answers are where a preview's writes stop:
//!
//! * every connector it hands out is held (reads sent, writes refused), and it
//!   resolves no bare `ConnectorConfig`, which the pipeline would build
//!   unwrapped;
//! * it reports `is_workspace_preview`, so agentic-pipeline builds no
//!   automation runner and hands out no builder bridges, and an Airway step is
//!   refused;
//! * an `http_request` step sends only `GET`/`HEAD`; no secret is persisted and
//!   no pipeline destination resolved;
//! * the anomaly, monitor-scan and compile ports are off — each writes.
//!
//! The trait impls consult [`OxyProjectContext::holds_writes`]; the decisions
//! that are more than a line live here.

use std::sync::Arc;

use agentic_connector::DatabaseConnector;

use super::OxyProjectContext;
use crate::server::previews::request_hold;

impl OxyProjectContext {
    /// Whether this context holds every write: it was built for a
    /// workspace-preview request, or is being used inside one.
    pub fn holds_writes(&self) -> bool {
        self.holds_writes || request_hold::active()
    }

    /// `conn` for `database`, held when this context holds writes.
    pub(super) fn held(
        &self,
        conn: Arc<dyn DatabaseConnector>,
        database: &str,
    ) -> Arc<dyn DatabaseConnector> {
        request_hold::hold_if(self.holds_writes(), conn, database)
    }

    /// The connector a held context hands the pipeline for `db_name` — every
    /// database, held — or `None` (logged) when it cannot be built.
    pub(super) async fn held_pre_built(&self, db_name: &str) -> Option<Arc<dyn DatabaseConnector>> {
        match self.build_connector_for(db_name).await {
            Ok(conn) => Some(conn),
            Err(e) => {
                tracing::warn!(db = %db_name, error = %e, "preview: connector build failed");
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "preview_hold_tests.rs"]
mod tests;
