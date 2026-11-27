//! The background driver's workspace context: what the run drivers
//! (`oxy-server`'s `drivers::recovery` and `drivers::periodic`), the workspace-health
//! and reconciliation sweeps and a queued run's requester build an
//! `OxyProjectContext` from when no request is there to build it. Down from
//! `oxy-app`'s `server/router/recovery.rs` (decompose W6b, 2026-11-23), which
//! re-exports what it and the prelude call; [`worker_ctx`] is the driver's
//! cached, revision-tagged copy.

use std::sync::Arc;

use oxy::adapters::secrets::SecretsManager;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::WorkingCopy;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
use sea_orm::{DatabaseConnection, EntityTrait};

use crate::project_ctx::OxyProjectContext;

pub mod worker_ctx;

/// The global driver's default tick interval, in seconds: how often
/// `oxy-server`'s `drivers::recovery` loop runs when
/// `OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS` is unset or invalid. Down from
/// `recovery.rs` (decompose R0, 2026-11-23), which re-exports it, so a surface
/// that reasons about the driver's cadence (the experiment status's staleness
/// window) need not depend on `oxy-app`.
pub const DEFAULT_INPROC_GLOBAL_WORKER_INTERVAL_SECS: u64 = 30;

pub async fn build_local_project_ctx(
    cwd: &std::path::Path,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<Arc<OxyProjectContext>> {
    let mut builder = match WorkspaceBuilder::new(LOCAL_WORKSPACE_ID)
        .with_working_copy(cwd, None, oxy::config::OnMissing::Empty)
        .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                cwd = %cwd.display(),
                error = %e,
                "local recovery: failed to resolve workspace path"
            );
            return None;
        }
    };
    // Attach the same DB-first (env-fallback) secrets manager the request
    // middleware builds — otherwise a resumed run can't resolve workspace
    // secrets stored in the DB (e.g. a ClickHouse `password_var`, a Toast
    // `TOAST_CLIENT_SECRET`) and only sees process env vars. This is the
    // difference between the resume path and a direct run.
    builder = with_db_secrets_manager(builder, LOCAL_WORKSPACE_ID);
    let wm = match builder.build().await {
        Ok(wm) => wm,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                cwd = %cwd.display(),
                error = %e,
                "local recovery: failed to build WorkspaceManager"
            );
            return None;
        }
    };
    // Wire the DB handle exactly like the HTTP workspace middleware does.
    // The in-process global driver executes `TaskSpec::Compile` through this
    // context, and `compile_dispatcher()` (like the anomaly tools) is `None`
    // unless `db` is set — without it every compile fails with
    // "compile_dispatcher() returned None". See OxyProjectContext::with_db.
    Some(Arc::new(
        OxyProjectContext::new(wm)
            .with_db(Arc::new(db.clone()))
            .with_preagg(preagg),
    ))
}

/// Attach the DB-backed (env-fallback) secrets manager to a workspace
/// builder, mirroring the request middleware. Best-effort: a failure logs
/// and leaves the builder's default manager, matching middleware behavior.
fn with_db_secrets_manager(
    builder: WorkspaceBuilder<WorkingCopy>,
    workspace_id: uuid::Uuid,
) -> WorkspaceBuilder<WorkingCopy> {
    match SecretsManager::from_database_with_env_fallback(SecretManagerService::new(workspace_id)) {
        Ok(secrets_manager) => builder.with_secrets_manager(secrets_manager),
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                %workspace_id,
                error = %e,
                "recovery: failed to create DB secrets manager; resumed run may not resolve DB secrets"
            );
            builder
        }
    }
}

/// Build a workspace-scoped `OxyProjectContext` for a background sweep given
/// only a `workspace_id` (resolving its path from the `workspaces` table).
/// Returns `None` when the workspace has no path (e.g. a local-mode sentinel) —
/// the caller degrades gracefully. Cloud path only; reuses
/// [`build_cloud_project_ctx_inner`]. Not cached here — callers that touch many
/// checks should build once per workspace and reuse.
pub async fn build_workspace_ctx(
    workspace_id: uuid::Uuid,
    db: &DatabaseConnection,
) -> Option<Arc<OxyProjectContext>> {
    // No preagg context: this builder serves the workspace-health smoke probes
    // and the reconciliation check, both of which exist to test the WAREHOUSE.
    // Reconciliation compares an Oxy measure against a live external source, so
    // answering the Oxy side from a rollup would report drift that is really
    // rollup lag — the check would be measuring its own cache.
    build_compiled_scan_ctx(workspace_id, db, &PreaggCacheCtx::default()).await
}

