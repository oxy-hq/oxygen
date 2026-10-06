//! The world-model graph on a pod with no working copy — what makes the six
//! `/semantic/world-model*` routes `FleetOk`.
//!
//! They were `IdeOnly` while their handlers read the semantic model from
//! `semantics_scan_path()`, the working copy. They resolve the scan through
//! `resolve_query_scan_source` now, as the metric tree does. The claim is only
//! worth a test that takes the checkout away, which is the shape of
//! `custom_app_functions_diskless` — whose fixtures this reuses:
//!
//! 1. a workspace with a view, an entity and a `.world-model.yml` is compiled
//!    and promoted;
//! 2. the working copy is **deleted**;
//! 3. the process declares itself a replica (`workspace_fs_probe`);
//! 4. the real handlers run against the manager `workspace_middleware` hands a
//!    request: pinned to the promoted revision, over a root this node lacks.
//!
//! The other two cases are what a replica must NOT answer with an empty graph
//! or an empty list: a model it cannot read yet, and a database that is a file
//! in the checkout.

use std::path::Path as FsPath;

use axum::extract::{Json, Path, Query, State};
use axum::http::StatusCode;
use entity::workspace_members::WorkspaceRole;
use entity::workspaces;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::{OnMissing, ReadOnly};
use oxy::workspace_fs_probe::leaks;
use oxy_app::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, SemanticEngineCacheCtx, SemanticLayerCacheCtx, WorkspaceManagerReadOnly,
};
use oxy_app::server::api::semantic::{ErrorResponse, WorkspacePath};
use oxy_app::server::api::world_model_graph::{
    WmFilterCountsRequest, WmInstanceDetailQuery, WmInstancesQuery, WmMeasureBreakdownQuery,
    get_world_model, get_world_model_instance_detail, get_world_model_instances,
    get_world_model_measure_breakdown, post_world_model_filter_counts,
};
use oxy_app::server::router::{AppState, bare_app_state};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::EntityTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_diskless::{
    AsDisklessReplica, queued_compiles, uncompiled_workspace,
};
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant, throwaway_org};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::warehouse_writes_on_engines::postgres_entry;

/// One entity (`order`) over one view reading `datasource`.
fn write_view(root: &FsPath, datasource: &str) {
    let dir = root.join("semantics");
    std::fs::create_dir_all(&dir).expect("semantics dir");
    std::fs::write(
        dir.join("orders.view.yml"),
        format!(
            "name: orders\ndatasource: {datasource}\nsql: |\n  SELECT 'from-compiled' AS label\n\
             entities:\n- name: order\n  type: primary\n  key: label\n\
             dimensions:\n- name: label\n  type: string\n  expr: label\n\
             measures:\n- name: n\n  type: count\n"
        ),
    )
    .expect("write view");
}

/// [`write_view`], and a display config that renames the entity — so a graph
/// that carries the label read BOTH the compiled model and the compiled
/// `.world-model.yml`.
fn write_model(root: &FsPath, datasource: &str) {
    write_view(root, datasource);
    std::fs::write(
        root.join(".world-model.yml"),
        "entities:\n- id: order\n  label: Orders (compiled)\n",
    )
    .expect("write .world-model.yml");
}

/// The manager `workspace_middleware` hands a request on a replica: pinned to
/// whatever revision the workspace has promoted, over a root that is not on
/// this node, with the filesystem capability dropped.
async fn replica_manager(
    t: &Tenant,
    workspace: Uuid,
    absent: &FsPath,
) -> WorkspaceManager<ReadOnly> {
    assert!(!absent.exists(), "the working copy must really be absent");
    let revision = workspaces::Entity::find_by_id(workspace)
        .one(&t.db)
        .await
        .expect("load workspace")
        .and_then(|w| w.current_revision_id);
    WorkspaceBuilder::new(workspace)
        .with_working_copy(absent, revision, OnMissing::Empty)
        .await
        .expect("workspace builder")
        .build()
        .await
        .expect("workspace manager")
        .into_read_only()
}

/// The per-workspace caches the middleware attaches, over one process's
/// `AppState`.
struct Request {
    workspace: Uuid,
    manager: WorkspaceManager<ReadOnly>,
    state: AppState,
}

impl Request {
    fn layer_cache(&self) -> SemanticLayerCacheCtx {
        SemanticLayerCacheCtx {
            cache: self.state.semantic_layer_cache.clone(),
            workspace_id: self.workspace,
            engine_cache: self.state.semantic_engine_cache.clone(),
        }
    }

    fn engine_cache(&self) -> SemanticEngineCacheCtx {
        SemanticEngineCacheCtx {
            cache: self.state.semantic_engine_cache.clone(),
            workspace_id: self.workspace,
        }
    }

