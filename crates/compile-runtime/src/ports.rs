//! Driven ports: what the compile runtime calls and something above it
//! implements (`internal-docs/backend-architecture.md`, "Crate tiers and
//! depth"). Each is wired where the compile worker is built
//! (`oxy-workspace-runtime`'s `compile_dispatcher`), so a worker cannot be
//! built without its adapter.

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use oxy_compile::RevisionKind;

use crate::worker::CompileSpec;

/// After a `Ready` staging compile: queue that revision's Airway change check
/// when staff preview the branch at that commit. The previews' rows own the
/// check (`oxy_previews::queue::analyze::enqueue_after_staging_compile`); this
/// crate sits beside them and may not depend on them, so the worker calls this
/// port instead. One production adapter, `oxy-workspace-runtime`'s
/// `OxyPreviewChangeCheck`. Best-effort: it never fails the compile.
#[async_trait]
pub trait PreviewChangeCheck: Send + Sync {
    async fn after_staging_compile(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
        branch: Option<&str>,
        git_sha: Option<&str>,
        revision_id: Uuid,
    );
}

/// After a `Ready` compile: when it is a staging revision, ask `check` to
/// queue that revision's change check. Best-effort; never fails the compile.
pub(crate) async fn queue_preview_check(
    check: &dyn PreviewChangeCheck,
    db: &DatabaseConnection,
    spec: &CompileSpec,
    revision_id: Uuid,
) {
    if !matches!(spec.kind, RevisionKind::Staging) {
        return;
    }
    check
        .after_staging_compile(
            db,
            spec.workspace_id,
            spec.branch.as_deref(),
            spec.git_sha.as_deref(),
            revision_id,
        )
        .await;
}
