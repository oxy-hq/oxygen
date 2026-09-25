//! Read side of the custom-app **Oxy Functions** admin surface: list an app's
//! functions + their manifest config, their recent invocation history, and a
//! single function-job run's status + persisted logs. Powers the AppDetail
//! "Functions" section (manage / debug). The write side — triggering a job — is
//! `handlers::run_function_job` (`POST .../functions/{name}/runs`).
//!
//! All handlers are admin-gated at the router layer and DB-only (FleetOk): they
//! read `app_functions` (the per-build registry), `app_function_invocations`
//! (the invocation audit), and — for a job run — `agentic_runs` +
//! `agentic_run_events` (the `function_log` lines Slice A persists).
//! See internal-docs/customer-apps-functions.md.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::prelude::{AppFunctionInvocations, AppFunctions};
use entity::{app_function_invocations, app_functions, apps};
use oxy::database::client::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

/// Recent-invocation window returned by `list_invocations`. Bounded so the audit
/// table (which accumulates across builds) can't return an unbounded payload.
const INVOCATION_LIMIT: u64 = 50;

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// One function in the app's active build, projected from its manifest.
#[derive(Debug, Serialize)]
pub struct FunctionSummary {
    pub name: String,
    /// Whether the function is HTTP-invocable as the runtime serves it — true
    /// unless the manifest sets `route: false` (even a scheduled function is
    /// callable unless it opts out).
    pub route: bool,
    /// Cron expression when the function declares a schedule.
    pub schedule: Option<String>,
    pub timezone: Option<String>,
    /// The function is wired as an Airway pipeline transform step.
    pub airway: bool,
    pub timeout_seconds: Option<u32>,
    /// Marked "check": true — run by `oxyc checks run`.
    pub check: bool,
    /// Background-run retry policy, when declared (`maxAttempts > 1`).
    pub retries: Option<RetriesSummary>,
    /// The function may write app-scoped secrets via `ctx.secrets.set`.
    pub secrets_write: bool,
    /// Databases the function may write to via `ctx.warehouse`.
    pub destinations: Vec<String>,
    /// Author-declared example input (manifest `inputExample`) — a sample JSON
    /// body the "Run now" surface prefills so an operator knows what params the
    /// function expects. `None` when the function declares none.
    pub input_example: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct RetriesSummary {
    pub max_attempts: u32,
    pub min_timeout_ms: Option<u64>,
    pub max_timeout_ms: Option<u64>,
}

/// One recorded invocation (route / schedule / airway / manual job).
#[derive(Debug, Serialize)]
pub struct InvocationSummary {
    pub id: Uuid,
    /// `"route"` (HTTP) | `"schedule"` (cron fire) | `"manual"` (run-now / API
    /// job) | `"airway"`.
    pub mode: String,
    /// `"running"` | `"success"` | `"error"` | `"cancelled"` | `"timeout"` |
    /// `"shed"` (the platform declined to start it — no concurrency permit).
    /// The recorded outcome, verbatim: `success` means only that the handler
    /// returned without throwing, so read `failed` for whether it worked.
    pub status: String,
    /// The platform counted this invocation as a failure — see
    /// `counted_as_failure`. True for a `success` that answered 5xx or caught
    /// a failed `ctx.*` call, which `status` alone reports as a success.
    pub failed: bool,
    /// The HTTP status the function answered. Stored only beside a kept
    /// response body (keyed route calls), so usually `None`.
    pub result_status: Option<i16>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
    /// A stored response body is available (kept for keyed route calls).
    pub has_result: bool,
}

/// Whether the platform counted an invocation as a failure: the rule
/// `custom_apps_functions::failure_signal::Failure::of` applies at
/// finalization, read back from the row it finalized. Every invocation that
/// rule counts gets a `failure_fingerprint` — including a `success` that
/// answered 5xx or caught a failed host call — so the fingerprint is the
/// signal; `error` / `timeout` also count on their own, for rows written
/// before the column existed. `result_status` can't stand in for it: it is
/// only stored for keyed calls. `cancelled`, `shed` and `running` are not
/// failures.
fn counted_as_failure(status: &str, failure_fingerprint: Option<&str>) -> bool {
    failure_fingerprint.is_some() || matches!(status, "error" | "timeout")
}

/// A single function-job run's status + persisted logs, for the trigger-and-watch
/// debug loop. Reads the run row + its `function_log` events.
#[derive(Debug, Serialize)]
pub struct FunctionRunDetail {
    pub run_id: String,
    /// Effective status: `queued` (enqueued, not yet claimed by a worker) |
    /// `running` (a worker is executing it) | `done` | `failed` | `cancelled` |
    /// `timed_out`.
    pub status: Option<String>,
    /// `scheduled` | `manual`, from the run metadata.
    pub trigger: Option<String>,
    /// The function's return body on success.
    pub answer: Option<String>,
    pub error: Option<String>,
    pub logs: Vec<FunctionRunLogLine>,
}

#[derive(Debug, Serialize)]
pub struct FunctionRunLogLine {
    /// The event sequence number — a stable, unique id per run (a React key).
    pub seq: i64,
    pub level: String,
    pub message: String,
}

// ── Manifest projection (read-only) ──────────────────────────────────────────
//
// A decoupled view of the per-function `manifest_json` — just the fields the
// admin surface displays. The authoritative parse lives in
// `custom_apps_functions`; this projection keeps the admin module independent
// of the runtime host.

#[derive(Debug, serde::Deserialize, Default)]
struct ManifestView {
    #[serde(default)]
    route: Option<bool>,
    #[serde(default)]
    schedule: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default, rename = "timeoutSeconds")]
    timeout_seconds: Option<u32>,
    #[serde(default)]
    check: Option<bool>,
    #[serde(default, rename = "airwayStep")]
    airway_step: Option<serde_json::Value>,
    #[serde(default)]
    retries: Option<RetriesView>,
    #[serde(default)]
    secrets: Option<SecretsView>,
    #[serde(default)]
    destinations: Option<Vec<String>>,
    #[serde(default, rename = "inputExample")]
    input_example: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize, Default)]
