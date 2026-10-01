//! Held procedure runs on a previewed branch (phase 2a), end to end: submitted
//! through the route, queued per workspace, driven by the real global-run
//! driver with the production `PreviewRunResolver`, and read back.
//!
//! The staging config names a ClickHouse nothing listens on and a managed
//! Airhouse this server holds no credentials for, so a write that was NOT held
//! would fail its step and the run (I2). Database-backed (`Schema::All`).

mod airhouse;
mod airhouse_fences;
mod fences;
mod transforms;
mod world;

use std::sync::Arc;

use agentic_automation::WorkspaceContext;
use agentic_automation::preview_names::scoped;
use agentic_pipeline::platform::{PlatformContext, RunPlatformResolver};
use axum::http::StatusCode;
use oxy_app::server::previews::runs::advance;
use oxy_app::server::previews::runtime::PreviewRunResolver;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::preview_routes::fixture::{BRANCH, exec, send_json};
use world::{PROCEDURE, base_ctx, enable_runs, run_to_the_end, submit};

fn write_then_format(database: &str, sql: &str) -> Value {
    json!([
        { "name": "write", "type": "execute_sql", "database": database, "sql_query": sql },
        { "name": "after", "type": "formatter", "template": "the procedure went on" }
    ])
}

/// I2: a ClickHouse write in a preview procedure is held — the step succeeds
/// with the hold recorded, the procedure goes on, and nothing reached a
/// connector (one would have failed: nothing listens on that port).
#[tokio::test]
async fn clickhouse_writes_in_a_preview_procedure_are_held() {
    let sql = "INSERT INTO analytics.journal SELECT 1";
    let fx = world::world(write_then_format("clickhouse", sql)).await;
    let detail = run_to_the_end(&fx).await;

    assert_eq!(detail["state"], "finished", "{detail}");
    assert_eq!(detail["outcome"], "succeeded", "{detail}");
    assert_eq!(detail["held_count"], 1);
    assert_eq!(detail["target_ref"], PROCEDURE);
    assert_eq!(detail["revision_id"], fx.staging.to_string());
    let steps = detail["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "{detail}");
    assert_eq!(steps[0]["name"], "write");
    assert_eq!(steps[0]["kind"], "execute_sql");
    assert_eq!(steps[0]["status"], "held");
    let held = &steps[0]["held"];
    assert_eq!(held["verb"], "INSERT");
    assert!(held["targets"].to_string().contains("journal"), "{held}");
    assert_eq!(held["sql"], sql);
    assert!(
        held["reason"].as_str().unwrap().contains("clickhouse"),
        "{held}"
    );
    assert_eq!(steps[1]["status"], "succeeded");
    assert_eq!(steps[1]["held"], Value::Null);

    // The listing agrees.
    let (_, list) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/runs?branch=feat%2Fje-v2", fx.ws),
        None,
    )
    .await;
    assert_eq!(list[0]["run_id"], detail["run_id"]);
    assert_eq!(list[0]["held_count"], 1);
}

/// Where no Airhouse can confine a preview Writer — here, none is configured
/// at all — a managed-Airhouse write, DML or DDL, is held as in phase 2a,
/// like a warehouse one, and a held run never needs an Airhouse credential.
/// (Where one can, it lands in the preview: `airhouse::`.)
#[tokio::test]
async fn airhouse_writes_are_held_where_no_airhouse_confines_a_writer() {
    let tasks = json!([
        { "name": "land", "type": "execute_sql", "database": "airhouse",
          "sql_query": "INSERT INTO nces.schools SELECT 1" },
        { "name": "shape", "type": "execute_sql", "database": "airhouse",
          "sql_query": "CREATE TABLE nces.rollup AS SELECT 1 AS n" }
    ]);
    let fx = world::world(tasks).await;
    let detail = run_to_the_end(&fx).await;
    assert_eq!(detail["outcome"], "succeeded", "{detail}");
    assert_eq!(detail["held_count"], 2);
    let steps = detail["steps"].as_array().unwrap();
    assert_eq!(steps[0]["held"]["verb"], "INSERT");
    assert_eq!(steps[1]["status"], "held");
    assert!(
        steps[1]["held"]["verb"]
            .as_str()
            .unwrap()
            .starts_with("CREATE"),
        "{detail}"
    );
}

