//! The fences past the obvious ones: an old pod handed a preview's HTTP write
//! cannot send it (I6), a root that says it is a preview but has no registry
//! row is never driven as production, and a run that never ends cannot hold a
//! workspace's queue forever.

use std::sync::Arc;

use agentic_automation::preview_names::scoped;
use agentic_pipeline::platform::{PlatformContext, RunPlatformResolver};
use axum::http::StatusCode;
use oxy_app::server::previews::runtime::PreviewRunResolver;
use serde_json::{Value, json};
use uuid::Uuid;

use super::world::{self, PROCEDURE, base_ctx, enable_runs, submit};
use crate::preview_routes::fixture::{BRANCH, exec, send_json};

async fn run_row(
    fx: &crate::preview_routes::fixture::Fx,
    id: &str,
) -> agentic_runtime::entity::run::Model {
    agentic_runtime::crud::get_run(&fx.db, id)
        .await
        .unwrap()
        .expect("run row")
}

/// I6 for `http_request`: the step a preview serves carries a scoped method
/// and a scoped secret name. The production platform — what a pod without
/// preview code runs it with after a crash — refuses it at the method, before
/// any request exists; the preview platform holds it and reports the verb.
#[tokio::test]
async fn a_base_platform_cannot_send_a_held_http_write() {
    let rotate = json!([{
        "name": "rotate", "type": "http_request", "method": "post",
        "url": "https://oauth.example.invalid/token",
        "persist_to_secret": { "from": "/refresh_token", "name": "QB_REFRESH_TOKEN" }
    }]);
    let fx = world::world(rotate).await;
    enable_runs(true);
    let (_, submitted) = submit(&fx, BRANCH, PROCEDURE).await;
    let run_id = submitted["run_id"].as_str().unwrap().to_string();
    let dir = tempfile::tempdir().unwrap();
    let base = base_ctx(&fx, dir.path()).await;
    let base_platform: Arc<dyn PlatformContext> = base.clone();
    let preview = PreviewRunResolver::new(fx.db.clone())
        .platform_for(&run_row(&fx, &run_id).await, base_platform)
        .await
        .expect("the preview platform");

    let yaml = preview
        .resolve_automation_yaml(&scoped(&run_id, PROCEDURE))
        .await
        .expect("served");
    let definition: Value = serde_yaml::from_str(&yaml).unwrap();
    let step = definition["tasks"][0].clone();

    // The behaviour first: the old pod gets an error from the request
    // builder, never from a send (the host would not resolve).
    let err =
        agentic_automation::run_automation_step(base.as_ref(), step.clone(), json!({}), json!({}))
            .await
            .expect_err("an old pod must not send it");
    assert!(
        err.contains("invalid method"),
        "refused before sending, not at the send: {err}"
    );
    assert_eq!(step["method"], scoped(&run_id, "POST"), "{yaml}");
    assert_eq!(
        step["persist_to_secret"]["name"],
        scoped(&run_id, "QB_REFRESH_TOKEN")
    );

    let held =
        agentic_automation::run_automation_step(preview.as_ref(), step, json!({}), json!({}))
            .await
            .expect("the preview platform holds it");
    let held: Value = serde_json::from_str(&held).unwrap();
    assert_eq!(held["preview"]["held"], true);
    assert_eq!(held["preview"]["method"], "POST");
}

/// A root with no registry row that nonetheless says it is a preview's —
/// by its trigger, or by a scoped `workflow_ref` — is retired, never handed
/// the production platform. The control, an ordinary root, still is.
#[tokio::test]
async fn a_preview_marked_root_without_a_registry_row_is_retired() {
    let fx = world::world(json!([])).await;
    let dir = tempfile::tempdir().unwrap();
    let base: Arc<dyn PlatformContext> = base_ctx(&fx, dir.path()).await;
    let resolver = PreviewRunResolver::new(fx.db.clone());
    let marked = [
        json!({ "trigger": "preview", "workflow_ref": PROCEDURE }),
        json!({ "workflow_ref": scoped("gone-run", PROCEDURE) }),
    ];
    for metadata in marked {
        let id = Uuid::new_v4().to_string();
        agentic_runtime::crud::insert_run(
            &fx.db,
            &id,
            "q",
            None,
            "workflow",
            Some(metadata.clone()),
            fx.ws,
        )
        .await
        .unwrap();
        let root = run_row(&fx, &id).await;
        assert!(
            resolver.platform_for(&root, base.clone()).await.is_err(),
            "{metadata}"
        );
        assert_eq!(
            run_row(&fx, &id).await.task_status.as_deref(),
            Some("failed"),
            "{metadata}"
        );
    }
    let ordinary = Uuid::new_v4().to_string();
    agentic_runtime::crud::insert_run(
        &fx.db,
        &ordinary,
        "q",
        None,
        "workflow",
        Some(json!({ "workflow_ref": PROCEDURE })),
        fx.ws,
    )
    .await
    .unwrap();
    let platform = resolver
        .platform_for(&run_row(&fx, &ordinary).await, base)
        .await
        .unwrap();
    assert!(
        platform.preview_scope().is_none(),
        "an ordinary root is driven as production"
    );
}

/// A run `running` past `OXY_PREVIEW_RUN_MAX_MINUTES` (default 60) is retired
/// with the reason and finished, and the workspace's next run starts. A run
/// inside the ceiling is left alone.
#[tokio::test]
async fn an_overdue_run_is_retired_and_the_queue_moves_on() {
    let fx = world::world(json!([])).await;
    enable_runs(true);
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var(oxy_app::server::previews::runs::MAX_MINUTES_ENV) };
    let (_, first) = submit(&fx, BRANCH, PROCEDURE).await;
    let (_, second) = submit(&fx, BRANCH, PROCEDURE).await;
    let (a, b) = (
        first["run_id"].as_str().unwrap(),
        second["run_id"].as_str().unwrap(),
    );
    assert_eq!(
        (first["state"].as_str(), second["state"].as_str()),
        (Some("running"), Some("queued"))
    );

    oxy_app::server::previews::runs::sweep(&fx.db).await;
    assert_eq!(
        run_row(&fx, a).await.task_status.as_deref(),
        Some("running"),
        "inside the ceiling"
    );

    exec(
        &fx.db,
        "UPDATE workspace_preview_runs SET started_at = now() - interval '61 minutes' WHERE run_id = $1",
        vec![a.into()],
    )
    .await;
    oxy_app::server::previews::runs::sweep(&fx.db).await;

    let retired = run_row(&fx, a).await;
    assert_eq!(retired.task_status.as_deref(), Some("failed"));
    let (status, detail) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/runs/{a}", fx.ws),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(
        (detail["state"].as_str(), detail["outcome"].as_str()),
        (Some("finished"), Some("failed"))
    );
    assert!(
        detail["error"]
            .as_str()
            .unwrap_or_default()
            .contains("60-minute ceiling"),
        "{detail}"
    );
    let (_, next) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/runs/{b}", fx.ws),
        None,
    )
    .await;
    assert_eq!(next["state"], "running", "the queue moved on: {next}");
}
