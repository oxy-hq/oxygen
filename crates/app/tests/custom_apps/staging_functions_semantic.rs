//! Staging functions read their build's pinned semantic model (#3370), and a
//! rollup built from the promoted model never answers them.
//!
//! The pin is re-scoped inside the host's `ctx.semantic` call
//! (`host.rs::semantic_query`): a host call runs on a task of its own, and
//! `preagg_context` reads the pin from the task to decline rollups. The rollup
//! test is the one that fails without that re-scope — the branch model and
//! the promoted one are identical there, so the rollup's hash is live under
//! both and only the pin makes it stand aside.

use std::sync::Arc;

use axum::Router;
use axum::routing::any;
use entity::{app_builds, apps};
use oxy_app::server::api::custom_apps_functions::seam::FunctionQueryExecutor;
use oxy_app::server::api::custom_apps_serve;
use oxy_app::server::api::middlewares::workspace_context::PreaggCacheCtx;
use oxy_app::server::api::projects::query::DataPlaneQueryExecutor;
use oxy_compile::{CompileRequest, RevisionKind, compile_workspace, compiler_version};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, publish_build, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::staging_functions::{call_on, data, make_guest_staff, production_host, staging_host};

/// `ctx.semantic` over a one-row view.
const SEMANTIC_JS: &str = r#"
export default async (req, ctx) => {
  const { rows } = await ctx.semantic.query({ topic: "orders", dimensions: ["orders.label"], measures: ["orders.n"] });
  return Response.json({ channel: ctx.channel, rows });
};
"#;

fn labels() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: "labels",
        manifest: json!({ "route": true, "timeoutSeconds": 60 }),
        js: SEMANTIC_JS,
    }]
}

/// A working copy with one view, `orders`, whose only row is `label`, and —
/// with `rollup` — a pre-aggregation over both of its members.
fn semantic_workspace(label: &str, rollup: bool) -> tempfile::TempDir {
    let root = write_config("  - name: duck\n    type: duckdb\n    path: semantic.duckdb\n");
    let dir = root.path().join("semantics");
    std::fs::create_dir_all(&dir).expect("semantics dir");
    let pre_aggregations = if rollup {
        "pre_aggregations:\n- name: by_label\n  dimensions:\n  - label\n  measures:\n  - n\n"
    } else {
        ""
    };
    std::fs::write(
        dir.join("orders.view.yml"),
        format!(
            "name: orders\ndatasource: duck\nsql: |\n  SELECT '{label}' AS label\n\
             dimensions:\n- name: label\n  type: string\n  expr: label\n\
             measures:\n- name: n\n  type: count\n{pre_aggregations}"
        ),
    )
    .expect("write view");
    std::fs::write(
        dir.join("orders.topic.yml"),
        "name: orders\nviews:\n- orders\n",
    )
    .expect("write topic");
    root
}

/// Compile `root` as a `staging` revision of `workspace` — never promoted.
async fn stage(t: &Tenant, workspace: Uuid, root: &std::path::Path) -> Uuid {
    use oxy_app::server::compile_config_gate::runtime_config_gate;
    let staged = compile_workspace(CompileRequest {
        db: &t.db,
        workspace_id: workspace,
        workspace_path: root,
        git_sha: None,
        branch: Some("feat/orders-label".into()),
        compiler_version: compiler_version(),
        promote: true, // asked for, and refused: staging never promotes
        kind: RevisionKind::Staging,
        owner_user_id: None,
        config_gate: Some(runtime_config_gate()),
    })
    .await
    .expect("compile the branch");
    assert!(staged.failures.is_empty(), "{:?}", staged.failures);
    staged.revision_id
}

async fn current_revision(t: &Tenant, workspace: Uuid) -> Option<Uuid> {
    entity::workspaces::Entity::find_by_id(workspace)
        .one(&t.db)
        .await
        .unwrap()
        .unwrap()
        .current_revision_id
}

/// An app whose production build and staging build both run [`labels`], with
/// the staging build pinned to `revision`.
async fn publish_pinned(t: &Tenant, app: &str, workspace: Uuid, revision: Uuid) {
    let live = publish_build(t, app, workspace, "sem-prod", true, &labels()).await;
    publish_build(t, app, workspace, "sem-stg", false, &labels()).await;
    let row = apps::Entity::find_by_id(live.app_id)
        .one(&t.db)
        .await
        .unwrap()
        .unwrap();
    let mut build: app_builds::ActiveModel =
        app_builds::Entity::find_by_id(row.draft_build_id.expect("a staging build"))
            .one(&t.db)
            .await
            .unwrap()
            .unwrap()
            .into();
    build.semantic_revision_id = ActiveValue::Set(Some(revision));
    build.update(&t.db).await.expect("pin the staging build");
}

