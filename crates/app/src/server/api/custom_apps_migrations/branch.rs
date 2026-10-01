//! The bundle's OLTP migrations, applied to the org's **staging branch** on
//! every publish (previews P4b; env design §4.3).
//!
//! Every publish moves staging's pointer, so staging's copy of the app's
//! schema must carry the build's tables. When the org has an active staging
//! branch, the same files run there — as the app's writer on the branch,
//! through `oxy_oltp`'s branch resolver, which refuses a branch row that names
//! production — and are recorded under the branch's own ledger target,
//! `branch:<provider id>` ([`MigrationTarget::Branch`]). Production's ledger
//! and database are never read or written here.
//!
//! **It never fails the publish, and never holds it up.** The publish queues
//! it as the last thing it does (`custom_apps_nonproduction::staging_task`),
//! and the worker fleet runs it. The session carries a
//! [`BRANCH_LOCK_TIMEOUT`] and a [`BRANCH_STATEMENT_TIMEOUT`], and the whole
//! step a [`BRANCH_APPLY_TIMEOUT`]. A failure — a file that does not apply to
//! the copy, a branch mid-reset, a writer provisioned after the cut, a
//! timeout — is recorded as the task's failure, [`branch_warning`].
//!
//! The branch starts with production's ledger (a cut or reset copies it,
//! `oxy_oltp::provisioner`), so a file production already ran is not run
//! again on a copy that already has its tables. Each file is recorded only
//! while the branch is still the cut the apply connected to
//! (`oxy_oltp::branches::still_current`, in the record's transaction).
//!
//! **"already exists" on the branch.** A file whose DDL committed on the
//! branch but whose ledger row was not written — the step timed out or was
//! dropped between the two, or a reset landed in between — is run again by
//! the next publish, and fails there with Postgres's `already exists`. That is
//! the loud direction (never a silent skip), and it only warns. `oxyc oltp
//! reset --branch staging` fixes it: the fresh copy's ledger is production's,
//! and the next publish applies staging's files to it from there.

use std::time::Duration;

use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use oxy_oltp::resolver::ResolveError;

use super::apply::{Destination, app_writer, apply_to, nothing_pending};
use super::types::{Applied, DeclaredMigration, MigrationError, MigrationTarget};

/// How long a statement on the branch waits for a lock before it gives up.
pub(crate) const BRANCH_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one statement on the branch may run.
pub(crate) const BRANCH_STATEMENT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the whole branch step may take before it is stopped.
pub(crate) const BRANCH_APPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// The branch session's limits, set before the apply lock is taken.
fn session_setup() -> String {
    format!(
        "SET lock_timeout = '{}ms'; SET statement_timeout = '{}ms'",
        BRANCH_LOCK_TIMEOUT.as_millis(),
        BRANCH_STATEMENT_TIMEOUT.as_millis()
    )
}

/// Apply `declared` to the org's staging branch, if it has one, within
/// [`BRANCH_APPLY_TIMEOUT`]. `Ok(None)`: the org has no staging branch.
#[instrument(skip(db, declared), fields(app_id = %app_id, app_slug = %app_slug))]
pub(crate) async fn apply_to_staging_branch(
    db: &DatabaseConnection,
    app_id: Uuid,
    app_slug: &str,
    org_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
) -> Result<Option<Applied>, MigrationError> {
    if declared.is_empty() {
        return Ok(None);
    }
    let applying = apply_on_branch(db, app_id, app_slug, org_id, build_pk, declared);
    let outcome = match tokio::time::timeout(BRANCH_APPLY_TIMEOUT, applying).await {
        Ok(outcome) => outcome,
        Err(_) => Err(MigrationError::Infra {
            filename: String::new(),
            message: format!(
                "the step did not finish within {}s and was stopped",
                BRANCH_APPLY_TIMEOUT.as_secs()
            ),
        }),
    };
    match &outcome {
        Ok(Some(applied)) if !applied.summary().is_empty() => {
            info!("staging branch {} for app {app_id}", applied.summary());
        }
        Err(e) => warn!("staging branch migrations failed for app {app_id}: {e}"),
        _ => {}
    }
    outcome
}

/// What a failed branch apply is recorded with.
pub(crate) fn branch_warning(e: &MigrationError) -> String {
    format!(
        "the org's OLTP staging branch was not migrated ({e}). The publish went ahead; \
         staging's ctx.oltp may miss this build's tables until a later publish migrates the \
         branch, or `oxyc oltp reset --branch staging` re-copies it from production"
    )
}

/// `Ok(None)`: the org has no staging branch.
async fn apply_on_branch(
    db: &DatabaseConnection,
    app_id: Uuid,
    app_slug: &str,
    org_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
) -> Result<Option<Applied>, MigrationError> {
    let writer = app_writer(app_slug)?;
    let resolved = oxy_oltp::resolver::resolve_branch_writer_for_org(
        db,
        org_id,
        oxy_oltp::OltpBranch::Staging,
        &writer,
    )
    .await;
    let branch = match resolved {
        Ok(Some(branch)) => branch,
        // No branch, or no OLTP database for one to be cut from.
        Ok(None) | Err(ResolveError::NotProvisioned(_)) => return Ok(None),
        Err(e) => {
            return Err(MigrationError::Infra {
                filename: String::new(),
                message: format!("the staging branch is not usable: {e}"),
            });
        }
    };
    // The branch this connection reaches, read from the same row: a reset
    // that re-cut it under a new id in between cannot misfile these rows.
    let target = MigrationTarget::Branch(branch.cut.provider_branch_id.clone());
    if let Some(done) = nothing_pending(db, app_id, declared, &target).await? {
        return Ok(Some(done));
    }
    apply_to_resolved_branch(db, app_id, build_pk, declared, &branch)
        .await
        .map(Some)
}

/// The staging apply for a writer the caller already resolved — the seam the
/// reset-race test drives, resolving before a reset and applying after it.
#[doc(hidden)]
pub async fn apply_to_resolved_branch(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
    branch: &oxy_oltp::resolver::BranchWriter,
) -> Result<Applied, MigrationError> {
    let setup = session_setup();
    let dest = Destination {
        conn: &branch.connection,
        target: MigrationTarget::Branch(branch.cut.provider_branch_id.clone()),
        session_setup: Some(&setup),
        cut: Some(&branch.cut),
    };
    apply_to(db, app_id, build_pk, declared, &dest).await
}
