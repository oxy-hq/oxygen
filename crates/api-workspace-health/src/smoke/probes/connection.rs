//! Warehouse connectivity probe: `SELECT 1` against one configured database.
//!
//! This is the same call chain the "Test connection" button in the database
//! settings UI uses (`server::api::database::test_database_connection`), minus
//! the SSE surface and the temp-secret dance — `build_connector_for` picks the
//! right connector for the warehouse type and surfaces the real error.

use std::sync::Arc;

use super::ProbeFailure;
use oxy_app::surface::workspace_runtime::OxyProjectContext;

pub(crate) async fn ping(ctx: &Arc<OxyProjectContext>, db_name: &str) -> Result<(), ProbeFailure> {
    // A connector we can't even build is a config/secret problem on our side,
    // not a broken warehouse — we learned nothing about the database itself.
    let connector = ctx
        .build_connector_for(db_name)
        .await
        .map_err(|e| ProbeFailure::Unavailable(scrub(format!("could not build connector: {e}"))))?;

    connector
        .execute_query("SELECT 1", 1)
        .await
        .map(|_| ())
        .map_err(|e| ProbeFailure::Broken(scrub(format!("SELECT 1 failed: {e}"))))
}

/// The probe's text is stored in the health payload and shown to anyone who
/// can read Workspace Health, and a connector built outside `agentic-connector`
/// (the core DuckLake attach) reports through `OxyError`, which is not
/// scrubbed on its own. So it is scrubbed here as well.
fn scrub(text: String) -> String {
    oxy_shared::redact::redact_credentials(&text)
}