#[tokio::test]
async fn disabled_flag_answers_404() {
    let fx = world::world(write_then_format("clickhouse", "INSERT INTO t SELECT 1")).await;
    enable_runs(false);
    let body = json!({ "branch": BRANCH, "kind": "procedure", "ref": PROCEDURE });
    for (method, uri, body) in [
        ("POST", format!("/{}/previews/runs", fx.ws), Some(body)),
        (
            "GET",
            format!("/{}/previews/runs?branch=feat%2Fje-v2", fx.ws),
            None,
        ),
        (
            "GET",
            format!("/{}/previews/runs/{}", fx.ws, Uuid::new_v4()),
            None,
        ),
    ] {
        let (status, resp) = send_json(&fx.staff, method, uri.clone(), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {resp}");
        assert_eq!(resp["code"], "preview_runs_disabled", "{method} {uri}");
    }
    let rows = fx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM workspace_preview_runs WHERE workspace_id = $1",
            [fx.ws.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rows.try_get::<i64>("", "n").unwrap(),
        0,
        "nothing was queued"
    );
}

/// No ready staging revision is 409 `preview_not_ready`; the other refusals
/// carry their own codes.
#[tokio::test]
async fn not_ready_answers_409() {
    let fx = world::world(write_then_format("clickhouse", "INSERT INTO t SELECT 1")).await;
    enable_runs(true);
    exec(
        &fx.db,
        "INSERT INTO workspace_previews (workspace_id, branch, git_sha, created_by) VALUES ($1, 'feat/wip', $2, $3)",
        vec![fx.ws.into(), "0123456789012345678901234567890123456789".into(), fx.staff.id.into()],
    )
    .await;
    let (status, body) = submit(&fx, "feat/wip", PROCEDURE).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "preview_not_ready");

    let (status, body) = submit(&fx, "feat/nobody-previews", PROCEDURE).await;
    assert_eq!(
        (status, body["code"].clone()),
        (StatusCode::NOT_FOUND, json!("preview_not_found"))
    );
    let (status, body) = submit(&fx, BRANCH, "workflows/missing.procedure.yml").await;
    assert_eq!(
        (status, body["code"].clone()),
        (StatusCode::NOT_FOUND, json!("ref_not_in_revision"))
    );
    // A kind staff cannot start (builds are queued by change checks).
    let (status, body) = send_json(
        &fx.staff,
        "POST",
        format!("/{}/previews/runs", fx.ws),
        Some(json!({ "branch": BRANCH, "kind": "transform_build", "ref": PROCEDURE })),
    )
    .await;
    assert_eq!(
        (status, body["code"].clone()),
        (StatusCode::BAD_REQUEST, json!("bad_request"))
    );
    // An Airway sample (2b) of a path that is no pipeline in the revision.
    let (status, body) = send_json(
        &fx.staff,
        "POST",
        format!("/{}/previews/runs", fx.ws),
        Some(json!({ "branch": BRANCH, "kind": "airway_sample", "ref": PROCEDURE })),
    )
    .await;
    assert_eq!(
        (status, body["code"].clone()),
        (StatusCode::NOT_FOUND, json!("ref_not_in_revision"))
    );
}

/// I8: preview work serialises per workspace. The second run waits, queued and
/// unseeded, until the first finishes; the sweep then starts it.
#[tokio::test]
async fn a_second_run_queues_behind_the_first() {
    let fx = world::world(write_then_format("clickhouse", "INSERT INTO t SELECT 1")).await;
    enable_runs(true);
    let (_, first) = submit(&fx, BRANCH, PROCEDURE).await;
    let (_, second) = submit(&fx, BRANCH, PROCEDURE).await;
    assert_eq!(first["state"], "running", "{first}");
    assert_eq!(second["state"], "queued", "{second}");
    let (a, b) = (
        first["run_id"].as_str().unwrap(),
        second["run_id"].as_str().unwrap(),
    );
    assert!(
        agentic_runtime::crud::get_run(&fx.db, b)
            .await
            .unwrap()
            .is_none(),
        "a queued run is not seeded"
    );

    let (_, list) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/runs?branch=feat%2Fje-v2", fx.ws),
        None,
    )
    .await;
    assert_eq!(
        (list[0]["run_id"].as_str(), list[1]["run_id"].as_str()),
        (Some(b), Some(a)),
        "newest first"
    );

    agentic_runtime::crud::update_run_done(&fx.db, a, "", None)
        .await
        .unwrap();
    oxy_app::server::previews::runs::sweep(&fx.db).await;
    let state = |id: &str| {
        let id = id.to_string();
        let db = fx.db.clone();
        async move {
            let row = db
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT state FROM workspace_preview_runs WHERE run_id = $1",
                    [id.into()],
                ))
                .await
                .unwrap()
                .unwrap();
            row.try_get::<String>("", "state").unwrap()
        }
    };
    assert_eq!(state(a).await, "finished");
    assert_eq!(state(b).await, "running");
    let seeded = agentic_runtime::crud::get_run(&fx.db, b)
        .await
        .unwrap()
        .expect("seeded");
    assert_eq!(
        seeded.metadata.unwrap()["workflow_ref"],
        scoped(b, PROCEDURE)
    );
}

