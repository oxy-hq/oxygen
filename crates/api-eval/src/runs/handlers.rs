//! The two eval-run routes. Thin: check the request, read the served
//! revision, queue or read back a run. The work is
//! [`super::executor::EvalRunExecutor`]'s.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use oxy::config::test_config::TestFileConfig;
use oxy::config::{ArtifactError, ConfigManager, DiskSlot, EvalTest, EvalTests};
use oxy_app::surface::{EffectiveWorkspaceRole, WorkspaceManagerReadOnly};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseConnection, EntityTrait, TransactionTrait};
use serde::Serialize;
use uuid::Uuid;

use super::executor::{EVAL_RUN_KIND, EVAL_RUN_SOURCE};
use super::request::{CreateRunRequest, EvalRunPayload, resolve_case_index, validate};

type ApiError = (StatusCode, String);

#[derive(Debug, Serialize)]
pub(crate) struct CreatedRun {
    run_id: String,
    test_files: Vec<String>,
}

/// `POST /api/{workspace_id}/evals/runs` → `202 { run_id, test_files }`.
///
/// 400 a request `oxy test` would have refused, or a test file that does not
/// parse; 404 a test file the served revision does not have (a push compiles
/// it); 503 the compile boundary could not be read (retryable).
pub(crate) async fn create_run(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(_workspace_id): Path<Uuid>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    role: Option<EffectiveWorkspaceRole>,
    Json(req): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<CreatedRun>), ApiError> {
    validate(&req).map_err(bad_request)?;
    let (test_files, case_index) = resolve_tests(
        workspace_manager.workspace_id,
        &workspace_manager.config_manager,
        &req,
    )
    .await?;
    let payload = EvalRunPayload {
        workspace_id: workspace_manager.workspace_id,
        user_id: user.id,
        role: role.map(|r| r.0),
        test_files: test_files.clone(),
        case_index,
        tag: req.tag,
        regrade: req.regrade,
    };
    let run_id = enqueue(&connect().await?, &payload).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CreatedRun { run_id, test_files }),
    ))
}

/// The test files a request names, read from the served revision, and its
/// case as an index.
async fn resolve_tests<S: DiskSlot>(
    workspace_id: Uuid,
    manager: &ConfigManager<S>,
    req: &CreateRunRequest,
) -> Result<(Vec<String>, Option<usize>), ApiError> {
    let Some(file) = req.test_file.as_deref() else {
        let tests = match manager.list_eval_tests().await.map_err(boundary)? {
            EvalTests::Read(tests) => tests,
            EvalTests::NotCompiled => return Err(not_compiled(workspace_id).await),
        };
        let files: Vec<String> = tests
            .into_iter()
            .filter(|t| TestFileConfig::from_definition(&t.file_path, t.definition.clone()).is_ok())
            .map(|t| t.file_path)
            .collect();
        if files.is_empty() {
            return Err(not_found(
                "No .test.yml files in the served revision. Test files are compiled on push: \
                 push one"
                    .to_string(),
            ));
        }
        return Ok((files, None));
    };
    let entry = match manager.eval_test(file).await.map_err(boundary)? {
        EvalTest::Found(entry) => entry,
        EvalTest::Missing => {
            return Err(not_found(format!(
                "{file} is not in the served revision. Test files are compiled on push: push it"
            )));
        }
        EvalTest::NotCompiled => return Err(not_compiled(workspace_id).await),
    };
    let config =
        TestFileConfig::from_definition(&entry.file_path, entry.definition).map_err(bad_request)?;
    config
        .validate_grading()
        .map_err(|e| bad_request(format!("{}: {e}", entry.file_path)))?;
    let case_index = match req.case.as_deref() {
        Some(case) => {
            Some(resolve_case_index(&config, case, &entry.file_path).map_err(bad_request)?)
        }
        None => None,
    };
    Ok((vec![entry.file_path], case_index))
}

