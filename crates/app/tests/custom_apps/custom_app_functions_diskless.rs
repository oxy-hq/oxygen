//! A function invocation on a pod with no working copy — what makes
//! `POST /customer-apps/<org>/<app>/fn/<name>` `FleetOk`.
//!
//! The route was `IdeOnly` while a function read `config.yml` and the semantic
//! model off the working copy. Both read the promoted revision now, so a
//! `serve` replica — which holds no checkout — runs the function itself. The
//! claim is only worth a test that takes the checkout away:
//!
//! 1. the workspace is compiled and promoted, the app published;
//! 2. the working copy is **deleted**, so `workspaces.path` names a directory
//!    that is not on this node — a replica's exact position;
//! 3. the process declares itself one (`workspace_fs_probe`), which is what
//!    `OXY_ROLE=serve` does at boot;
//! 4. a real invocation runs through `serve_dispatch` and the isolate.
//!
//! It must answer from the compiled model and the compiled config, and the
//! probe — which counts every workspace path resolved on a diskless process —
//! must stay at zero. A function that still worked only because some arm
//! quietly fell back to disk would fail the first; one that reached for the
//! disk and got lucky would fail the second.
//!
//! The warehouse is Postgres because a replica can reach it.
//!
//! The other two cases are the workspaces a replica can NOT serve, and must
//! not try to. In production each is replayed to the Factory; a test process
//! has no `OXY_IDE_UPSTREAM`, so what is observable here is the refusal that
//! stands in for the forward, and that the isolate never ran:
//!
//! - **nothing compiled** — it must not run against an empty config. A
//!   retryable 503, and a compile is enqueued;
//! - **a database that is a file in the checkout** (a DuckDB `path:` with no S3
//!   mirror) — a 503 naming the pod it needs, and NO compile: the workspace is
//!   compiled already, and recompiling per call would be a storm.
//!
//! The bundle comes from the filesystem build store here (`test_db` unsets
//! `OXY_CUSTOMER_APPS_S3_BUCKET`); that is the state dir, not the working
//! copy, and a deployed fleet reads it from S3.
//!
//! `custom_app_functions_forwarded` runs the same three workspaces with a
//! Factory reachable, and shares the helpers below.

use axum::http::StatusCode;
use entity::workspaces;
use oxy::workspace_fs_probe::{leaks, reset_leaks, set_process_owns_workspace_files};
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, Statement};
use serde_json::json;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, call_function_in, invocations, publish_app, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::warehouse_writes_on_engines::postgres_entry;

/// One call, three reads of the workspace: the semantic model, the default
/// database (`ctx.query` names none, so the config picks it), and a database
/// by name.
const READS_JS: &str = r#"
export default async (req, ctx) => {
  const semantic = await ctx.semantic.query({ topic: "orders", dimensions: ["orders.label"], measures: ["orders.n"] });
  const byDefault = await ctx.query("SELECT 41 + 1 AS answer");
  const byName = await ctx.warehouse.query("pg", "SELECT 'named' AS via");
  return Response.json({ semantic: semantic.rows, byDefault: byDefault.rows, byName: byName.rows });
};
"#;

pub(crate) const READS: &str = "reads";

pub(crate) fn functions() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: READS,
        manifest: json!({ "route": true, "timeoutSeconds": 60 }),
        js: READS_JS,
    }]
}

/// A registered workspace row in the tenant's org, never compiled, whose
/// directory is not on this node.
pub(crate) async fn uncompiled_workspace(t: &Tenant) -> Uuid {
    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace),
        name: ActiveValue::Set("Never compiled".into()),
        org_id: ActiveValue::Set(Some(t.org_id)),
        path: ActiveValue::Set(Some(format!("/nonexistent/workspaces/{workspace}"))),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed workspace");
    workspace
}

/// The process as a `serve` replica: it owns no working copy, and the leak
/// counter starts from zero. Restored on drop; nextest gives each test its own
/// process (`common::fresh_db` asserts it), so the flag reaches no other test.
pub(crate) struct AsDisklessReplica;

impl AsDisklessReplica {
    pub(crate) fn enter() -> Self {
        set_process_owns_workspace_files(false);
        reset_leaks();
        AsDisklessReplica
    }
}

impl Drop for AsDisklessReplica {
    fn drop(&mut self) {
        set_process_owns_workspace_files(true);
        reset_leaks();
    }
}

pub(crate) fn write_view(root: &std::path::Path) {
    let dir = root.join("semantics");
    std::fs::create_dir_all(&dir).expect("semantics dir");
    std::fs::write(
        dir.join("orders.view.yml"),
        "name: orders\ndatasource: pg\nsql: |\n  SELECT 'from-compiled' AS label\n\
         dimensions:\n- name: label\n  type: string\n  expr: label\n\
         measures:\n- name: n\n  type: count\n",
    )
    .expect("write view");
    std::fs::write(
        dir.join("orders.topic.yml"),
        "name: orders\nviews:\n- orders\n",
    )
    .expect("write topic");
}