/// [`build_workspace_ctx`] for a queued eval run (`oxy-api-eval`'s
/// `EvalRunExecutor`): the same promoted revision and compiled semantic scan,
/// plus the node's Layer-1 pre-aggregation cache.
///
/// An eval scores the answer a served chat gives, and a served chat is
/// answered from a rollup whenever one covers the query (the workspace
/// middleware attaches this same cache). Built without it, every query compiles
/// to warehouse SQL, so a case asserting `preagg: true` can never pass — not
/// even with every rollup built. `preagg` is the one the executor was built
/// with (`TaskDeps::preagg`). That is the cache only where the eval drains in
/// `oxy serve`'s in-process driver (a single instance, a `just up` stack); a
/// standalone `oxy worker` has none (`commands::worker`), so an eval drained
/// there reads the warehouse, as every other queued run on that pod does.
///
/// A promoted revision with no compiled semantic model is not fatal here
/// either: the context is still built, and the eval's semantic queries report
/// the model unavailable until a compile lands.
pub async fn build_eval_workspace_ctx(
    workspace_id: uuid::Uuid,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<Arc<OxyProjectContext>> {
    build_compiled_scan_ctx(workspace_id, db, preagg).await
}

/// The body [`build_workspace_ctx`] and [`build_eval_workspace_ctx`] share: the
/// workspace row's path, the driver context over the promoted revision, and a
/// semantic scan root resolved from the compile boundary and nowhere else.
async fn build_compiled_scan_ctx(
    workspace_id: uuid::Uuid,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<Arc<OxyProjectContext>> {
    let path = match entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
    {
        Ok(Some(ws)) => ws.path?,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(target: "recovery", %workspace_id, error = %e,
                "build_compiled_scan_ctx: workspace lookup failed");
            return None;
        }
    };
    let ctx = build_cloud_project_ctx_inner(workspace_id, &path, db, preagg).await?;

    // Resolve the semantic scan root from the PROMOTED REVISION, and only from
    // it. Two failures, one line:
    //
    // 1. The eval runs as a queued task on the global-run fleet, which holds a
    //    workspace's ROW and not its DIRECTORY. Reading `working_copy().root()`
    //    there failed with `failed to load semantic model: failed to read
    //    <workspace path>: No such file or directory` — an alert about the
    //    node, worded as an alert about the workspace, on a workspace whose
    //    every other surface was querying that same model fine.
    // 2. On a node that DOES hold a working copy — a single instance, or the
    //    factory doubling as the global-run node — that copy holds whatever
    //    someone is editing in the IDE right now. Health judges the workspace,
    //    so it must judge what is promoted: an uncommitted `.view.yml` must
    //    never page the on-call about a revision that is fine.
    //
    // Hence `resolve_compiled_scan_source`, not `resolve_query_scan_source` —
    // the serving resolver falls back to the working copy by design, which is
    // right for every reader that is not this one.
    //
    // The scan is resolved ONCE and held for the life of the context: a
    // materialised scan is a tempdir, and every probe in the pass reads it.
    let ctx = ctx.requiring_compiled_semantic_scan();
    let ctx = match crate::scan_source::resolve_compiled_scan_source(ctx.workspace_manager()).await
    {
        Ok(scan) => ctx.with_semantic_scan(scan),
        Err(unavailable) => {
            // Not fatal: the connection and app probes don't need a layer, and
            // the workspace is still evaluated on everything else. A compile is
            // enqueued, and the semantic checks report that the model is not
            // compiled rather than blaming the YAML — or, worse, judging
            // someone's draft.
            tracing::warn!(
                target: "health_eval",
                %workspace_id,
                reason = %unavailable.message(),
                "no compiled semantic model for the health eval or eval run; semantic \
                 checks will report unavailable until a compile lands"
            );
            ctx
        }
    };
    Some(Arc::new(ctx))
}

/// Why [`build_requester_base_ctx`] built nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequesterBaseError {
    /// Nothing is compiled and this node cannot compile: a compile was
    /// queued, and the same build succeeds once it lands.
    NotCompiled,
    /// The workspace manager would not build — a config that will not load
    /// here. Waiting does not change it.
    Build,
}

/// The workspace context a driver builds for a run it drives *as its
/// requester* (`queued_analytics::requester`): the tick's own construction —
/// the promoted revision, the DB-first secrets manager, the database handle —
/// before a subject and a role are put on it. No preagg context: the request
/// this stands in for read through the app's, which a driver does not hold;
/// a query then reads the warehouse instead of a rollup, never a wrong answer.
///
/// Says which of its two refusals it is, because a caller retries one and not
/// the other — as the build itself decided it, never from a second look at
/// state that can change in between.
pub async fn build_requester_base_ctx(
    workspace_id: uuid::Uuid,
    path: &str,
    db: &DatabaseConnection,
) -> Result<OxyProjectContext, RequesterBaseError> {
    build_cloud_project_ctx_result(workspace_id, path, db, &PreaggCacheCtx::default()).await
}