async fn enqueue(db: &DatabaseConnection, payload: &EvalRunPayload) -> Result<String, ApiError> {
    use agentic_core::delegation::TaskSpec;
    use agentic_runtime::crud::{TaskScope, enqueue_task};
    use agentic_runtime::lifecycle::crud::runs::insert_run;

    let run_id = Uuid::new_v4().to_string();
    let question = format!("evals: {}", payload.test_files.join(", "));
    let metadata = serde_json::json!({
        "test_files": payload.test_files,
        "case_index": payload.case_index,
        "tag": payload.tag,
        "regrade": payload.regrade.is_some(),
        "user_id": payload.user_id,
    });
    // One transaction: a row whose task never queued would sit `running`
    // forever, with nothing to finish it.
    let txn = db.begin().await.map_err(internal)?;
    insert_run(
        &txn,
        &run_id,
        &question,
        None,
        EVAL_RUN_SOURCE,
        Some(metadata),
        payload.workspace_id,
    )
    .await
    .map_err(internal)?;
    let spec = TaskSpec::Custom {
        kind: EVAL_RUN_KIND.into(),
        payload: serde_json::to_value(payload).map_err(internal)?,
    };
    // No policy: a retried eval would spend the suite's LLM calls twice.
    enqueue_task(&txn, &run_id, &run_id, None, &spec, None, TaskScope::Global)
        .await
        .map_err(internal)?;
    txn.commit().await.map_err(internal)?;
    Ok(run_id)
}

/// `GET /api/{workspace_id}/evals/runs/{run_id}`.
#[derive(Debug, Serialize)]
pub(crate) struct RunView {
    run_id: String,
    /// The run row's: `running`, `done`, `failed`, `cancelled` or `timed_out`.
    status: String,
    test_files: Vec<String>,
    error: Option<String>,
    /// One per gated metric, in result order: what a `--min-accuracy` gate
    /// reads (`EvalResult::gated_accuracies`). `None` until done.
    accuracies: Option<Vec<f64>>,
    /// The results document, parsed.
    results: Option<serde_json::Value>,
    /// The results document's exact text, the bytes `oxy test --output-json`
    /// wrote. Write this, not a re-encoding of `results`: a JSON round trip can
    /// turn `1.0` into `1`.
    results_json: Option<String>,
}

pub(crate) async fn get_run(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, run_id)): Path<(Uuid, String)>,
) -> Result<Json<RunView>, ApiError> {
    use agentic_runtime::lifecycle::entity::run;
    let row = run::Entity::find_by_id(run_id.clone())
        .one(&connect().await?)
        .await
        .map_err(internal)?
        // Run ids are global: a row of another workspace, or another kind of
        // run, is not found here.
        .filter(|r| {
            r.workspace_id == workspace_manager.workspace_id
                && r.source_type.as_deref() == Some(EVAL_RUN_SOURCE)
        })
        .ok_or_else(|| not_found(format!("no eval run {run_id} in this workspace")))?;
    Ok(Json(view_of(row)))
}

