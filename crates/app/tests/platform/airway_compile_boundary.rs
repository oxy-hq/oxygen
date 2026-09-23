//! Regression test for the airway run path on the compile boundary.
//!
//! `execute_airway` / `reset_airway_schema` claim a queued `TaskSpec::Airway`
//! on the **durable worker fleet**, which is stateless and owns no working
//! copy. Resolving the pipeline's `.airway.yml` by walking the workspace
//! filesystem there is the instance-affinity failure mode: the read fails with
//! "workspace directory not found" / a spurious missing-pipeline error on a
//! replica, while it works on the node that happens to hold the checkout.
//!
//! `.airway.yml` IS compiled — one `airway_pipelines` row per file, keyed by
//! `revision_id`. These tests drive the real production path end-to-end:
//!
//!   `pipeline_ref::load_pipeline_yaml`  (containment guard, agentic-pipeline)
//!     → `WorkspaceContext::resolve_pipeline_yaml`   (port)
//!       → `OxyProjectContext`                        (host adapter, oxy-app)
//!         → `ConfigManager::pipeline_definition`      (Origin decides the source)
//!
//! with the workspace directory **absent from disk entirely**, which is what
//! makes them fail before the change and pass after.
//!
//! Database-backed through [`common::fresh_db`] — own database per test, so
//! this module belongs to `db-per-test` in `.config/nextest.toml`.

use std::path::PathBuf;

use entity::workspaces::WorkspaceStatus;
use entity::{airway_pipelines, organizations, revisions, workspace_compiled_configs, workspaces};
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy_app::agentic_wiring::OxyProjectContext;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::json;
use uuid::Uuid;

/// Per-test database, migrated and wired so that `establish_connection()`
/// (used inside `compiled_reader`) points at it.
///
/// [`common::fresh_db`] rather than a hand-rolled harness, exactly as
/// `toast_webhook_compile_boundary` does: it names the database with the
/// `oxytest_` prefix and the `NEXTEST_RUN_ID` tag that `drop_stale_databases`
/// needs in order to tell a live sibling from a stray, and it asserts
/// process-per-test before the `set_var` below — which is only sound under that
/// isolation. The chain also runs `Migrator::up` once per `cargo nextest run`
/// into a template this clones, instead of once per test.
async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup before any other env access. nextest
    // isolates each test in its own process, so pointing the process-wide
    // OnceCell at the per-test DB here is safe.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    db
}