/// Compile tasks queued for `workspace`'s main revision.
pub(crate) async fn queued_compiles(t: &Tenant, workspace: Uuid) -> i64 {
    let row =
        t.db.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM agentic_task_queue \
             WHERE spec->>'type' = 'compile' AND spec->>'workspace_id' = $1",
            [workspace.to_string().into()],
        ))
        .await
        .expect("query the task queue")
        .expect("count returns a row");
    row.try_get::<i64>("", "n").expect("n")
}

#[tokio::test]
async fn a_function_runs_on_a_pod_with_no_working_copy() {
    let t = throwaway_org(&seeded_tenant().await).await;
    let root = write_config(&postgres_entry("pg").await);
    write_view(root.path());
    let workspace = compile(&t, root.path()).await;
    let slug = "diskless";
    publish_app(&t, slug, workspace, &functions()).await;

    // The checkout is gone: `workspaces.path` now names a directory this node
    // does not have, exactly as it does on a replica.
    let gone = root.path().to_path_buf();
    drop(root);
    assert!(!gone.exists(), "the working copy must really be absent");

    let _replica = AsDisklessReplica::enter();
    let call = call_function_in(&t.org_slug, slug, READS, json!({})).await;

    assert_eq!(call.status, StatusCode::OK, "stream: {}", call.raw);
    let data = call
        .frame("data")
        .unwrap_or_else(|| panic!("no data frame; stream: {}", call.raw));
    assert_eq!(call.frame("done"), Some(&json!({ "status": 200 })));
    // Read as text: the row key a semantic query answers under is airlayer's
    // to choose, and a Postgres integer may arrive as a number or a string.
    let (semantic, by_default, by_name) = (
        data["semantic"].to_string(),
        data["byDefault"].to_string(),
        data["byName"].to_string(),
    );
    assert!(
        semantic.contains("from-compiled"),
        "ctx.semantic answers from the compiled model: {data}"
    );
    assert!(
        by_default.contains("42"),
        "ctx.query found its default database in the compiled config: {data}"
    );
    assert!(
        by_name.contains("named"),
        "ctx.warehouse.query built a connector from the compiled config: {data}"
    );
    assert_eq!(
        leaks(),
        0,
        "the invocation resolved a workspace path on a pod that holds no \
         working copy — something on this route still reaches for the disk"
    );
}

#[tokio::test]
async fn an_uncompiled_workspace_is_refused_on_a_replica_not_run_empty() {
    let t = throwaway_org(&seeded_tenant().await).await;
    let workspace = uncompiled_workspace(&t).await;
    let slug = "uncompiled";
    let app = publish_app(&t, slug, workspace, &functions()).await;
    assert_eq!(
        queued_compiles(&t, workspace).await,
        0,
        "nothing queued yet"
    );

    let _replica = AsDisklessReplica::enter();
    let call = call_function_in(&t.org_slug, slug, READS, json!({})).await;

    assert_eq!(
        call.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "not compiled yet is retryable, never a run against an empty config: {}",
        call.raw
    );
    let body: serde_json::Value = serde_json::from_str(&call.raw).expect("a JSON refusal");
    assert_eq!(body["error"], "WorkspaceNotCompiled", "{body}");
    assert_eq!(
        queued_compiles(&t, workspace).await,
        1,
        "the refusal queues the compile that makes the next call servable here"
    );
    assert!(
        invocations(&t.db, app.app_id, READS).await.is_empty(),
        "refused before the invocation row: a `running` row here would make \
         the Factory reject the replayed call as a concurrent duplicate"
    );
    assert_eq!(leaks(), 0, "the refusal itself must not reach for the disk");
}

#[tokio::test]
async fn a_working_copy_database_keeps_its_workspace_off_a_replica() {
    let t = throwaway_org(&seeded_tenant().await).await;
    // A DuckDB file in the checkout. No `OXY_COMPILE_BLOB_S3_BUCKET` in a test
    // process, so the compiler mirrors nothing to S3: the data is only here.
    let root = write_config("  - name: duck\n    type: duckdb\n    path: local.duckdb\n");
    let workspace = compile(&t, root.path()).await;
    let slug = "working-copy-db";
    let app = publish_app(&t, slug, workspace, &functions()).await;
    drop(root);

    let _replica = AsDisklessReplica::enter();
    let call = call_function_in(&t.org_slug, slug, READS, json!({})).await;

    assert_eq!(call.status, StatusCode::SERVICE_UNAVAILABLE, "{}", call.raw);
    let body: serde_json::Value = serde_json::from_str(&call.raw).expect("a JSON refusal");
    assert_eq!(body["error"], "WorkspaceNeedsWorkingCopy", "{body}");
    assert_eq!(
        queued_compiles(&t, workspace).await,
        0,
        "the workspace is compiled; a compile per call would change nothing"
    );
    assert!(
        invocations(&t.db, app.app_id, READS).await.is_empty(),
        "the isolate must not run where the database cannot be read"
    );
    assert_eq!(leaks(), 0, "the refusal itself must not reach for the disk");
}
