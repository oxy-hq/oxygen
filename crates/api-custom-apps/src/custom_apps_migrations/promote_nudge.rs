//! A production promote nudges the change stream relay (OLTP change stream
//! design §5.3): before its migration files apply, so what they change is
//! drained, and best effort again after capture is installed, for a commit
//! that took longer than the relay's margin.

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::types::MigrationError;
use crate::oltp_changes::relay::marks;

/// The environment a promote applies to.
const ENVIRONMENT: &str = "production";

/// Nudge before the migration files apply, when the build captures changes
/// and the org has the flag. A failure is the platform's, never the file's:
/// [`MigrationError::Infra`], retryable.
pub(super) async fn nudge_before_apply(
    db: &DatabaseConnection,
    org_id: Uuid,
    captures_changes: bool,
) -> Result<(), MigrationError> {
    if !captures_changes || !crate::oltp_changes::enabled_for(org_id) {
        return Ok(());
    }
    marks::nudge(db, org_id, ENVIRONMENT)
        .await
        .map_err(|e| MigrationError::Infra {
            filename: "the change stream nudge".to_owned(),
            message: format!("could not record the change stream's nudge (platform-side): {e}"),
        })
}

/// Nudge again once capture is installed. Best effort: logged, never failing
/// the promote.
pub(crate) async fn nudge_after_install(
    db: &DatabaseConnection,
    org_id: Uuid,
    captures_changes: bool,
) {
    if !captures_changes || !crate::oltp_changes::enabled_for(org_id) {
        return;
    }
    if let Err(e) = marks::nudge(db, org_id, ENVIRONMENT).await {
        tracing::warn!(
            "promote: could not nudge the change stream after install for org {org_id}: {e}"
        );
    }
}