/// The revision a driver context built from `row` would pin now: the same
/// resolve [`build_cloud_project_ctx_inner`] makes, from a row the caller just
/// read. Lives here, beside the build, because this file is where the driver
/// resolves its `Origin` — the one place outside the request middleware allowed
/// to name `compiled_reader` (`crates/guards/tests/integration/compiled_reader_is_not_a_back_door.rs`).
pub(super) async fn driver_revision_for(row: &entity::workspaces::Model) -> Option<uuid::Uuid> {
    oxy_app_core::compiled_reader::resolve_request_revision_for_workspace(row, None).await
}

/// The driver's workspace context, before the `Arc`, so a caller that has more
/// to attach (`build_workspace_ctx` resolves a semantic scan root, the driver's
/// cache a revision rebuilder — `worker_ctx::worker_project_ctx`) can do it
/// without unwrapping one.
pub async fn build_cloud_project_ctx_inner(
    workspace_id: uuid::Uuid,
    path: &str,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<OxyProjectContext> {
    build_cloud_project_ctx_result(workspace_id, path, db, preagg)
        .await
        .ok()
}

/// [`build_cloud_project_ctx_inner`], saying why it built nothing.
async fn build_cloud_project_ctx_result(
    workspace_id: uuid::Uuid,
    path: &str,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Result<OxyProjectContext, RequesterBaseError> {
    // Resolve the config the same way the request middleware does: the promoted
    // compiled revision first, the working copy only on a miss.
    //
    // This is not just a cache — the two sources are not the same config. The
    // compile worker *injects* fields the on-disk `config.yml` never has, most
    // importantly the DuckDB `s3_mirror` block (`oxy-compile::duckdb_mirror`).
    // A context built from the FS therefore builds a `Local` DuckDB connector
    // pointed at a working copy that a stateless replica doesn't have, while
    // every request path — reading the compiled config — builds the S3-mirror
    // connector and succeeds. Background work (run recovery, reconciliation, the
    // health smoke probes) must see the same databases, with the same shape, as
    // the queries it is meant to be checking; otherwise the smoke test reports a
    // dead connection for a warehouse the product is happily querying.
    //
    // Branch is `None` (the promoted revision) to match the workspace root path
    // this context is built against — the same pairing `resolve_smoke_settings`
    // already uses.
    let compiled_config =
        oxy_app_core::compiled_reader::resolve_request_revision(workspace_id, None).await;

    if compiled_config.is_none() && !oxy_app_core::pods::role_manifest::process_can_compile() {
        tracing::warn!(
            target: "recovery",
            %workspace_id,
            "no compiled config and no working copy on this node; enqueuing a compile"
        );
        oxy_compile_runtime::reconcile::enqueue::enqueue_lazy_compile(db, workspace_id).await;
        return Err(RequesterBaseError::NotCompiled);
    }
    assemble_cloud_project_ctx(workspace_id, path, compiled_config, db, preagg).await
}

/// The context a node that has nothing compiled for a workspace drives that
/// workspace's pending **commit compiles** under, and nothing else.
///
/// A commit compile reads no config from the context: it fetches its commit,
/// and the dispatcher resolves everything else from the database. What it does
/// need is a context that carries a database handle (the compile dispatcher is
/// `None` without one), so this is [`build_cloud_project_ctx_inner`] minus the
/// refusal to build one when nothing is compiled. It is never cached and never
/// given other work: the caller drives it under `DrivePolicy::Exactly`, because
/// an agent or a pipeline run against this empty config would fail where it
/// should wait for the compile.
pub async fn build_commit_compile_ctx(
    workspace_id: uuid::Uuid,
    path: &str,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<Arc<OxyProjectContext>> {
    assemble_cloud_project_ctx(workspace_id, path, None, db, preagg)
        .await
        .ok()
        .map(Arc::new)
}

async fn assemble_cloud_project_ctx(
    workspace_id: uuid::Uuid,
    path: &str,
    compiled_config: Option<uuid::Uuid>,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Result<OxyProjectContext, RequesterBaseError> {
    let builder_init = WorkspaceBuilder::new(workspace_id)
        .with_working_copy(path, compiled_config, oxy::config::OnMissing::Empty)
        .await;
    let mut builder = match builder_init {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                %workspace_id,
                error = %e,
                "cloud recovery: failed to resolve workspace path, skipping"
            );
            return Err(RequesterBaseError::Build);
        }
    };
    // Same DB-first secrets manager as the request middleware, so a
    // resumed run resolves this workspace's DB-stored secrets (not just
    // process env). See `build_local_project_ctx`.
    builder = with_db_secrets_manager(builder, workspace_id);
    let wm = match builder.build().await {
        Ok(wm) => wm,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                %workspace_id,
                error = %e,
                "cloud recovery: failed to build WorkspaceManager, skipping"
            );
            return Err(RequesterBaseError::Build);
        }
    };
    // Wire the DB handle exactly like the HTTP workspace middleware does.
    // The in-process global driver executes `TaskSpec::Compile` through this
    // context, and `compile_dispatcher()` (like the anomaly tools) is `None`
    // unless `db` is set — without it every compile fails with
    // "compile_dispatcher() returned None". See OxyProjectContext::with_db.
    Ok(OxyProjectContext::new(wm)
        .with_db(Arc::new(db.clone()))
        .with_preagg(preagg))
}

