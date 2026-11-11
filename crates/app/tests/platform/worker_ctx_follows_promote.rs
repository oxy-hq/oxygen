//! The background driver's workspace context follows a promote.
//!
//! BMG prod, 2026-11-11: a revision adding `pipelines/pnl_seller_costs.airway.yml`
//! was promoted, and a Run-now a minute later was claimed sixteen times over
//! ~9 minutes by one worker, every claim failing `NotInRevision` naming the
//! PREVIOUS revision, until the 600s ceiling failed the run. A rerun on another
//! worker finished in 2s. Two layers pinned the old revision:
//!
//! 1. the driver's context cache was keyed on `workspace_id` alone with a 600s
//!    TTL, so a tick after the promote got the context built before it;
//! 2. a drive builds one executor (one context) for the whole run and re-claims
//!    deferred tasks on it, so even a fresh cache could not reach a run already
//!    in flight.
//!
//! Each case compiles and promotes a real R1 without the pipeline, then a real
//! R2 with it, through the production compiler.
//!
//! Database-backed through [`common::fresh_db`], so it belongs to `db-per-test`.

use std::path::Path;
use std::sync::Arc;

use agentic_pipeline::pipeline_ref::{PipelineRefError, load_pipeline_yaml};
use agentic_pipeline::platform::ProjectContext;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::api::middlewares::workspace_context::PreaggCacheCtx;
use oxy_app::server::router::{
    WorkspaceContextCache, new_workspace_context_cache, worker_project_ctx,
};
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::airway_compile_boundary::{compile_at, seed_workspace_at};

const PIPELINE: &str = "pipelines/pnl_seller_costs.airway.yml";

const PIPELINE_YAML: &str = concat!(
    "name: pnl_seller_costs\n",
    "source:\n",
    "  kind: filesystem\n",
    "  config:\n",
    "    base_path: /tmp/pnl\n",
    "    pattern: '*.csv'\n",
    "    format: csv\n",
    "    table_name: seller_costs\n",
    "destination:\n",
    "  database: warehouse\n",
    "  dataset_name: raw\n",
    "resources:\n",
    "  - seller_costs\n",
);

async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup before any other env access; nextest
    // gives each test its own process.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    db
}