    fn user(&self) -> AuthenticatedUserExtractor {
        AuthenticatedUserExtractor(AuthenticatedUser {
            id: Uuid::new_v4(),
            email: Some("builder@acme.test".into()),
            name: "Builder".into(),
            picture: None,
            status: entity::users::UserStatus::Active,
        })
    }

    async fn graph(&self) -> Result<Value, (StatusCode, String)> {
        get_world_model(
            WorkspaceManagerReadOnly(self.manager.clone()),
            self.layer_cache(),
            Path(WorkspacePath {
                workspace_id: self.workspace,
            }),
        )
        .await
        .map(|Json(graph)| serde_json::to_value(graph).expect("graph serialises"))
        .map_err(refusal)
    }

    async fn instances(&self, entity: &str) -> Result<Value, (StatusCode, String)> {
        let query: WmInstancesQuery =
            serde_json::from_value(json!({ "entity": entity })).expect("instances query");
        get_world_model_instances(
            WorkspaceManagerReadOnly(self.manager.clone()),
            self.user(),
            EffectiveWorkspaceRole(WorkspaceRole::Admin),
            self.layer_cache(),
            self.engine_cache(),
            State(self.state.clone()),
            Path(WorkspacePath {
                workspace_id: self.workspace,
            }),
            Query(query),
        )
        .await
        .map(|Json(page)| serde_json::to_value(page).expect("page serialises"))
        .map_err(refusal)
    }

    /// Only the refusal: an accepted request is an SSE stream.
    async fn filter_counts_refusal(&self, entity: &str) -> Option<(StatusCode, String)> {
        post_world_model_filter_counts(
            WorkspaceManagerReadOnly(self.manager.clone()),
            self.user(),
            EffectiveWorkspaceRole(WorkspaceRole::Admin),
            self.layer_cache(),
            self.engine_cache(),
            State(self.state.clone()),
            Path(WorkspacePath {
                workspace_id: self.workspace,
            }),
            Json(WmFilterCountsRequest {
                entity_id: entity.to_string(),
                key_value: "from-compiled".to_string(),
            }),
        )
        .await
        .err()
        .map(refusal)
    }

    /// Only the refusal: an accepted request is an SSE stream.
    async fn instance_detail_refusal(&self, entity: &str) -> Option<(StatusCode, String)> {
        let query: WmInstanceDetailQuery =
            serde_json::from_value(json!({ "entity": entity, "key": "from-compiled" }))
                .expect("instance-detail query");
        get_world_model_instance_detail(
            WorkspaceManagerReadOnly(self.manager.clone()),
            self.user(),
            EffectiveWorkspaceRole(WorkspaceRole::Admin),
            self.layer_cache(),
            self.engine_cache(),
            Path(WorkspacePath {
                workspace_id: self.workspace,
            }),
            Query(query),
        )
        .await
        .err()
        .map(refusal)
    }

    /// Only the refusal: an accepted request is an SSE stream.
    async fn measure_breakdown_refusal(&self, entity: &str) -> Option<(StatusCode, String)> {
        let query: WmMeasureBreakdownQuery = serde_json::from_value(
            json!({ "entity": entity, "key": "from-compiled", "measure": "n" }),
        )
        .expect("measure-breakdown query");
        get_world_model_measure_breakdown(
            WorkspaceManagerReadOnly(self.manager.clone()),
            self.user(),
            EffectiveWorkspaceRole(WorkspaceRole::Admin),
            self.layer_cache(),
            self.engine_cache(),
            Path(WorkspacePath {
                workspace_id: self.workspace,
            }),
            Query(query),
        )
        .await
        .err()
        .map(refusal)
    }
}

fn refusal(
    (status, Json(ErrorResponse { message })): (StatusCode, Json<ErrorResponse>),
) -> (StatusCode, String) {
    (status, message)
}

async fn request(t: &Tenant, workspace: Uuid, absent: &FsPath) -> Request {
    Request {
        workspace,
        manager: replica_manager(t, workspace, absent).await,
        state: bare_app_state(),
    }
}

#[tokio::test]
async fn the_graph_and_its_instances_are_served_with_the_working_copy_gone() {
    let t = throwaway_org(&seeded_tenant().await).await;
    let root = write_config(&postgres_entry("pg").await);
    write_model(root.path(), "pg");
    let workspace = compile(&t, root.path()).await;
    let gone = root.path().to_path_buf();
    drop(root);

    let _replica = AsDisklessReplica::enter();
    let request = request(&t, workspace, &gone).await;

    let graph = request.graph().await.expect("the graph is served");
    let entities = graph["entities"].as_array().expect("entities");
    assert_eq!(entities.len(), 1, "{graph}");
    assert_eq!(
        entities[0]["id"], "order",
        "the entity of the compiled view"
    );
    assert_eq!(
        entities[0]["label"], "Orders (compiled)",
        "the label of the compiled .world-model.yml: {graph}"
    );

    let page = request
        .instances("order")
        .await
        .expect("instances are served");
    assert!(
        page.to_string().contains("from-compiled"),
        "the picker ran the compiled view against the warehouse in the compiled config: {page}"
    );

    assert_eq!(
        leaks(),
        0,
        "a world-model read resolved a workspace path on a pod that holds no \
         working copy — something on these routes still reaches for the disk"
    );
}