/// #3370: the staging build pins a revision compiled from a branch; its
/// functions read it, production reads the promoted model, and nothing moves
/// `current_revision_id`.
#[tokio::test]
async fn a_staging_function_reads_its_builds_pinned_model_and_production_reads_main() {
    let t = throwaway_org(&seeded_tenant().await).await;
    let main_root = semantic_workspace("from-main", false);
    let workspace = compile(&t, main_root.path()).await;
    let promoted = current_revision(&t, workspace).await;
    let branch_root = semantic_workspace("from-branch", false);
    let staged = stage(&t, workspace, branch_root.path()).await;
    let app = "stg-semantic";
    publish_pinned(&t, app, workspace, staged).await;
    make_guest_staff();

    let staging = call_on(&t, app, "labels", &staging_host(&t, app), &[]).await;
    let got = data(&staging).to_string();
    assert!(got.contains("\"staging\""), "{got}");
    assert!(got.contains("from-branch"), "staging reads the pin: {got}");
    let production = call_on(&t, app, "labels", &production_host(&t, app), &[]).await;
    let got = data(&production).to_string();
    assert!(got.contains("from-main"), "production reads main: {got}");
    assert_eq!(
        current_revision(&t, workspace).await,
        promoted,
        "staging never moves the pointer"
    );
}

/// Removes the rollup cache this test writes under the process state dir,
/// even when an assertion panics.
struct CacheDirGuard(std::path::PathBuf);

impl Drop for CacheDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A rollup of `orders.by_label` for `workspace` whose only row says
/// `from-rollup`: a Parquet file and the manifest that lists it, under the
/// hash the view at `root` declares — so the rollup is live for that model.
fn write_rollup(workspace: Uuid, root: &std::path::Path) -> CacheDirGuard {
    let layer = oxy_airlayer_compat::load_layer_from_dir(root).expect("layer loads");
    let views: Vec<&oxy_airlayer_compat::View> = layer.views.iter().collect();
    let (_, hash) = oxy_airlayer_compat::preagg::live_rollups(&views)
        .into_iter()
        .find(|(view, _)| view == "orders")
        .expect("the view declares a rollup");
    let dir = oxy::state_dir::get_airlayer_cache_dir(workspace);
    std::fs::create_dir_all(&dir).expect("cache dir");
    let guard = CacheDirGuard(dir.clone());
    let file = format!("orders__{hash}.parquet");
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    conn.execute_batch(&format!(
        "COPY (SELECT 'from-rollup' AS label, 1::BIGINT AS n__count) TO '{}' (FORMAT PARQUET);",
        dir.join(&file).display()
    ))
    .expect("write the rollup");
    let manifest = json!({
        "pulled_at": "2026-09-29T00:00:00Z",
        "source_database": "duck",
        "rollups": [{
            "view_name": "orders",
            "rollup_name": "by_label",
            "rollup_hash": hash,
            "file": file,
            "dimensions": ["label"],
            "measures": [{ "name": "n", "type": "count", "columns": ["n__count"] }],
            "time_dimension": null,
            "granularity": null,
            "build_date": "2026-09-29 00:00:00"
        }]
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).expect("manifest");
    guard
}

/// The serve route with a Layer-1 rollup cache, as `serve.rs` mounts it on a
/// node that runs the rebuild worker.
fn serve_with_rollups() -> Router {
    let preagg = PreaggCacheCtx {
        cache: Some(Arc::new(std::sync::RwLock::new(
            agentic_semantic::refresh_key_cache::RefreshKeyCache::new(),
        ))),
        renewal_threshold_secs: Some(3600),
    };
    Router::new()
        .route(
            "/customer-apps/{*path}",
            any(custom_apps_serve::serve_dispatch)
                .layer(axum::Extension(
                    Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>
                )),
        )
        .layer(axum::Extension(preagg))
}

async fn labels_on(t: &Tenant, app: &str, host: &str) -> String {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/customer-apps/{}/{app}/fn/labels", t.org_slug))
        .header("host", host)
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .expect("request");
    let response = serve_with_rollups()
        .oneshot(request)
        .await
        .expect("oneshot");
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A rollup built from the promoted model answers production; under the
/// staging pin it stands aside even though the branch model declares the very
/// same rollup, and the warehouse answers instead.
#[tokio::test]
async fn a_rollup_answers_production_and_never_a_staging_function() {
    let t = throwaway_org(&seeded_tenant().await).await;
    let main_root = semantic_workspace("from-warehouse", true);
    let workspace = compile(&t, main_root.path()).await;
    let branch_root = semantic_workspace("from-warehouse", true);
    let staged = stage(&t, workspace, branch_root.path()).await;
    let _cache = write_rollup(workspace, main_root.path());
    let app = "stg-rollup";
    publish_pinned(&t, app, workspace, staged).await;
    make_guest_staff();

    let production = labels_on(&t, app, &production_host(&t, app)).await;
    assert!(
        production.contains("from-rollup"),
        "the rollup is live and answers production: {production}"
    );
    let staging = labels_on(&t, app, &staging_host(&t, app)).await;
    assert!(staging.contains("\"staging\""), "{staging}");
    assert!(
        staging.contains("from-warehouse") && !staging.contains("from-rollup"),
        "under the staging pin the rollup stands aside: {staging}"
    );
}