/// A workspace whose promoted revision R1 has a config and no pipeline.
async fn promoted_without_pipeline(db: &DatabaseConnection) -> (tempfile::TempDir, Uuid, Uuid) {
    let dir = tempfile::tempdir().expect("workspace dir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    let ws_id = seed_workspace_at(db, dir.path()).await;
    let r1 = compile_at(db, ws_id, dir.path(), Some("r1-sha")).await;
    (dir, ws_id, r1)
}

/// Commit the pipeline and compile + promote R2.
async fn promote_with_pipeline(
    db: &DatabaseConnection,
    ws_id: Uuid,
    root: &Path,
    r1: Uuid,
) -> Uuid {
    std::fs::create_dir_all(root.join("pipelines")).expect("mkdir pipelines");
    std::fs::write(root.join(PIPELINE), PIPELINE_YAML).expect("write pipeline");
    let r2 = compile_at(db, ws_id, root, Some("r2-sha")).await;
    assert_ne!(
        r2, r1,
        "precondition: the second compile promoted a new revision"
    );
    r2
}

/// The workspace context, obtained exactly as the driver's tick obtains it: a
/// fresh read of the row, then the cache tagged with its promoted revision.
async fn tick_context(
    db: &DatabaseConnection,
    cache: &Arc<WorkspaceContextCache>,
    ws_id: Uuid,
) -> Arc<OxyProjectContext> {
    let row = entity::workspaces::Entity::find_by_id(ws_id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists");
    let path = row.path.clone().expect("workspace has a path");
    worker_project_ctx(
        cache,
        ws_id,
        &path,
        row.current_revision_id,
        db,
        &PreaggCacheCtx::default(),
    )
    .await
    .expect("a promoted workspace builds a context")
}

fn pinned(ctx: &OxyProjectContext) -> Option<Uuid> {
    ctx.workspace_manager().config_manager.revision_id()
}

/// Layer 1: the next tick after a promote must see the new revision, not a
/// context cached up to ten minutes before it.
#[tokio::test]
async fn a_tick_after_a_promote_gets_a_context_at_the_new_revision() {
    let db = setup_db().await;
    let (dir, ws_id, r1) = promoted_without_pipeline(&db).await;
    let cache = new_workspace_context_cache();

    let before = tick_context(&db, &cache, ws_id).await;
    assert_eq!(pinned(&before), Some(r1));
    let err = load_pipeline_yaml(before.as_ref(), PIPELINE)
        .await
        .expect_err("precondition: R1 does not serve the pipeline");
    assert!(matches!(err, PipelineRefError::NotInRevision(_)), "{err:?}");

    let r2 = promote_with_pipeline(&db, ws_id, dir.path(), r1).await;

    let after = tick_context(&db, &cache, ws_id).await;
    let yaml = load_pipeline_yaml(after.as_ref(), PIPELINE)
        .await
        .unwrap_or_else(|e| {
            panic!("the tick after promoting R2 ({r2}) must resolve the pipeline: {e}")
        });
    assert!(yaml.contains("pnl_seller_costs"), "{yaml}");
    assert_eq!(pinned(&after), Some(r2));

    // And it is cached at R2: the following tick reuses it rather than
    // rebuilding every 300ms.
    let again = tick_context(&db, &cache, ws_id).await;
    assert!(
        Arc::ptr_eq(&after, &again),
        "an unchanged revision is a cache hit"
    );
}

/// Layer 2: a drive that built its executor at R1 and still holds that
/// context — no cache lookup between claims — must be able to re-resolve to R2.
#[tokio::test]
async fn a_held_context_re_resolves_to_the_promoted_revision() {
    let db = setup_db().await;
    let (dir, ws_id, r1) = promoted_without_pipeline(&db).await;
    let cache = new_workspace_context_cache();

    let held = tick_context(&db, &cache, ws_id).await;
    assert!(
        held.at_current_promoted_revision().await.is_none(),
        "nothing to re-resolve while R1 is still current"
    );

    let r2 = promote_with_pipeline(&db, ws_id, dir.path(), r1).await;

    // What the drive still holds is pinned to R1, as in the incident.
    let err = load_pipeline_yaml(held.as_ref(), PIPELINE)
        .await
        .expect_err("the held context is pinned to R1");
    assert!(matches!(err, PipelineRefError::NotInRevision(_)), "{err:?}");

    let fresh = held
        .at_current_promoted_revision()
        .await
        .expect("a held driver context must re-resolve after a promote");
    load_pipeline_yaml(fresh.as_ref(), PIPELINE)
        .await
        .unwrap_or_else(|e| panic!("the re-resolved context must serve R2 ({r2}): {e}"));
    assert_eq!(fresh.compiled_revision(), Some(r2));
    assert!(
        fresh.at_current_promoted_revision().await.is_none(),
        "the fresh context is at the current revision, so a caller retries at most once"
    );

    // The re-resolve refreshed the cache entry, so the next tick reuses it.
    let next_tick = tick_context(&db, &cache, ws_id).await;
    assert_eq!(pinned(&next_tick), Some(r2));
    let fresh_as_platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = next_tick.clone();
    assert!(
        Arc::ptr_eq(&fresh, &fresh_as_platform),
        "the rebuilt context is the cache entry the next tick reuses"
    );
}

/// A context not built by the driver — an HTTP request's — never re-resolves:
/// a request sees one revision start to finish.
#[tokio::test]
async fn a_request_context_does_not_re_resolve() {
    let db = setup_db().await;
    let (dir, ws_id, r1) = promoted_without_pipeline(&db).await;
    let manager = WorkspaceBuilder::new(ws_id)
        .with_working_copy(dir.path(), Some(r1), oxy::config::OnMissing::Empty)
        .await
        .expect("workspace builder")
        .build()
        .await
        .expect("workspace manager");
    let request_ctx = OxyProjectContext::new(manager);
    promote_with_pipeline(&db, ws_id, dir.path(), r1).await;
    assert!(request_ctx.at_current_promoted_revision().await.is_none());
}
