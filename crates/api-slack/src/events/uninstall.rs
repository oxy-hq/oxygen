use crate::services::installations::InstallationsService;
use entity::slack_installations::Model as InstallationRow;
use oxy_shared::errors::OxyError;

/// Marks the installation revoked. Cascades delete user_links, user_preferences,
/// slack_threads via FK on_delete rules on those tables.
///
/// Idempotent — safe to call when already revoked (no-ops).
pub async fn revoke(installation: InstallationRow) -> Result<(), OxyError> {
    if installation.revoked_at.is_some() {
        return Ok(());
    }
    InstallationsService::revoke(installation.id).await
}