struct RetriesView {
    #[serde(rename = "maxAttempts")]
    max_attempts: Option<u32>,
    #[serde(rename = "minTimeoutMs")]
    min_timeout_ms: Option<u64>,
    #[serde(rename = "maxTimeoutMs")]
    max_timeout_ms: Option<u64>,
}

#[derive(Debug, serde::Deserialize, Default)]
struct SecretsView {
    #[serde(default)]
    write: Option<bool>,
}

fn to_summary(name: String, manifest: Option<&serde_json::Value>) -> FunctionSummary {
    let m: ManifestView = manifest
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let has_airway = m.airway_step.is_some();
    // Reflect what the runtime actually serves, not just the author's intent: the
    // `/fn/<name>` handler rejects only an explicit `route: false`, so a function
    // is HTTP-invocable unless it opts out — even a schedule-only one. Showing the
    // badge on that case is the honest signal for a debug surface (the operator
    // sees the function is publicly callable). Distinct from the SDK validator's
    // "effective route" (intent), which defaults off when another surface exists.
    let route = m.route != Some(false);
    let retries = m.retries.and_then(|r| {
        r.max_attempts.filter(|&a| a > 1).map(|a| RetriesSummary {
            max_attempts: a,
            min_timeout_ms: r.min_timeout_ms,
            max_timeout_ms: r.max_timeout_ms,
        })
    });
    FunctionSummary {
        name,
        route,
        schedule: m.schedule,
        timezone: m.timezone,
        airway: has_airway,
        timeout_seconds: m.timeout_seconds,
        check: m.check.unwrap_or(false),
        retries,
        secrets_write: m.secrets.and_then(|s| s.write).unwrap_or(false),
        destinations: m.destinations.unwrap_or_default(),
        input_example: m.input_example,
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /admin/apps/{id}/functions` — the app's functions in its active build
/// (published, else draft), with each one's manifest config. Empty when the app
/// has no build or ships no functions.
pub async fn list_functions(
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<FunctionSummary>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("list_functions DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let app = apps::Entity::find_by_id(id)
        .one(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let Some(build_id) = app.published_build_id.or(app.draft_build_id) else {
        return Ok(Json(vec![]));
    };
    let rows = AppFunctions::find()
        .filter(app_functions::Column::BuildId.eq(build_id))
        .order_by_asc(app_functions::Column::Name)
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("list_functions query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let out = rows
        .into_iter()
        .map(|r| to_summary(r.name, r.manifest_json.as_ref()))
        .collect();
    Ok(Json(out))
}

/// `GET /admin/apps/{id}/functions/{name}/invocations` — the most recent
/// invocations of one function across all its builds (newest first), for the
/// debug history table.
pub async fn list_invocations(
    Path((id, name)): Path<(Uuid, String)>,
) -> Result<Json<Vec<InvocationSummary>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("list_invocations DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // `find()` selects every column, so `failure_fingerprint` and
    // `result_status` are on each row for `failed` / `result_status` below.
    let rows = AppFunctionInvocations::find()
        .filter(app_function_invocations::Column::AppId.eq(id))
        .filter(app_function_invocations::Column::FunctionName.eq(name))
        .order_by_desc(app_function_invocations::Column::CreatedAt)
        .limit(INVOCATION_LIMIT)
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("list_invocations query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let out = rows
        .into_iter()
        .map(|r| InvocationSummary {
            id: r.id,
            mode: r.mode,
            failed: counted_as_failure(&r.status, r.failure_fingerprint.as_deref()),
            status: r.status,
            result_status: r.result_status,
            duration_ms: r.duration_ms,
            error: r.error,
            created_at: r.created_at.to_rfc3339(),
            has_result: r.result_body.is_some(),
        })
        .collect();
    Ok(Json(out))
}

/// Whether a run is an `app_function` run seeded for app `id`. Both the scheduled
/// and manual seeds set `question = "fn:<app_id>/<name>"`, and `<app_id>` is a
/// fixed-length UUID followed by `/`, so the prefix can't collide across apps.
/// The security guard for the app-scoped run-detail endpoint.
fn run_belongs_to_app(source_type: Option<&str>, question: &str, id: Uuid) -> bool {
    source_type == Some("app_function") && question.starts_with(&format!("fn:{id}/"))
}

/// The run's *effective* execution state for the UI. `insert_run` stamps
/// `task_status="running"` at enqueue time — before any worker claims the task —
/// so a run that is only *queued* would otherwise read as "running" (a silent
/// spinner if no worker is draining the queue). Consult the queue: a still-
/// `queued` task reports `queued` (waiting for a worker), a `dead` task (retries
/// exhausted) reports `failed`; a terminal run status always wins.
fn effective_run_status(run_status: Option<&str>, queue_status: Option<&str>) -> String {
    if let Some(s) = run_status
        && matches!(s, "done" | "failed" | "cancelled" | "timed_out")
    {
        return s.to_string();
    }
    match queue_status {
        Some("queued") => "queued".to_string(),
        Some("dead") => "failed".to_string(),
        _ => run_status.unwrap_or("running").to_string(),
    }
}

/// `GET /admin/apps/{id}/function-runs/{run_id}` — a single function-job run's
/// status + persisted `function_log` output, for watching a just-triggered run.
/// Verifies the run is an `app_function` run for this app before returning
/// anything, so the app-scoped path can't read an unrelated run.
pub async fn get_function_run(
    Path((id, run_id)): Path<(Uuid, String)>,
) -> Result<Json<FunctionRunDetail>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("get_function_run DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let run = agentic_runtime::crud::get_run(&db, &run_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // Ownership guard: the run must be an app_function run seeded for THIS app,
    // so the app-scoped path can't read another app's (or any non-function) run.
    if !run_belongs_to_app(run.source_type.as_deref(), &run.question, id) {
        return Err(StatusCode::NOT_FOUND);
    }
    // Report queued-vs-running honestly: a background job is only executing once
    // a worker has claimed its queue task. Otherwise a run sitting in the queue
    // (e.g. no global worker draining it) reads as a perpetual "running" spinner.
    let queue_status = agentic_runtime::crud::get_queue_entry(&db, &run_id)
        .await
        .ok()
        .flatten()
        .map(|q| q.queue_status);
    let status = effective_run_status(run.task_status.as_deref(), queue_status.as_deref());
    let trigger = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("trigger"))
        .and_then(|t| t.as_str())
        .map(str::to_string);
    let logs = agentic_runtime::crud::get_all_events(&db, &run_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .filter(|e| e.event_type == "function_log")
        .map(|e| FunctionRunLogLine {
            seq: e.seq,
            level: e
                .payload
                .get("level")
                .and_then(|v| v.as_str())
                .unwrap_or("log")
                .to_string(),
            message: e
                .payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        })
        .collect();
    Ok(Json(FunctionRunDetail {
        run_id,
        status: Some(status),
        trigger,
        answer: run.answer,
        error: run.error_message,
        logs,
    }))
}

#[cfg(test)]
mod tests {
    use super::{counted_as_failure, effective_run_status, run_belongs_to_app, to_summary};
    use serde_json::json;
    use uuid::Uuid;

    /// The Sep 2026 warehouse incident: a function that caught every refused
    /// write and answered its own 500 was recorded `success`, and the history
    /// showed a week of green. `failed` must agree with the pager instead.
    #[test]
    fn failed_mirrors_what_the_pager_counted() {
        // A clean success is not a failure.
        assert!(!counted_as_failure("success", None));
        // A success the pager fingerprinted (answered 5xx, or caught a failed
        // ctx call) is — the case `status` alone reports as green.
        assert!(counted_as_failure("success", Some("af597dfd3a7536b8")));
        // Errors and timeouts count with or without a fingerprint (rows from
        // before the column carry none).
        assert!(counted_as_failure("error", None));
        assert!(counted_as_failure("error", Some("af597dfd3a7536b8")));
        assert!(counted_as_failure("timeout", None));
        // Cancelled, shed and in-flight invocations are not failures.
        assert!(!counted_as_failure("cancelled", None));
        assert!(!counted_as_failure("shed", None));
        assert!(!counted_as_failure("running", None));
    }

    #[test]
    fn effective_status_reports_queued_vs_running() {
        // Freshly enqueued: run stamped "running" but the queue task is still
        // queued (no worker claimed it) → report "queued", not a false "running".
        assert_eq!(
            effective_run_status(Some("running"), Some("queued")),
            "queued"
        );
        // Claimed by a worker → genuinely running.
        assert_eq!(
            effective_run_status(Some("running"), Some("claimed")),
            "running"
        );
        // A terminal run status always wins over the queue.
        assert_eq!(
            effective_run_status(Some("done"), Some("completed")),
            "done"
        );
        assert_eq!(effective_run_status(Some("failed"), None), "failed");
        assert_eq!(effective_run_status(Some("timed_out"), None), "timed_out");
        // Dead-lettered (retries exhausted) surfaces as failed.
        assert_eq!(
            effective_run_status(Some("running"), Some("dead")),
            "failed"
        );
        // No queue row (already pruned) falls back to the run status.
        assert_eq!(effective_run_status(Some("running"), None), "running");
    }

    #[test]
    fn ownership_guard_matches_only_this_apps_function_runs() {
        let app = Uuid::new_v4();
        let other = Uuid::new_v4();

        // A function run seeded for this app → owned.
        assert!(run_belongs_to_app(
            Some("app_function"),
            &format!("fn:{app}/refresh-token"),
            app
        ));
        // Another app's function run → rejected (the cross-app read guard).
        assert!(!run_belongs_to_app(
            Some("app_function"),
            &format!("fn:{other}/refresh-token"),
            app
        ));
        // Right question shape but wrong source_type → rejected.
        assert!(!run_belongs_to_app(
            Some("workflow"),
            &format!("fn:{app}/refresh-token"),
            app
        ));
        // Non-function run (e.g. an agent run) → rejected.
        assert!(!run_belongs_to_app(
            Some("app_function"),
            "some agent question",
            app
        ));
        assert!(!run_belongs_to_app(None, &format!("fn:{app}/x"), app));
    }

    #[test]
    fn summary_projects_manifest_surfaces_and_retries() {
        // Route defaults on when no other surface is declared.
        let s = to_summary("plain".into(), Some(&json!({})));
        assert!(s.route);
        assert!(s.schedule.is_none());
        assert!(!s.airway);

        // A schedule-only function is still route-servable (no explicit
        // route:false) — the badge reflects actual runtime invocability — and its
        // schedule / retries / secrets are all projected.
        let s = to_summary(
            "cron".into(),
            Some(&json!({
                "schedule": "*/50 * * * *",
                "timezone": "UTC",
                "retries": { "maxAttempts": 3, "minTimeoutMs": 1000, "maxTimeoutMs": 30000 },
                "secrets": { "write": true }
            })),
        );
        assert!(s.route);
        assert_eq!(s.schedule.as_deref(), Some("*/50 * * * *"));
        assert!(s.secrets_write);
        assert_eq!(s.retries.expect("retries projected").max_attempts, 3);

        // Explicit opt-out → not route-servable.
        let opted_out = to_summary(
            "no-route".into(),
            Some(&json!({ "route": false, "schedule": "0 * * * *" })),
        );
        assert!(!opted_out.route);

        // maxAttempts <= 1 is not a retry policy.
        let s = to_summary(
            "once".into(),
            Some(&json!({ "retries": { "maxAttempts": 1 } })),
        );
        assert!(s.retries.is_none());
    }

    #[test]
    fn summary_projects_the_check_flag() {
        let with = json!({ "check": true, "schedule": "*/15 * * * *" });
        assert!(to_summary("smoke".into(), Some(&with)).check);
        let without = json!({ "route": true });
        assert!(!to_summary("echo".into(), Some(&without)).check);
        assert!(!to_summary("bare".into(), None).check);
    }
}