#[cfg(test)]
mod health_scan_wiring_tests {
    /// Source up to this module's own test attribute — the same shape guard
    /// `oxy_api_metric_monitoring::metric_tree::handlers_never_scan_the_working_copy` uses.
    fn src() -> &'static str {
        include_str!("driver_ctx.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("source splits on its own test-module attribute")
    }

    /// One function's body: from its signature to the first line that closes
    /// a top-level item, so an assertion about one builder cannot be satisfied
    /// by the next one down the file.
    fn body(name: &str) -> &'static str {
        let after = src()
            .split(&format!("async fn {name}("))
            .nth(1)
            .unwrap_or_else(|| panic!("{name} is defined here"));
        after.split("\n}\n").next().expect("the body closes")
    }

    /// `build_workspace_ctx` is the ONLY context builder the workspace-health
    /// eval uses, and `build_eval_workspace_ctx` the one a queued eval run
    /// uses. Both judge a workspace, so they must read the promoted revision
    /// and nothing else: the fleet node they run on may not hold the workspace
    /// directory at all, and a node that does holds whatever is being edited in
    /// the IDE. Both go through `build_compiled_scan_ctx`, which is where that
    /// is decided.
    ///
    /// Guarded as source shape because the regression is an ABSENCE: dropping
    /// either call compiles perfectly and silently restores the working-copy
    /// fallback, which is only wrong in cloud. Poke House Staging paged for 58
    /// days on exactly that. `resolve_query_scan_source` is named here as the
    /// wrong door — it falls back to the working copy by design.
    #[test]
    fn the_health_context_reads_the_promoted_revision_and_nothing_else() {
        let shared = body("build_compiled_scan_ctx");
        assert!(
            shared.contains("resolve_compiled_scan_source")
                && shared.contains("with_semantic_scan"),
            "build_compiled_scan_ctx must hand its context a compile-boundary scan root; \
             without one the health eval parses the semantic model out of a working \
             copy the fleet does not have"
        );
        assert!(
            shared.contains("requiring_compiled_semantic_scan"),
            "and it must REFUSE the working-copy fallback: on a node that does hold the \
             files, falling back judges someone's uncommitted IDE edit and pages the \
             on-call about a promoted revision that is fine"
        );
        assert!(
            !shared.contains("resolve_query_scan_source("),
            "the serving resolver falls back to the working copy by design"
        );
        for builder in ["build_workspace_ctx", "build_eval_workspace_ctx"] {
            assert!(
                body(builder).contains("build_compiled_scan_ctx("),
                "{builder} must build through build_compiled_scan_ctx, which holds the \
                 compiled-scan resolution"
            );
        }
    }

    /// The eval context carries the node's pre-aggregation cache; the health
    /// context does not.
    ///
    /// An eval scores the answer a served chat gives, and a chat is answered
    /// from a rollup when one covers the query. Handing the eval the health
    /// builder's `PreaggCacheCtx::default()` made every query compile to
    /// warehouse SQL, so a case asserting `preagg: true` failed on both
    /// runtimes with all 30 Demo rollups built (parity run, 2026-11-27). The
    /// health context is the reverse: reconciliation compares an Oxy measure
    /// with a live source, so a rollup there reports its own lag as drift.
    #[test]
    fn the_eval_context_reads_rollups_and_the_health_context_does_not() {
        let eval = body("build_eval_workspace_ctx");
        assert!(
            eval.contains("preagg: &PreaggCacheCtx")
                && eval.contains("build_compiled_scan_ctx(workspace_id, db, preagg)"),
            "build_eval_workspace_ctx must pass its caller's preagg cache through; \
             without it an eval can never be answered from a rollup"
        );
        assert!(
            !eval.contains("PreaggCacheCtx::default()"),
            "build_eval_workspace_ctx must not drop the cache it is handed"
        );
        let health = body("build_workspace_ctx");
        assert!(
            health.contains("PreaggCacheCtx::default()") && !health.contains("preagg:"),
            "build_workspace_ctx must stay preagg-free: the health probes and \
             reconciliation exist to test the warehouse, not a rollup of it"
        );
        let shared = body("build_compiled_scan_ctx");
        assert!(
            shared.contains("build_cloud_project_ctx_inner(workspace_id, &path, db, preagg)"),
            "build_compiled_scan_ctx must build with the preagg it is given"
        );
    }
}