#[tokio::test]
async fn a_model_this_pod_cannot_read_yet_is_a_retryable_503_not_an_empty_graph() {
    let t = throwaway_org(&seeded_tenant().await).await;
    let workspace = uncompiled_workspace(&t).await;
    let absent = std::path::PathBuf::from(format!("/nonexistent/workspaces/{workspace}"));
    assert_eq!(
        queued_compiles(&t, workspace).await,
        0,
        "nothing queued yet"
    );

    let _replica = AsDisklessReplica::enter();
    let request = request(&t, workspace, &absent).await;
    let (status, message) = request
        .graph()
        .await
        .expect_err("no compiled model and no working copy: there is nothing to draw");

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "not compiled yet is retryable, never an empty graph: {message}"
    );
    assert!(message.contains("no compiled semantic model"), "{message}");
    assert_eq!(
        queued_compiles(&t, workspace).await,
        1,
        "the refusal queues the compile that makes the next call servable here"
    );
}

#[tokio::test]
async fn a_database_in_the_checkout_is_refused_by_name_not_listed_empty() {
    let t = throwaway_org(&seeded_tenant().await).await;
    // A DuckDB file in the checkout. No `OXY_COMPILE_BLOB_S3_BUCKET` in a test
    // process, so the compiler mirrors nothing to S3: the data is only there.
    let root = write_config("  - name: duck\n    type: duckdb\n    path: local.duckdb\n");
    write_model(root.path(), "duck");
    let workspace = compile(&t, root.path()).await;
    let gone = root.path().to_path_buf();
    drop(root);

    let _replica = AsDisklessReplica::enter();
    let request = request(&t, workspace, &gone).await;

    // The graph is structure only — it queries no database, so it is served.
    let graph = request.graph().await.expect("the graph needs no database");
    assert_eq!(graph["entities"][0]["id"], "order", "{graph}");

    let mut wrong = Vec::new();
    for (what, refused) in [
        ("instances", request.instances("order").await.err()),
        (
            "filter-counts",
            request.filter_counts_refusal("order").await,
        ),
        (
            "instance-detail",
            request.instance_detail_refusal("order").await,
        ),
        (
            "measure-breakdown",
            request.measure_breakdown_refusal("order").await,
        ),
    ] {
        // Every route is judged before the test fails, so one run names all
        // the routes that are wrong rather than the first.
        match refused {
            None => wrong.push(format!(
                "{what} answered on a pod that cannot open the database — its failed \
                 queries read as an empty page or zero counts"
            )),
            Some((status, message)) => {
                let named = message.contains("`duck`") && message.contains("working copy");
                if status != StatusCode::SERVICE_UNAVAILABLE || !named {
                    wrong.push(format!(
                        "{what}: expected a 503 naming `duck` and the working copy, got \
                         {status}: {message}"
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert_eq!(leaks(), 0, "the refusal itself must not reach for the disk");
}

/// Most workspaces have no `.world-model.yml`. On a pod with no working copy
/// the pinned revision is the only place to look, and its having no row is the
/// whole answer — "no display config", every entity drawn. Whatever is done
/// about a config read that fails, this case has to keep drawing the graph.
#[tokio::test]
async fn a_revision_with_no_display_config_draws_every_entity_on_a_replica() {
    let t = throwaway_org(&seeded_tenant().await).await;
    let root = write_config(&postgres_entry("pg").await);
    write_view(root.path(), "pg");
    let workspace = compile(&t, root.path()).await;
    let gone = root.path().to_path_buf();
    drop(root);

    let _replica = AsDisklessReplica::enter();
    let request = request(&t, workspace, &gone).await;
    let graph = request
        .graph()
        .await
        .expect("a revision with no display config still has a graph to draw");

    let entities = graph["entities"].as_array().expect("entities");
    assert_eq!(entities.len(), 1, "{graph}");
    assert_eq!(entities[0]["id"], "order", "{graph}");
    assert_ne!(
        entities[0]["label"], "Orders (compiled)",
        "no display config was compiled, so nothing renames the entity: {graph}"
    );
    assert_eq!(leaks(), 0, "the read must not reach for the disk");
}
