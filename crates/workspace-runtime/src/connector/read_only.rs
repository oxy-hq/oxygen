//! The read-only Airhouse connector a react run queries through
//! ([`crate::read_only_airhouse`]). Skips the preview write hold, like every
//! builder here, so only `OxyProjectContext` may call it
//! (`unheld_callers_guard`), and it answers `None` while holding writes.

use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use oxy::config::model::DatabaseType;

use super::{CredentialUse, build_airhouse_connector};

/// A read-only connector for `db_name` when it is `airhouse_managed` and
/// Airhouse mints read-only credentials; `None` otherwise (another database
/// type, a static airhouse with no mint, or an Airhouse that predates
/// read-only credentials, logged). `subject` and `role` only place the run:
/// the credential is a Reader for the workspace whoever asks.
pub(crate) async fn resolve_read_only_airhouse(
    db_name: &str,
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Option<Arc<dyn DatabaseConnector>> {
    let db = workspace_manager
        .config_manager
        .resolve_database(db_name)
        .ok()?;
    if !matches!(db.database_type, DatabaseType::AirhouseManaged(_)) {
        return None;
    }
    let read_only = CredentialUse::ReadOnly;
    match build_airhouse_connector(&db, workspace_manager, (subject, role), read_only).await {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::info!(db = %db.name, "no read-only airhouse connector: {e}");
            None
        }
    }
}