/// Seed an org + a **promoted** workspace (a `revisions` row set as
/// `current_revision_id`) with no pipelines yet. Returns `(workspace_id,
/// revision_id)`.
async fn seed_promoted_workspace(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let now = chrono::Utc::now().fixed_offset();

    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("acb-org".into()),
        slug: ActiveValue::Set(format!("acb-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");

    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set("acb-ws".into()),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(None),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");

    let rev_id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        workspace_id: ActiveValue::Set(ws_id),
        git_sha: ActiveValue::Set("deadbeef".into()),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("full".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");

    // Promote (the FK requires the revision to exist first).
    let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(ws_id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists")
        .into();
    // A promoted revision always carries a compiled config in production —
    // `oxy-compile` writes both in the same pass. Without it `load_config`
    // downgrades the manager's `Origin` to `Disk` (its documented behaviour for
    // a revision with no readable config) and the boundary is never queried, so
    // a fixture that promotes without one is not modelling a promoted
    // workspace.
    workspace_compiled_configs::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        databases: ActiveValue::Set(json!([])),
        models: ActiveValue::Set(Some(json!([]))),
        integrations: ActiveValue::Set(None),
        repositories: ActiveValue::Set(None),
        builder_agent: ActiveValue::Set(None),
        mcp: ActiveValue::Set(None),
        other: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed compiled config");

    ws.current_revision_id = ActiveValue::Set(Some(rev_id));
    ws.update(db).await.expect("promote revision");

    (ws_id, rev_id)
}

/// Insert one compiled `.airway.yml` row. `name` is the YAML `name:` field and
/// deliberately differs from `file_path` — the mismatch that makes the choice
/// of lookup column observable.
async fn seed_pipeline(db: &DatabaseConnection, rev_id: Uuid, name: &str, file_path: &str) {
    airway_pipelines::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        name: ActiveValue::Set(name.into()),
        file_path: ActiveValue::Set(file_path.into()),
        // Shape mirrors what `oxy-compile` writes: the whole parsed YAML,
        // untyped. It must satisfy the strict (`deny_unknown_fields`)
        // `AirwayPipelineSpec` on the way back out — that round-trip is the
        // point of the test, so the fixture is a realistic authored document
        // (a `destination:` *reference*, which is what users write).
        definition: ActiveValue::Set(json!({
            "name": name,
            "source": {
                "kind": "filesystem",
                "config": {
                    "base_path": "/tmp/acb",
                    "pattern": "*.jsonl",
                    "format": "jsonl",
                    "table_name": "orders",
                },
            },
            "destination": { "database": "warehouse", "dataset_name": "raw" },
            "resources": ["orders"],
        })),
    }
    .insert(db)
    .await
    .expect("seed airway pipeline");
}

/// An `OxyProjectContext` whose workspace path points at a directory that does
/// NOT exist — a stateless durable worker that never cloned the repo.
async fn worker_context_without_working_copy(
    db: &DatabaseConnection,
    ws_id: Uuid,
) -> (OxyProjectContext, PathBuf) {
    // Make this process look like a worker replica, so the host adapter takes
    // the "no working copy → no branch hint" arm.
    // SAFETY: single-threaded test setup; nextest gives each test its own process.
    unsafe {
        std::env::set_var("OXY_ROLE", "worker");
    }
    oxy_app::server::role_manifest::init_process_role_from_env();

    let absent = PathBuf::from(format!("/nonexistent-oxy-workspace/{}", Uuid::new_v4()));
    assert!(!absent.exists(), "precondition: no working copy on disk");

    // Pinned to whatever revision the workspace has promoted, which is what
    // `workspace_middleware` does per request — `ConfigManager` reads its
    // `Origin`, it does not go looking for a revision of its own. `None` here
    // yields `Origin::Disk`, and the absent root below then reports the miss.
    let revision_id = current_revision_id(db, ws_id).await;

    // A root that does NOT exist: the manager carries the capability, and
    // `disk()` turns the absent directory into a retryable error rather than
    // an empty answer. Same shape a durable worker sees.
    let wm = WorkspaceBuilder::new(ws_id)
        .with_working_copy(&absent, revision_id, oxy::config::OnMissing::Empty)
        .await
        .expect("workspace builder")
        .build()
        .await
        .expect("workspace manager");
    (OxyProjectContext::new(wm), absent)
}

/// The workspace's promoted revision, read the way the middleware reads it.
///
/// Takes the per-test connection rather than calling `establish_connection`:
/// this module is on `common::fresh_db`, and `shared_db_registry` counts a
/// literal `establish_connection` here as a claim on the SHARED database — it
/// would then race the serial-db packages on `public`.
async fn current_revision_id(db: &DatabaseConnection, ws_id: Uuid) -> Option<Uuid> {
    workspaces::Entity::find_by_id(ws_id)
        .one(db)
        .await
        .expect("load workspace")
        .and_then(|w| w.current_revision_id)
}

/// The production reader, reached the way a request reaches it: a manager
/// pinned to whatever revision this workspace has promoted.
///
/// `compiled_reader::resolve_pipeline` is gone — `ConfigManager` owns the
/// compiled-vs-disk choice now, so a test that called the raw query would be
/// asserting about a path no request takes. `None` here means the boundary had
/// no row AND there was no working copy to fall through to, which is the same
/// clean miss the old reader reported.
async fn resolve_pipeline_definition(
    db: &DatabaseConnection,
    ws_id: Uuid,
    file_path: &str,
) -> Option<serde_json::Value> {
    let (ctx, _absent) = worker_context_without_working_copy(db, ws_id).await;
    ctx.workspace_manager()
        .config_manager
        .pipeline_definition(file_path)
        .await
        .unwrap_or(None)
}

/// The `name:` the fixture wrote into the definition, which is how a resolved
/// row identifies itself now that the reader returns the document rather than
/// a row wrapper.
fn definition_name(definition: &serde_json::Value) -> &str {
    definition
        .get("name")
        .and_then(|v| v.as_str())
        .expect("the fixture writes a name")
}

/// THE regression: the run path resolves a pipeline with no workspace
/// directory anywhere on disk. Before the compile-boundary port this could
/// only fail — `resolve_pipeline_ref` canonicalises the workspace root first,
/// so an absent working copy is an immediate "workspace root is not
/// accessible".
#[tokio::test]
async fn airway_pipeline_yaml_resolves_with_no_workspace_directory() {
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_promoted_workspace(&db).await;
    seed_pipeline(&db, rev_id, "toast_orders", "pipelines/toast.airway.yml").await;

    let (ctx, absent_root) = worker_context_without_working_copy(&db, ws_id).await;

    // The FS path is genuinely impossible here — pin that, so a future change
    // that quietly re-creates a working copy can't make this test vacuous.
    assert!(
        agentic_pipeline::pipeline_ref::resolve_pipeline_ref(
            &absent_root,
            "pipelines/toast.airway.yml"
        )
        .is_err(),
        "the filesystem path must be unavailable for this test to mean anything"
    );

    // Full production path: guard → port → host adapter → compiled_reader.
    let yaml =
        agentic_pipeline::pipeline_ref::load_pipeline_yaml(&ctx, "pipelines/toast.airway.yml")
            .await
            .expect("compiled row must satisfy the read with no working copy");

    // And the body the worker gets is one the airway parser accepts — the
    // JSONB → YAML round-trip has to survive its actual consumer, not just be
    // a non-empty string.
    let spec = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(&yaml, None)
        .expect("compiled definition must round-trip into an AirwayPipelineSpec");
    assert_eq!(spec.name, "toast_orders");
    assert_eq!(spec.source.kind, "filesystem");

    // Variables still render at run time: the worker re-renders the same
    // document with the run's `variables`, so templating is not collapsed by
    // serving from the boundary.
    let rendered = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(
        &yaml,
        Some(&json!({ "unused": "x" })),
    )
    .expect("rendering with variables must still work on a compiled body");
    assert_eq!(rendered.name, "toast_orders");
}

/// The containment guard still holds on the compiled path: a traversal ref is
/// rejected before either backend is consulted, and the error quotes only the
/// caller-supplied ref.
#[tokio::test]
async fn airway_pipeline_ref_containment_holds_without_a_working_copy() {
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_promoted_workspace(&db).await;
    seed_pipeline(&db, rev_id, "toast_orders", "pipelines/toast.airway.yml").await;

    let (ctx, _absent) = worker_context_without_working_copy(&db, ws_id).await;

    for bad in ["", "   ", "/etc/passwd", "../../etc/passwd", "a/../../b"] {
        let err = agentic_pipeline::pipeline_ref::load_pipeline_yaml(&ctx, bad)
            .await
            .expect_err("traversal / empty refs must be rejected");
        assert!(
            !err.to_string().contains("nonexistent-oxy-workspace"),
            "errors must quote only the ref, never a resolved path: {err}"
        );
    }
}

/// `pipeline_ref` is a workspace-relative PATH, so the reader must key on
/// `file_path` — not the `(revision_id, name)` primary key, whose `name` is the
/// YAML `name:` field. Keying by `name` misses for every pipeline whose name
/// differs from its path, silently falling back to a filesystem a stateless
/// replica doesn't have.
#[tokio::test]
async fn airway_compiled_reader_keys_by_file_path_not_name() {
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_promoted_workspace(&db).await;
    seed_pipeline(&db, rev_id, "toast_orders", "pipelines/toast.airway.yml").await;

    let definition = resolve_pipeline_definition(&db, ws_id, "pipelines/toast.airway.yml")
        .await
        .expect("must resolve by workspace-relative file_path");
    assert_eq!(definition_name(&definition), "toast_orders");

    assert!(
        resolve_pipeline_definition(&db, ws_id, "toast_orders")
            .await
            .is_none(),
        "the YAML name must not resolve a row — the reader keys by file_path"
    );

    // An uncompiled / unknown path is a clean miss so the caller can fall
    // back, not an error.
    assert!(
        resolve_pipeline_definition(&db, ws_id, "pipelines/missing.airway.yml")
            .await
            .is_none()
    );
}

/// Multi-tenant containment on the DB path: a `pipeline_ref` naming another
/// workspace's pipeline must not resolve. The row set is scoped by the
/// caller's own promoted `revision_id`, which belongs to exactly one workspace.
#[tokio::test]
async fn airway_pipeline_ref_cannot_reach_another_workspace() {
    let db = setup_db().await;
    let (ws_a, rev_a) = seed_promoted_workspace(&db).await;
    let (ws_b, rev_b) = seed_promoted_workspace(&db).await;
    seed_pipeline(&db, rev_a, "a_pipeline", "pipelines/a.airway.yml").await;
    seed_pipeline(&db, rev_b, "b_pipeline", "pipelines/secret_b.airway.yml").await;

    assert!(
        resolve_pipeline_definition(&db, ws_a, "pipelines/secret_b.airway.yml")
            .await
            .is_none(),
        "workspace A must not resolve workspace B's pipeline"
    );
    let own = resolve_pipeline_definition(&db, ws_b, "pipelines/secret_b.airway.yml")
        .await
        .expect("B resolves its own pipeline");
    assert_eq!(definition_name(&own), "b_pipeline");
}

/// A workspace with no promoted revision is a clean `None` — the caller falls
/// through to the filesystem, exactly as before. This is `open_compiled_revision`'s
/// contract; pinned here so the airway reader can't drift from it.
#[tokio::test]
async fn airway_unpromoted_workspace_falls_through_to_fs() {
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_promoted_workspace(&db).await;
    seed_pipeline(&db, rev_id, "toast_orders", "pipelines/toast.airway.yml").await;

    // Un-promote.
    let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(ws_id)
        .one(&db)
        .await
        .expect("load workspace")
        .expect("workspace exists")
        .into();
    ws.current_revision_id = ActiveValue::Set(None);
    ws.update(&db).await.expect("un-promote");

    assert!(
        resolve_pipeline_definition(&db, ws_id, "pipelines/toast.airway.yml")
            .await
            .is_none(),
        "an unpromoted workspace must read the FS, not a stale revision"
    );
}

/// Seed an org + a workspace row pointing at `root`, then run the REAL
/// compiler over it and promote the result.
///
/// Every test above seeds `airway_pipelines` by hand, so all of them agree on
/// a `file_path` spelling that nothing in the compiler was ever asked to
/// produce. The writer's key and the reader's key are only ever compared here.
async fn compile_and_promote(db: &DatabaseConnection, root: &std::path::Path) -> Uuid {
    let ws_id = seed_workspace_at(db, root).await;
    compile_at(db, ws_id, root, None).await;
    ws_id
}

/// Seed an org + a workspace row whose `path` is `root`. Split from
/// [`compile_and_promote`] so a test can compile the same workspace more than
/// once, which is what production does and what a from-scratch fixture cannot.
async fn seed_workspace_at(db: &DatabaseConnection, root: &std::path::Path) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("rt-org".into()),
        slug: ActiveValue::Set(format!("rt-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");

    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set("rt-ws".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        path: ActiveValue::Set(Some(root.to_string_lossy().into_owned())),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ws_id
}

/// Run the real compiler over `root` and promote the result.
///
/// `git_sha` matters more than it looks. `compile_after_content_change` — the
/// production trigger — resolves HEAD and passes `Some(sha)`, which is what
/// arms `compile_workspace`'s idempotency short-circuit; `None` mints a unique
/// `local-<uuid>` and opts out of it entirely. A fixture that always passes
/// `None` therefore never exercises the path every cloud compile takes.
async fn compile_at(
    db: &DatabaseConnection,
    ws_id: Uuid,
    root: &std::path::Path,
    git_sha: Option<&str>,
) -> Uuid {
    let outcome = oxy_compile::compile_workspace(oxy_compile::CompileRequest {
        db,
        workspace_id: ws_id,
        workspace_path: root,
        git_sha: git_sha.map(str::to_string),
        branch: Some("main".to_string()),
        compiler_version: oxy_compile::compiler_version(),
        promote: true,
        kind: oxy_compile::RevisionKind::Main,
        owner_user_id: None,
        config_gate: Some(oxy_app::server::compile_config_gate::runtime_config_gate()),
    })
    .await
    .expect("compile the workspace");

    assert!(
        outcome.failures.is_empty(),
        "fixture must compile clean: {:?}",
        outcome.failures
    );
    assert_eq!(
        outcome.promotion,
        oxy_compile::Promotion::Promoted,
        "the worker reads the promoted revision"
    );
    outcome.revision_id
}

/// A working copy with a `config.yml` and one pipeline under `pipelines/`.
///
/// `project_subdir` is the **only** thing that varies between the two cases
/// below, and it is the difference between the two workspace layouts running
/// in production: some workspaces' `workspaces.path` is the clone root, others'
/// is a project directory inside it (`<clone>/oxy`). Returns the tempdir (the
/// guard) and the path the workspace row should carry.
fn working_copy_with_pipeline(project_subdir: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("workspace dir");
    let root = match project_subdir {
        Some(sub) => dir.path().join(sub),
        None => dir.path().to_path_buf(),
    };
    std::fs::create_dir_all(&root).expect("mkdir project root");
    std::fs::write(root.join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    std::fs::create_dir_all(root.join("pipelines")).expect("mkdir pipelines");
    std::fs::write(
        root.join("pipelines/marketing_plan.airway.yml"),
        concat!(
            "name: marketing_plan\n",
            "source:\n",
            "  kind: filesystem\n",
            "  config:\n",
            "    base_path: /tmp/mp\n",
            "    pattern: '*.csv'\n",
            "    format: csv\n",
            "    table_name: marketing_plan\n",
            "destination:\n",
            "  database: warehouse\n",
            "  dataset_name: raw\n",
            "resources:\n",
            "  - marketing_plan\n",
        ),
    )
    .expect("write pipeline");
    (dir, root)
}

/// THE PRODUCTION ROUND TRIP. The compiler's `file_path` and the runtime's
/// `pipeline_ref` must be the same string.
///
/// A queued `TaskSpec::Airway` carries a workspace-relative ref. If the writer
/// stores anything else, the row exists and the worker still cannot find it —
/// which on a node with no working copy is an `Unavailable` deferral that
/// repeats every 30s for as long as the queue ceiling allows, with the run
/// sitting in `running` and nothing ever recorded as an error.
///
/// Asserts the key twice over: once raw against `airway_pipelines` (what the
/// writer stored) and once through the full reader path (what the worker asks
/// for). A failure in the first is a writer bug and in the second a reader
/// bug, and the two are indistinguishable from the deferral alone.
async fn assert_round_trip_for_layout(project_subdir: Option<&str>) {
    let db = setup_db().await;
    let (_guard, root) = working_copy_with_pipeline(project_subdir);
    let ws_id = compile_and_promote(&db, &root).await;

    let rev_id = current_revision_id(&db, ws_id)
        .await
        .expect("the compile promoted a revision");
    let rows = airway_pipelines::Entity::find()
        .filter(airway_pipelines::Column::RevisionId.eq(rev_id))
        .all(&db)
        .await
        .expect("read airway_pipelines");
    assert_eq!(
        rows.iter()
            .map(|r| r.file_path.as_str())
            .collect::<Vec<_>>(),
        vec!["pipelines/marketing_plan.airway.yml"],
        "the writer must key on the path relative to the PROJECT root, whatever \
         the layout — got {:?} for subdir {project_subdir:?}",
        rows.iter().map(|r| &r.file_path).collect::<Vec<_>>()
    );

    let (ctx, _absent) = worker_context_without_working_copy(&db, ws_id).await;
    let yaml = agentic_pipeline::pipeline_ref::load_pipeline_yaml(
        &ctx,
        "pipelines/marketing_plan.airway.yml",
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "a compiled pipeline must resolve with no working copy (subdir {project_subdir:?}): {e}"
        )
    });
    let spec = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(&yaml, None)
        .expect("the compiled body must round-trip into an AirwayPipelineSpec");
    assert_eq!(spec.name, "marketing_plan");
}

/// The layout where `workspaces.path` IS the clone root.
#[tokio::test]
async fn a_compiled_pipeline_resolves_by_the_ref_a_queued_task_carries() {
    assert_round_trip_for_layout(None).await;
}

/// The layout where `workspaces.path` is a project directory inside the clone.
///
/// Both shapes are in production side by side, and a key composed against the
/// wrong base would round-trip in one and not the other — which reads exactly
/// like the incident: one workspace's pipelines resolve on the fleet and
/// another's do not, with no deploy in between. Pinned as its own case so a
/// regression names the layout instead of just failing.
#[tokio::test]
async fn the_ref_is_relative_to_the_project_root_not_the_clone_root() {
    assert_round_trip_for_layout(Some("oxy")).await;
}

/// A pipeline ADDED to a workspace that has already compiled must be
/// registered by the compile that first sees it.
///
/// The two cases above compile a fresh workspace, once, that already contains
/// the pipeline — so every entity in them is a first compile with no prior
/// revision to differ from. Production is the other shape: the workspace had
/// compiled many times, and the pipeline arrived on a pull and was compiled for
/// the first time in the revision the worker then could not resolve it from.
/// A from-scratch fixture cannot see a fault that needs a predecessor.
///
/// Both compiles carry a real `git_sha`, because that is what arms
/// `compile_workspace`'s idempotency short-circuit — with `None` the compiler
/// mints `local-<uuid>` and skips it, so a fixture that never passes one never
/// exercises the path every cloud compile takes. Distinct shas, as two commits
/// would have: the partial unique index on `(workspace_id, git_sha)` for
/// ready+main revisions is what makes that the realistic shape.
#[tokio::test]
async fn a_pipeline_added_to_an_existing_workspace_registers_on_the_next_compile() {
    let db = setup_db().await;

    // First: a workspace with a config and NO pipelines, compiled and promoted.
    let dir = tempfile::tempdir().expect("workspace dir");
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    let ws_id = seed_workspace_at(&db, &root).await;
    let first = compile_at(
        &db,
        ws_id,
        &root,
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    )
    .await;
    assert!(
        airway_pipelines::Entity::find()
            .filter(airway_pipelines::Column::RevisionId.eq(first))
            .all(&db)
            .await
            .expect("read airway_pipelines")
            .is_empty(),
        "precondition: the first revision has no pipelines, so the second is \
         genuinely the first compile that sees one"
    );

    // Then: the pipeline arrives, as a pull would deliver it, and is compiled.
    std::fs::create_dir_all(root.join("pipelines")).expect("mkdir pipelines");
    std::fs::write(
        root.join("pipelines/marketing_plan.airway.yml"),
        concat!(
            "name: marketing_plan\n",
            "source:\n",
            "  kind: filesystem\n",
            "  config:\n",
            "    base_path: /tmp/mp\n",
            "    pattern: '*.csv'\n",
            "    format: csv\n",
            "    table_name: marketing_plan\n",
            "destination:\n",
            "  database: warehouse\n",
            "  dataset_name: raw\n",
            "resources:\n",
            "  - marketing_plan\n",
        ),
    )
    .expect("write pipeline");
    let second = compile_at(
        &db,
        ws_id,
        &root,
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_ne!(
        first, second,
        "the second compile must mint its own revision"
    );
    assert_eq!(
        current_revision_id(&db, ws_id).await,
        Some(second),
        "the second revision must be the promoted one"
    );

    let rows = airway_pipelines::Entity::find()
        .filter(airway_pipelines::Column::RevisionId.eq(second))
        .all(&db)
        .await
        .expect("read airway_pipelines");
    assert_eq!(
        rows.iter()
            .map(|r| r.file_path.as_str())
            .collect::<Vec<_>>(),
        vec!["pipelines/marketing_plan.airway.yml"],
        "a newly added pipeline must be registered by the compile that first \
         walks it, not only by a later one"
    );

    // And the worker can actually resolve it from that revision.
    let (ctx, _absent) = worker_context_without_working_copy(&db, ws_id).await;
    let yaml = agentic_pipeline::pipeline_ref::load_pipeline_yaml(
        &ctx,
        "pipelines/marketing_plan.airway.yml",
    )
    .await
    .expect("a newly added pipeline must resolve on a node with no working copy");
    assert_eq!(
        agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(&yaml, None)
            .expect("round-trips into an AirwayPipelineSpec")
            .name,
        "marketing_plan"
    );
}
