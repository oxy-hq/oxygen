//! Driven ports: what the compile runtime calls and something above it
//! implements (`internal-docs/backend-architecture.md`, "Crate tiers and
//! depth"). Each is wired where the compile worker is built
//! (`oxy-workspace-runtime`'s `compile_dispatcher`), so a worker cannot be
//! built without its adapter.

use async_trait::async_trait;
use sea_orm::{DatabaseConnection, DbErr};
use serde_json::Value;
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

/// The compiled-config reads a post-promote reconcile makes: which revision a
/// workspace serves, and that revision's merged `config.yml` and
/// `reconcile.yml` rows. The reads themselves (the revision resolution and the
/// merge the compile-time gate uses) live in `oxy-app-core`, which this crate
/// may not depend on, so the reconciles in [`crate::worker`] take this port.
///
/// One production adapter, `oxy-workspace-runtime`'s
/// `OxyCompiledConfigReader`, which delegates to `oxy-app-core` and is passed
/// by the compile worker ([`crate::worker::CompileWorker::new`]) and by
/// `oxy-server`'s periodic reconcile driver. It is a parameter, never an
/// `Option` or a process global, so a caller that forgets it fails to compile.
/// Each method has the same contract as the `oxy-app-core` function it is named
/// after.
#[async_trait]
pub trait CompiledConfigReader: Send + Sync {
    /// The one revision a read of `workspace_id` uses, or `None` when there is
    /// no compiled revision to read.
    async fn resolve_request_revision(
        &self,
        workspace_id: Uuid,
        branch_hint: Option<&str>,
    ) -> Option<Uuid>;

    /// The merged compiled `config.yml` of `revision_id`.
    async fn resolve_workspace_config_at(&self, revision_id: Uuid) -> Result<Option<Value>, DbErr>;

    /// The compiled `reconcile.yml` of `revision_id`, if it has one.
    async fn resolve_reconcile_config_at(&self, revision_id: Uuid) -> Result<Option<Value>, DbErr>;

    /// The merged compiled `config.yml` of the revision a read of
    /// `workspace_id` uses.
    async fn resolve_workspace_config(
        &self,
        workspace_id: Uuid,
        branch_hint: Option<&str>,
    ) -> Result<Option<Value>, DbErr>;
}
