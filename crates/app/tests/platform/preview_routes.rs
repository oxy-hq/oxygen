//! The previews checks API (`GET /previews/checks?branch=`, and `checks` on the
//! list item) end to end: staff only, pending until the Airway change check of
//! the preview's current revision has run, and then that check's report — here
//! produced by the real `preview_analyze` executor over compiled
//! `airway_pipelines` rows of two revisions.
//!
//! Database-backed (`Schema::All`: the check's run and task live in the runtime
//! tables, the stored schema in Airway's). No Airhouse is configured, which is
//! itself asserted: an unreachable Airhouse is a warning on the pipeline, never
//! a failed check.

/// Shared with `preview_runs`, which previews the same branch.
pub(crate) mod fixture;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use axum::http::StatusCode;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{Value, json};

use fixture::{BRANCH, Fx, get_json, setup};
use oxy_app::server::previews::analyze::{PREVIEW_ANALYZE_KIND, PreviewAnalyzeExecutor};

async fn checks(fx: &Fx) -> Value {
    let (status, body) = get_json(
        &fx.staff,
        format!("/{}/previews/checks?branch=feat%2Fje-v2", fx.ws),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn listed_checks(fx: &Fx) -> Value {
    let (status, body) = get_json(&fx.staff, format!("/{}/previews", fx.ws)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"][0]["checks"].clone()
}

/// What the coordinator does with a root task's outcome, done by hand: run the
/// executor and record its `Done` on the run row.
async fn run_the_check(db: &DatabaseConnection, run_id: &str) {
    let exec = PreviewAnalyzeExecutor::airhouse(db.clone());
    let mut task = exec
        .execute(TaskAssignment {
            task_id: run_id.into(),
            parent_task_id: None,
            run_id: run_id.into(),
            spec: TaskSpec::Custom {
                kind: PREVIEW_ANALYZE_KIND.into(),
                payload: json!({ "preview_run_id": run_id }),
            },
            policy: None,
        })
        .await
        .expect("the executor accepts its own kind");
    let outcome = task.outcomes.recv().await.expect("an outcome");
    let TaskOutcome::Done { answer, metadata } = outcome else {
        panic!(
            "an unreachable Airhouse and an unbuildable source are findings, not a failed check: {outcome:?}"
        );
    };
    agentic_runtime::crud::update_run_done(db, run_id, &answer, metadata)
        .await
        .expect("record the outcome");
}

#[tokio::test]
async fn checks_returns_the_latest_outcome() {
    let fx = setup().await;

    // Nothing queued yet for the preview's revision: pending, no pipelines.
    let body = checks(&fx).await;
    assert_eq!(body["branch"], BRANCH);
    assert_eq!(body["revision_id"], fx.staging.to_string());
    assert_eq!(body["status"], "pending");
    assert_eq!(body["error"], Value::Null);
    assert_eq!(body["pipelines"], json!([]));
    assert_eq!(
        listed_checks(&fx).await,
        Value::Null,
        "no check for this revision yet"
    );

    let run_id =
        oxy_app::server::previews::analyze::ensure_enqueued(&fx.db, fx.ws, BRANCH, fx.staging)
            .await
            .unwrap()
            .expect("queued");
    assert_eq!(checks(&fx).await["status"], "pending");
    assert_eq!(
        listed_checks(&fx).await,
        json!({ "status": "pending", "needs_reset": 0, "warnings": 0, "transforms": 0 })
    );

    run_the_check(&fx.db, &run_id).await;

    let body = checks(&fx).await;
    assert_eq!(body["status"], "done", "{body}");
    assert_eq!(body["error"], Value::Null);
    let pipelines = body["pipelines"].as_array().unwrap();
    assert_eq!(pipelines.len(), 2, "{body}");

    let edited = &pipelines[0];
    assert_eq!(edited["name"], "nces_schools");
    assert_eq!(edited["file_path"], "airway/nces.airway.yml");
    assert_eq!(edited["change"], "modified");
    assert_eq!(edited["verdict"], "needs_reset");
    let findings = edited["findings"].as_array().unwrap();
    assert!(
        findings.contains(&json!({
            "kind": "WriteDispositionChanged",
            "verdict": "needs_reset",
            "detail": "schools: replace → merge",
            "prod_action": "Reset schema, then backfill",
        })),
        "{findings:?}"
    );
    // The live tables were not readable (no Airhouse here): said, not skipped.
    assert!(
        findings.iter().any(|f| f["kind"] == "Unevaluated"
            && f["detail"]
                .as_str()
                .unwrap()
                .contains("drift was not checked")),
        "{findings:?}"
    );

    let added = &pipelines[1];
    assert_eq!(added["name"], "mystery");
    assert_eq!(added["change"], "added");
    assert_eq!(added["verdict"], "warning");
    assert_eq!(added["findings"][0]["kind"], "Unevaluated");

    assert_eq!(
        listed_checks(&fx).await,
        json!({ "status": "done", "needs_reset": 1, "warnings": 1, "transforms": 0 })
    );
    let row = entity::workspace_preview_runs::Entity::find_by_id(run_id.clone())
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "finished");
    assert!(row.started_at.is_some() && row.finished_at.is_some());
}

#[tokio::test]
async fn checks_is_staff_only_and_404s_without_a_preview() {
    let fx = setup().await;
    let (status, _) = get_json(
        &fx.customer,
        format!("/{}/previews/checks?branch=feat%2Fje-v2", fx.ws),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an org admin is not staff");

    let (status, body) = get_json(
        &fx.staff,
        format!("/{}/previews/checks?branch=feat%2Fnope", fx.ws),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "preview_not_found");
}

/// A branch whose staging compile failed has no revision to check and never
/// will at that commit: failed, with the compile's error, not pending forever.
#[tokio::test]
async fn a_preview_whose_compile_failed_reads_as_failed_with_the_compile_error() {
    let fx = setup().await;
    fixture::exec(
        &fx.db,
        "UPDATE revisions SET status = 'failed', error_summary = $2 WHERE revision_id = $1",
        vec![
            fx.staging.into(),
            json!({ "fatal": "airway/nces.airway.yml: bad yaml" }).into(),
        ],
    )
    .await;
    let body = checks(&fx).await;
    assert_eq!(body["status"], "failed", "{body}");
    assert_eq!(body["revision_id"], Value::Null);
    assert!(
        body["error"].as_str().unwrap().contains("bad yaml"),
        "the compile's own error: {body}"
    );
    assert_eq!(body["pipelines"], json!([]));
}

/// A failed check reads as failed, with its error, and counts nothing.
#[tokio::test]
async fn a_failed_check_reads_as_failed_with_its_error() {
    let fx = setup().await;
    let run_id =
        oxy_app::server::previews::analyze::ensure_enqueued(&fx.db, fx.ws, BRANCH, fx.staging)
            .await
            .unwrap()
            .unwrap();
    agentic_runtime::crud::update_run_failed(&fx.db, &run_id, "database went away")
        .await
        .unwrap();
    let body = checks(&fx).await;
    assert_eq!(body["status"], "failed");
    assert_eq!(body["error"], "database went away");
    assert_eq!(body["pipelines"], json!([]));
    assert_eq!(
        listed_checks(&fx).await,
        json!({ "status": "failed", "needs_reset": 0, "warnings": 0, "transforms": 0 })
    );
}

/// I8: the held-runs routes are staff only — an org admin gets 403 on every
/// one, with the flag on, so the refusal is the guard's and not the flag's.
#[tokio::test]
async fn non_staff_get_403() {
    let fx = setup().await;
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_PREVIEW_RUNS", "1") };
    let body = json!({ "branch": BRANCH, "kind": "procedure", "ref": "workflows/x.procedure.yml" });
    for (method, uri, body) in [
        ("POST", format!("/{}/previews/runs", fx.ws), Some(body)),
        (
            "GET",
            format!("/{}/previews/runs?branch=feat%2Fje-v2", fx.ws),
            None,
        ),
        (
            "GET",
            format!("/{}/previews/runs/{}", fx.ws, uuid::Uuid::new_v4()),
            None,
        ),
    ] {
        let (status, resp) =
            fixture::send_json(&fx.customer, method, uri.clone(), body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {resp}");
        // The control: staff pass the guard (and then meet the handler's own answers).
        let (status, resp) = fixture::send_json(&fx.staff, method, uri.clone(), body).await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{method} {uri}: {resp}");
    }
}