/// The `one_running` index is what settles a race: a second running row is
/// refused outright, and two concurrent `advance`s start exactly one run.
#[tokio::test]
async fn the_one_running_index_rejects_a_race() {
    let fx = world::world(write_then_format("clickhouse", "INSERT INTO t SELECT 1")).await;
    let insert = "INSERT INTO workspace_preview_runs (run_id, workspace_id, branch, preview_key, \
                  revision_id, kind, target_ref, state) VALUES ($1, $2, $3, 'k', $4, 'procedure', $5, $6)";
    let row = |state: &str| {
        vec![
            Uuid::new_v4().to_string().into(),
            fx.ws.into(),
            BRANCH.into(),
            fx.staging.into(),
            PROCEDURE.into(),
            state.into(),
        ]
    };
    exec(&fx.db, insert, row("running")).await;
    let second = fx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            insert,
            row("running"),
        ))
        .await;
    assert!(
        second.is_err(),
        "a second running preview run in one workspace must be refused"
    );

    exec(
        &fx.db,
        "DELETE FROM workspace_preview_runs WHERE workspace_id = $1",
        vec![fx.ws.into()],
    )
    .await;
    exec(&fx.db, insert, row("queued")).await;
    exec(&fx.db, insert, row("queued")).await;
    let (x, y) = tokio::join!(advance(&fx.db, fx.ws), advance(&fx.db, fx.ws));
    let started = [x.unwrap(), y.unwrap()].into_iter().flatten().count();
    assert_eq!(started, 1, "exactly one advance wins");
}

/// I6: the production platform — what a pod without preview code drives with —
/// resolves none of a preview run's scoped names, while it resolves the same
/// names unscoped (the control).
#[tokio::test]
async fn a_base_platform_fails_every_preview_scoped_name() {
    let fx = world::world(write_then_format("clickhouse", "INSERT INTO t SELECT 1")).await;
    let root = tempfile::tempdir().unwrap();
    let base = base_ctx(&fx, root.path()).await;
    let run = Uuid::new_v4().to_string();

    assert!(
        base.resolve_automation_yaml(PROCEDURE).await.is_ok(),
        "control: unscoped resolves"
    );
    assert!(
        base.is_database_configured("clickhouse"),
        "control: the database exists unscoped"
    );
    let err = base
        .resolve_automation_yaml(&scoped(&run, PROCEDURE))
        .await
        .expect_err("scoped ref");
    assert!(
        !err.is_unavailable(),
        "a missing ref, not a retryable one: {err}"
    );
    let err = match base.get_connector(&scoped(&run, "clickhouse")).await {
        Err(e) => e,
        Ok(_) => panic!("a scoped database name must not resolve on the base platform"),
    };
    assert!(err.contains("not found"), "{err}");
    assert!(
        !matches!(
            base.resolve_pipeline_yaml(&scoped(&run, "airway/nces.airway.yml"))
                .await,
            Ok(Some(_))
        ),
        "a scoped pipeline ref must not resolve on the base platform"
    );
}

/// I7: the resolver hands the preview platform only to a root the registry
/// names as a procedure run; an ordinary root and the Airway check get the base.
#[tokio::test]
async fn only_registered_roots_get_the_preview_platform() {
    let fx = world::world(write_then_format("clickhouse", "INSERT INTO t SELECT 1")).await;
    enable_runs(true);
    let root_dir = tempfile::tempdir().unwrap();
    let base: Arc<dyn PlatformContext> = base_ctx(&fx, root_dir.path()).await;
    let resolver = PreviewRunResolver::new(fx.db.clone());
    let get = |id: String| {
        let db = fx.db.clone();
        async move {
            agentic_runtime::crud::get_run(&db, &id)
                .await
                .unwrap()
                .expect("run")
        }
    };

    let (_, preview) = submit(&fx, BRANCH, PROCEDURE).await;
    let preview_id = preview["run_id"].as_str().unwrap().to_string();
    let platform = resolver
        .platform_for(&get(preview_id.clone()).await, base.clone())
        .await
        .unwrap();
    let scope = platform
        .preview_scope()
        .expect("a registered procedure run is a preview");
    assert_eq!(
        (scope.run_id.as_str(), scope.revision_id),
        (preview_id.as_str(), fx.staging)
    );
    assert_eq!(platform.compiled_revision(), Some(fx.staging));

    let ordinary = Uuid::new_v4().to_string();
    agentic_runtime::crud::insert_run(&fx.db, &ordinary, "q", None, "workflow", None, fx.ws)
        .await
        .unwrap();
    let platform = resolver
        .platform_for(&get(ordinary).await, base.clone())
        .await
        .unwrap();
    assert!(
        platform.preview_scope().is_none(),
        "an unregistered root is driven as production"
    );
    assert_eq!(platform.compiled_revision(), base.compiled_revision());

    let check =
        oxy_app::server::previews::analyze::ensure_enqueued(&fx.db, fx.ws, BRANCH, fx.staging)
            .await
            .unwrap()
            .unwrap();
    let platform = resolver
        .platform_for(&get(check).await, base.clone())
        .await
        .unwrap();
    assert!(
        platform.preview_scope().is_none(),
        "the Airway check reads through its own connection"
    );
}