fn view_of(row: agentic_runtime::lifecycle::entity::run::Model) -> RunView {
    let test_files = row
        .metadata
        .as_ref()
        .and_then(|m| m.get("test_files"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let results_json = row
        .answer
        .filter(|_| row.task_status.as_deref() == Some("done"));
    let results: Option<serde_json::Value> = results_json
        .as_deref()
        .and_then(|text| serde_json::from_str(text).ok());
    RunView {
        run_id: row.id,
        status: row.task_status.unwrap_or_else(|| "running".to_string()),
        test_files,
        error: row.error_message,
        accuracies: results.as_ref().map(gated_accuracies),
        results,
        results_json,
    }
}

/// [`crate::builders::types::EvalResult::gated_accuracies`], read back off the
/// stored document: the same metric fields, in the same order.
fn gated_accuracies(results: &serde_json::Value) -> Vec<f64> {
    let metrics = results.as_array().into_iter().flatten().flat_map(|r| {
        r.get("metrics")
            .and_then(|m| m.as_array())
            .into_iter()
            .flatten()
    });
    metrics
        .filter_map(|m| match m.get("type").and_then(|t| t.as_str()) {
            Some("Similarity") | Some("Correctness") => m.get("score")?.as_f64(),
            Some("Execution") => m.get("answer_acc")?.as_f64(),
            _ => None,
        })
        .collect()
}

async fn connect() -> Result<DatabaseConnection, ApiError> {
    oxy::database::client::establish_connection()
        .await
        .map_err(internal)
}

/// The served revision predates test files as a compiled kind: retryable, and
/// a compile is queued so the next try finds them (`enqueue_lazy_compile`,
/// throttled per workspace).
async fn not_compiled(workspace_id: Uuid) -> ApiError {
    if let Ok(db) = connect().await {
        oxy_app::surface::enqueue_lazy_compile(&db, workspace_id).await;
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "not_compiled: the served revision was compiled before test files were; a compile is \
         queued, retry shortly"
            .to_string(),
    )
}

fn bad_request(message: String) -> ApiError {
    (StatusCode::BAD_REQUEST, message)
}

fn not_found(message: String) -> ApiError {
    (StatusCode::NOT_FOUND, message)
}

fn boundary(e: ArtifactError) -> ApiError {
    if e.retryable() {
        (StatusCode::SERVICE_UNAVAILABLE, e.to_string())
    } else {
        bad_request(e.to_string())
    }
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %e, "eval run route failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../builders/fixtures/results.json");

    fn row(status: &str, answer: Option<&str>) -> agentic_runtime::lifecycle::entity::run::Model {
        let now = chrono::Utc::now().fixed_offset();
        agentic_runtime::lifecycle::entity::run::Model {
            id: "run-9".into(),
            question: "evals: tests/a.test.yml".into(),
            answer: answer.map(str::to_string),
            error_message: (status == "failed").then(|| "No model configured".to_string()),
            thread_id: None,
            source_type: Some(EVAL_RUN_SOURCE.into()),
            metadata: Some(serde_json::json!({ "test_files": ["tests/a.test.yml"] })),
            parent_run_id: None,
            schedule_id: None,
            task_status: Some(status.into()),
            task_metadata: None,
            attempt: 0,
            recovery_requested_at: None,
            driver_id: None,
            driver_heartbeat_at: None,
            cancel_requested_at: None,
            workspace_id: Uuid::nil(),
            created_at: now,
            updated_at: now,
        }
    }

    /// The route hands back the stored document's exact text: the bytes the
    /// executor stored are the bytes `oxyc` writes.
    #[test]
    fn a_done_run_answers_its_results_text_verbatim() {
        let view = view_of(row("done", Some(FIXTURE)));
        assert_eq!(view.status, "done");
        assert_eq!(view.test_files, vec!["tests/a.test.yml".to_string()]);
        assert_eq!(view.results_json.as_deref(), Some(FIXTURE));
        assert_eq!(view.accuracies, Some(vec![0.5]));
        assert!(view.results.is_some());
        let wire = serde_json::to_value(&view).unwrap();
        assert_eq!(wire["results_json"].as_str(), Some(FIXTURE));
    }

    #[test]
    fn a_failed_or_running_run_carries_no_results() {
        let failed = view_of(row("failed", Some("partial")));
        assert_eq!(failed.error.as_deref(), Some("No model configured"));
        assert!(failed.results_json.is_none() && failed.accuracies.is_none());
        let running = view_of(row("running", None));
        assert_eq!(running.status, "running");
        assert!(running.results.is_none());
    }

    /// The route's gate reads the fields the engine's gate reads, off the
    /// pinned results document.
    #[test]
    fn the_stored_document_gates_like_the_engine() {
        let doc: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(gated_accuracies(&doc), vec![0.5]);
        let execution = serde_json::json!([{ "metrics": [
            { "type": "Execution", "answer_acc": 0.75, "ex": 1.0 },
            { "type": "Recall", "score": 0.1 }
        ]}]);
        assert_eq!(gated_accuracies(&execution), vec![0.75]);
    }
}
