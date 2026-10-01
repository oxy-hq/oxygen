//! Seeding a workspace-preview procedure run: the one writer of a `preview:`
//! scoped root ref.
//!
//! The run is an ordinary automation run on the global queue — its row, its
//! `TaskSpec::Automation` — with one difference that does all the work: the
//! root `workflow_ref` is `preview:<run_id>:<ref>`. A pod that knows previews
//! drives the root with the preview platform (the host's `RunPlatformResolver`
//! finds the run in its registry) and strips the prefix for this run only. A
//! pod that does not resolves nothing under that name, and the run fails
//! instead of running as production.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AutomationRunError, validate_automation_ref};

/// Everything the seed needs. `run_id` is the host's registry id, which the
/// `agentic_runs` row shares.
pub struct PreviewAutomationSeed<'a> {
    pub run_id: &'a str,
    /// The unscoped, workspace-relative automation ref.
    pub target_ref: &'a str,
    pub variables: Option<Value>,
    pub workspace_id: Uuid,
    pub preview_key: &'a str,
    pub branch: &'a str,
}

/// Insert the run row and queue its root task, `TaskScope::Global`, on `db` —
/// pass the transaction that marked the host's registry row running, so a row
/// never exists without its task.
///
/// `metadata.workflow_ref` is the SCOPED ref on purpose: `retry_run` rebuilds a
/// retry from it, and a scoped ref is refused by the public validator, so a
/// preview run cannot be retried into a production run.
pub async fn seed_preview_automation_run<C: ConnectionTrait>(
    db: &C,
    seed: PreviewAutomationSeed<'_>,
) -> Result<(), AutomationRunError> {
    validate_automation_ref(seed.target_ref)?;
    let scoped = agentic_automation::preview_names::scoped(seed.run_id, seed.target_ref);
    let metadata = json!({
        "workflow_ref": scoped,
        "trigger": "preview",
        "preview_key": seed.preview_key,
        "branch": seed.branch,
        "cache_enabled": false,
        "variables": seed.variables,
    });
    crud::insert_run(
        db,
        seed.run_id,
        &format!("preview of {} on {}", seed.target_ref, seed.branch),
        None,
        agentic_automation::SOURCE_TYPE,
        Some(metadata),
        seed.workspace_id,
    )
    .await?;
    let spec = TaskSpec::Automation {
        workflow_ref: scoped,
        variables: seed.variables,
        retry_from_run_id: None,
        cache_enabled: false,
        body: None,
        initial_render_context: None,
    };
    crud::enqueue_task(
        db,
        seed.run_id,
        seed.run_id,
        None,
        &spec,
        None,
        crud::TaskScope::Global,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seed refuses a target the public validator would, before writing
    /// anything — `Disconnected` would fail any write.
    #[tokio::test]
    async fn the_seed_validates_the_unscoped_target() {
        let db = sea_orm::DatabaseConnection::default();
        for bad in [
            "",
            "../x.procedure.yml",
            "/abs.procedure.yml",
            "preview:r:x.yml",
        ] {
            let err = seed_preview_automation_run(
                &db,
                PreviewAutomationSeed {
                    run_id: "r",
                    target_ref: bad,
                    variables: None,
                    workspace_id: Uuid::nil(),
                    preview_key: "k",
                    branch: "b",
                },
            )
            .await
            .expect_err(bad);
            assert!(
                matches!(err, AutomationRunError::InvalidInput(_)),
                "{bad}: {err}"
            );
        }
    }
}
