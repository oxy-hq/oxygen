//! The connection a function's `ctx.oltp` opens, for the home its admission
//! decided (`env_policy::OltpHome`, previews P4b).
//!
//! Production resolves the app's writer on production's database, as before.
//! A staging run admitted with the org's staging branch resolves the same
//! writer **on the branch** — its own endpoint, database and password — and
//! only there: through `oxy_oltp`'s branch resolver, whose guard refuses a
//! branch row that names production (by id, or by endpoint and database in
//! any spelling). If the branch is gone or not active by the time a call
//! connects, or was re-cut under another id since the admission chose it, the
//! call fails; it never falls back to production, nor moves to another copy.

use oxy_oltp::OltpBranch;
use oxy_oltp::resolver::{
    WriterConnection, resolve_branch_writer_for_org, resolve_writer_connection_for_org,
};
use oxy_oltp::schema::WriterRef;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::super::env_policy::OltpHome;

/// The app writer `writer_name`'s connection in `org_id`, on `home`. Public so
/// the environments' differential test resolves both homes exactly as the
/// host does.
pub async fn writer_connection(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer_name: &str,
    home: &OltpHome,
) -> Result<WriterConnection, String> {
    let writer =
        WriterRef::app(writer_name).map_err(|e| format!("invalid writer '{writer_name}': {e}"))?;
    match home {
        OltpHome::Production => resolve_writer_connection_for_org(db, org_id, &writer)
            .await
            .map_err(|e| {
                // Names the app's own writer, and points at the org operator
                // rather than a CLI the app author can't run.
                format!(
                    "this app's OLTP store ('{writer_name}') is not provisioned yet — ask \
                     whoever operates this org to provision it: {e}"
                )
            }),
        OltpHome::StagingBranch(admitted) => {
            branch_connection(db, org_id, writer_name, &writer, admitted).await
        }
    }
}

/// The writer on the org's staging branch the admission chose — never
/// production's, and never a branch re-cut since under another id.
async fn branch_connection(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer_name: &str,
    writer: &WriterRef,
    admitted: &str,
) -> Result<WriterConnection, String> {
    match resolve_branch_writer_for_org(db, org_id, OltpBranch::Staging, writer).await {
        Ok(Some(branch)) if branch.cut.provider_branch_id == admitted => Ok(branch.connection),
        Ok(Some(branch)) => Err(format!(
            "the org's OLTP staging branch was re-cut since this invocation was admitted \
             ({admitted} is now {}); nothing was sent. Call the function again",
            branch.cut.provider_branch_id
        )),
        Ok(None) => Err(format!(
            "this invocation was admitted to the org's OLTP staging branch ({admitted}), which \
             no longer exists; nothing was sent. Call the function again"
        )),
        Err(e) => Err(format!(
            "this app's OLTP store ('{writer_name}') on the org's staging branch cannot be \
             used: {e}"
        )),
    }
}
