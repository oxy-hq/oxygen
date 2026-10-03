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
//!
//! A sandbox admitted with its own schema on the branch resolves the same
//! writer there with that schema as the only `search_path` entry
//! (`oxy_oltp::sandbox_schema`), and only on the cut the admission found the
//! schema ready on: a branch reset since replaced the database it was in.

use oxy_oltp::OltpBranch;
use oxy_oltp::resolver::{
    WriterConnection, resolve_branch_writer_for_org, resolve_writer_connection_for_org,
};
use oxy_oltp::sandbox_schema::resolve_branch_sandbox_writer_for_org;
use oxy_oltp::schema::WriterRef;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::super::env_policy::{OltpHome, REFUSED_LABEL, SandboxHome};

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
        OltpHome::SandboxSchema(home) => sandbox_connection(db, org_id, &writer, home).await,
        // Every op is refused before it connects (`oltp_guard`); a path that
        // reaches here anyway gets no connection, never staging's schema.
        OltpHome::SandboxUnready(why) => Err(format!("{REFUSED_LABEL}: {}.", why.fix())),
    }
}

/// The writer on the org's staging branch, in the sandbox's own schema — and
/// only while the branch is still the cut the admission found it ready on.
async fn sandbox_connection(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer: &WriterRef,
    home: &SandboxHome,
) -> Result<WriterConnection, String> {
    let schema = &home.schema;
    let resolved =
        resolve_branch_sandbox_writer_for_org(db, org_id, OltpBranch::Staging, writer, schema)
            .await;
    match resolved {
        Ok(Some(branch)) if branch.cut == home.cut => Ok(branch.connection),
        Ok(Some(_)) => Err(format!(
            "the org's OLTP staging branch was reset or re-cut since this invocation was \
             admitted, which removed this sandbox's schema {schema}; nothing was sent. Publish \
             to the sandbox again"
        )),
        Ok(None) => Err(format!(
            "this invocation was admitted to the sandbox's schema {schema} on the org's OLTP \
             staging branch, which no longer exists; nothing was sent"
        )),
        Err(e) => Err(format!(
            "this sandbox's schema {schema} on the org's staging branch cannot be used: {e}"
        )),
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
