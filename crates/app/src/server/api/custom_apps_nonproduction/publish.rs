//! What a publish does for staging — and the rule it does it under:
//! **nothing staging may fail or delay a publish** it did not already block
//! before staging existed.
//!
//! - The `nonProduction` mapping is checked before a byte is stored
//!   ([`mapping_warning`]). An author error still refuses the publish; a
//!   workspace with no compiled config to check it by refuses a staging-only
//!   publish (409, retry) but only warns on a promoting one — the host checks
//!   the mapping again on every staging write, failing closed.
//! - Staging's homes — the sibling Airhouse schema and the org's OLTP staging
//!   branch — are migrated by tasks the publish **queues** as the last thing
//!   it does ([`super::staging_task`]) and the worker fleet runs
//!   ([`super::staging_task_executor`]). The publish waits for none of it; a
//!   task that cannot be queued is a warning, and one that fails is recorded
//!   as its run's failure.

use uuid::Uuid;

use super::{MappingRefusal, check_mapping};

/// Check the build's `nonProduction` block. `Ok(Some(warning))`: it could not
/// be checked, and the publish promotes, so it goes on.
pub async fn mapping_warning(
    project_id: Uuid,
    manifest_json: Option<&serde_json::Value>,
    promote: bool,
) -> Result<Option<String>, MappingRefusal> {
    match check_mapping(project_id, manifest_json).await {
        Ok(()) => Ok(None),
        Err(MappingRefusal::Unchecked(why)) if promote => {
            tracing::warn!(%project_id, "publish: {why}");
            Ok(Some(format!(
                "{why}. Production shipped; staging checks the mapping on every write and refuses \
                 one it cannot confirm"
            )))
        }
        Err(refusal) => Err(refusal),
    }
}
