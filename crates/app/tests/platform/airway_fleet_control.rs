//! Starting, cancelling and resetting an Airway pipeline from a replica that
//! holds no working copy — the Factory (`OXY_ROLE=ide`) being down must not
//! take manual control of ingestion with it.
//!
//! `agentic_http::airway_router_roles` declares these routes `FleetOk`. That is
//! a claim about the handlers, and this is where it is tested rather than
//! reasoned: the real router, over the real host adapter (`OxyProjectContext`)
//! pinned to a promoted revision, with the workspace directory **absent from
//! disk**. Anything in the path that reached for a file would fail here.
//!
//! What a replica answers when the compile boundary cannot supply the pipeline
//! is the other half: a retryable `503` naming its reason, never a `400` that
//! reads as bad input — plus a compile request, so the retry has something to
//! wait for.
//!
//! Database-backed through [`crate::common::fresh_db`] with every migrator
//! `oxy serve` runs; the seeding helpers are `airway_compile_boundary`'s.

use std::sync::Arc;

use agentic_http::AgenticState;
use agentic_http::routes::airway_not_servable::{CODE_NEEDS_RECOMPILE, HEADER_ERROR_CODE};
use agentic_pipeline::platform::{PlatformContext, ThreadOwnerLookup};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header::RETRY_AFTER};
use axum::{Extension, Router};
use entity::users::UserStatus;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

use super::airway_compile_boundary::{
    seed_pipeline, seed_promoted_workspace, worker_context_without_working_copy,
};

const SERVED_REF: &str = "pipelines/toast.airway.yml";
/// A ref the promoted revision has no row for — a pipeline that exists only in
/// someone's working copy, or not at all. A replica cannot tell which.
const UNSERVED_REF: &str = "pipelines/not_promoted.airway.yml";

/// Every migrator, and `OXY_DATABASE_URL` pointed at this test's database so
/// the compile-boundary reader inside the host adapter reaches it too.
async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::All).await;
    // SAFETY: single-threaded test setup; `fresh_db` asserts nextest's
    // process-per-test isolation before this runs.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    db
}

struct NoThreads;

#[async_trait]
impl ThreadOwnerLookup for NoThreads {
    async fn thread_owner(&self, _thread: Uuid) -> Result<Option<Option<Uuid>>, String> {
        Ok(None)
    }
}

/// One replica's airway surface: its own `AgenticState` (so its own, empty,
/// in-process channel maps) over the shared database.
fn replica(db: &DatabaseConnection, platform: Arc<dyn PlatformContext>) -> Router {
    let state = Arc::new(AgenticState::new(
        CancellationToken::new(),
        db.clone(),
        Arc::new(NoThreads),
    ));
    let user = AuthenticatedUser {
        id: Uuid::new_v4(),
        email: Some("operator@acme.test".into()),
        name: "Operator".into(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    };
    Router::new()
        .nest("/agentic-airway", agentic_http::airway_router(state))
        .layer(Extension(platform))
        .layer(Extension(user))
}

/// A promoted workspace serving [`SERVED_REF`], and the platform a request to a
/// diskless replica is handed for it: compiled origin, no directory, and the
/// database handle the workspace middleware attaches.
async fn diskless_workspace(db: &DatabaseConnection) -> (Uuid, Arc<dyn PlatformContext>) {
    let (ws_id, rev_id) = seed_promoted_workspace(db).await;
    seed_pipeline(db, rev_id, "toast_orders", SERVED_REF).await;
    let (ctx, absent_root) = worker_context_without_working_copy(db, ws_id).await;
    assert!(!absent_root.exists(), "precondition: no working copy");
    let platform: Arc<dyn PlatformContext> = Arc::new(ctx.with_db(Arc::new(db.clone())));
    (ws_id, platform)
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("json body ({e}): {}", self.body))
    }

    fn error_code(&self) -> Option<&str> {
        self.headers
            .get(HEADER_ERROR_CODE)
            .and_then(|v| v.to_str().ok())
    }

    /// The retryable-and-not-the-caller's-fault contract: `503`, the code, and
    /// when to ask again. Says nothing about whether a compile was asked for —
    /// the header is set for every `NotInRevision`; only the body says that.
    fn assert_not_served(&self, what: &str) {
        assert_eq!(
            self.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{what}: a ref this replica cannot serve is retryable, not a bad request: {}",
            self.body
        );
        assert_eq!(self.error_code(), Some(CODE_NEEDS_RECOMPILE), "{what}");
        assert!(
            self.headers.contains_key(RETRY_AFTER),
            "{what}: a 503 must say when to ask again"
        );
    }

    /// [`assert_not_served`](Self::assert_not_served), and the body says a
    /// compile was asked for — a mutating request outside the cooldown.
    fn assert_needs_recompile(&self, what: &str) {
        self.assert_not_served(what);
        assert!(
            self.body.contains(COMPILE_REQUESTED),
            "{what}: {}",
            self.body
        );
    }

    /// [`assert_not_served`](Self::assert_not_served), and the body does NOT
    /// claim a compile — a read, or a request inside the success cooldown.
    /// The claim would be false, and the `compile_tasks` count beside the
    /// caller is what proves no revision was bought.
    fn assert_not_served_without_a_compile_claim(&self, what: &str) {
        self.assert_not_served(what);
        assert!(
            !self.body.contains(COMPILE_REQUESTED),
            "{what}: no compile was requested, so the body must not say one was: {}",
            self.body
        );
    }
}

/// The sentence `agentic_pipeline::airway_request` appends when the host took
/// the compile request.
const COMPILE_REQUESTED: &str = "A compile has been requested";

/// A `main` compile of `workspace_id` that finished at `finished_at` with
/// `status`, as `oxy-compile` would leave it — the row the success cooldown
/// reads. It is not promoted (the fixture's revision stays current), so what
/// the boundary serves is unchanged; only the cooldown sees it.
///
/// The fixture's own revision is `kind: "full"`, a value the compiler never
/// writes, so it is invisible to a `kind = 'main'` read and the tests that
/// expect a compile request are not inside a cooldown by accident.
async fn seed_main_compile(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    status: &str,
    finished_at: chrono::DateTime<chrono::FixedOffset>,
) {
    entity::revisions::ActiveModel {
        revision_id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(workspace_id),
        git_sha: ActiveValue::Set(format!("local-{}", Uuid::new_v4())),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set(status.into()),
        kind: ActiveValue::Set("main".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(finished_at - chrono::Duration::seconds(5)),
        finished_at: ActiveValue::Set(Some(finished_at)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed main compile");
}

async fn call(router: &Router, method: &str, path: &str, body: Option<Value>) -> Answer {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(match body {
            Some(json) => Body::from(json.to_string()),
            None => Body::empty(),
        })
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    Answer {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn count(db: &DatabaseConnection, sql: &str, workspace_id: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [workspace_id.to_string().into()],
    ))
    .await
    .expect("count query")
    .expect("one row")
    .try_get::<i64>("", "n")
    .expect("n")
}

async fn compile_tasks(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
    count(
        db,
        "SELECT count(*) AS n FROM agentic_task_queue \
         WHERE spec->>'type' = 'compile' AND spec->>'workspace_id' = $1",
        workspace_id,
    )
    .await
}

async fn airway_runs(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
    count(
        db,
        "SELECT count(*) AS n FROM agentic_runs \
         WHERE source_type = 'airway' AND workspace_id::text = $1",
        workspace_id,
    )
    .await
}

/// THE case: `POST /runs` on a replica with no working copy seeds the run and
/// hands it to the worker fleet. `scope_owned = false` is what `Global` means
/// in the queue — no co-located driver owns it, so a worker claims it — and it
/// is the difference between "enqueued" and "started here".
#[tokio::test]
async fn a_start_on_a_diskless_replica_enqueues_a_run_for_the_worker_fleet() {
    let db = setup_db().await;
    let (ws_id, platform) = diskless_workspace(&db).await;
    let replica = replica(&db, platform);

    let answer = call(
        &replica,
        "POST",
        "/agentic-airway/runs",
        Some(json!({ "pipeline_ref": SERVED_REF })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let run_id = answer.json()["run_id"]
        .as_str()
        .expect("run_id")
        .to_string();

    let task = agentic_runtime::crud::get_queue_entry(&db, &run_id)
        .await
        .expect("queue lookup")
        .expect("the submit must leave a queued task behind");
    assert_eq!(task.queue_status, "queued");
    assert!(
        !task.scope_owned,
        "the run must be enqueued Global: this replica runs no workers, so a \
         Scoped task would wait for a driver that does not exist"
    );
    assert_eq!(task.spec["type"], "airway");
    assert_eq!(task.spec["pipeline_ref"], SERVED_REF);

    let run = agentic_runtime::crud::get_run(&db, &run_id)
        .await
        .expect("run lookup")
        .expect("run row");
    assert_eq!(run.workspace_id, ws_id);
    assert_eq!(compile_tasks(&db, ws_id).await, 0, "nothing to recompile");
}

/// A single-window backfill is the same enqueue with a window on it.
#[tokio::test]
async fn a_single_window_backfill_on_a_diskless_replica_enqueues_too() {
    let db = setup_db().await;
    let (_ws_id, platform) = diskless_workspace(&db).await;
    let replica = replica(&db, platform);

    let answer = call(
        &replica,
        "POST",
        "/agentic-airway/backfill",
        Some(json!({
            "pipeline_ref": SERVED_REF,
            "from": "2026-01-01T00:00:00Z",
            "to": "2026-01-02T00:00:00Z",
        })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let run_id = answer.json()["run_id"]
        .as_str()
        .expect("run_id")
        .to_string();

    let task = agentic_runtime::crud::get_queue_entry(&db, &run_id)
        .await
        .expect("queue lookup")
        .expect("queued task");
    assert!(!task.scope_owned, "Global, like a plain start");
    assert!(
        task.spec["backfill_from"].is_string() && task.spec["backfill_to"].is_string(),
        "the window rides the queued spec: {}",
        task.spec
    );
}

/// The compile boundary cannot supply the pipeline and there is no disk to
/// read: retryable, coded, and a compile is asked for — once. No run row is
/// left behind for a submit that did not happen.
#[tokio::test]
async fn a_start_for_a_ref_the_revision_does_not_serve_is_retryable_and_asks_for_a_compile() {
    let db = setup_db().await;
    let (ws_id, platform) = diskless_workspace(&db).await;
    let replica = replica(&db, platform);

    let answer = call(
        &replica,
        "POST",
        "/agentic-airway/runs",
        Some(json!({ "pipeline_ref": UNSERVED_REF })),
    )
    .await;

    answer.assert_needs_recompile("POST /runs");
    assert_eq!(airway_runs(&db, ws_id).await, 0, "no orphaned run row");
    assert_eq!(
        compile_tasks(&db, ws_id).await,
        1,
        "the retry must have a compile to wait for"
    );
}

/// Both resets and the reset's picker, for the same unserved ref. The cursor
/// pair answered `400` for this — the caller's perfectly good ref reported as
/// malformed — which stopped being harmless the moment a replica could serve
/// them. The two resets ask for a compile; the picker, a read, does not.
#[tokio::test]
async fn resets_for_a_ref_the_revision_does_not_serve_are_retryable_not_bad_requests() {
    let db = setup_db().await;
    let (_ws_id, platform) = diskless_workspace(&db).await;
    let replica = replica(&db, platform);
    let body = Some(json!({ "pipeline_ref": UNSERVED_REF }));

    call(
        &replica,
        "POST",
        "/agentic-airway/reset-cursors",
        body.clone(),
    )
    .await
    .assert_needs_recompile("POST /reset-cursors");
    call(&replica, "POST", "/agentic-airway/reset-schema", body)
        .await
        .assert_needs_recompile("POST /reset-schema");
    call(
        &replica,
        "GET",
        &format!("/agentic-airway/resource-cursors?pipeline_ref={UNSERVED_REF}"),
        None,
    )
    .await
    .assert_not_served_without_a_compile_claim("GET /resource-cursors");
}

/// The picker is a React Query fetch: it retries a 503 three times on its own
/// and refetches when the tab regains focus. Were it to ask for a compile, each
/// retry would mint and promote a fresh revision for a ref that is simply gone.
/// So a burst of reads leaves no compile task behind, while still answering
/// retryable and coded — the picker is not the caller's mistake either.
#[tokio::test]
async fn the_picker_read_never_asks_for_a_compile() {
    let db = setup_db().await;
    let (ws_id, platform) = diskless_workspace(&db).await;
    let replica = replica(&db, platform);

    for attempt in 1..=3 {
        call(
            &replica,
            "GET",
            &format!("/agentic-airway/resource-cursors?pipeline_ref={UNSERVED_REF}"),
            None,
        )
        .await
        .assert_not_served_without_a_compile_claim(&format!(
            "GET /resource-cursors, attempt {attempt}"
        ));
    }
    assert_eq!(
        compile_tasks(&db, ws_id).await,
        0,
        "a read must not buy a revision per retry"
    );
}

/// A `main` compile completed moments ago and the ref is still not served: the
/// tree has not changed, another compile of it would serve the same files, and
/// the request arriving now is a retry. The host holds the compile request —
/// no task, no claim in the body — and the answer stays a coded, retryable
/// `503`. Deleting the cooldown turns the `0` into a `1`.
#[tokio::test]
async fn a_start_inside_the_success_cooldown_is_held_and_claims_no_compile() {
    let db = setup_db().await;
    let (ws_id, platform) = diskless_workspace(&db).await;
    seed_main_compile(&db, ws_id, "ready", chrono::Utc::now().fixed_offset()).await;
    let replica = replica(&db, platform);

    let answer = call(
        &replica,
        "POST",
        "/agentic-airway/runs",
        Some(json!({ "pipeline_ref": UNSERVED_REF })),
    )
    .await;

    answer.assert_not_served_without_a_compile_claim("POST /runs inside the cooldown");
    assert_eq!(
        compile_tasks(&db, ws_id).await,
        0,
        "a compile that just succeeded must not be followed by another for the same tree"
    );
}

/// The other side of the window: the last `main` compile completed longer ago
/// than the self-heal's own retry interval, so a request may ask again — once.
/// This is the case the fixture alone cannot pin (its revision is not `main`),
/// stated with a real `main` row so a fixture change cannot move it.
#[tokio::test]
async fn a_start_past_the_success_cooldown_asks_for_a_compile_again() {
    let db = setup_db().await;
    let (ws_id, platform) = diskless_workspace(&db).await;
    let long_ago = chrono::Utc::now().fixed_offset() - chrono::Duration::minutes(10);
    seed_main_compile(&db, ws_id, "ready", long_ago).await;
    let replica = replica(&db, platform);

    let answer = call(
        &replica,
        "POST",
        "/agentic-airway/runs",
        Some(json!({ "pipeline_ref": UNSERVED_REF })),
    )
    .await;

    answer.assert_needs_recompile("POST /runs past the cooldown");
    assert_eq!(compile_tasks(&db, ws_id).await, 1);
}

/// And for a ref the revision DOES serve, the resets work with no working
/// copy: the spec comes from the boundary and everything else is Postgres. The
/// pipeline has never run, so there is nothing to clear or drop — which is the
/// point: the request got all the way through.
#[tokio::test]
async fn resets_for_a_served_ref_work_on_a_diskless_replica() {
    let db = setup_db().await;
    let (_ws_id, platform) = diskless_workspace(&db).await;
    let replica = replica(&db, platform);
    let body = Some(json!({ "pipeline_ref": SERVED_REF }));

    let picker = call(
        &replica,
        "GET",
        &format!("/agentic-airway/resource-cursors?pipeline_ref={SERVED_REF}"),
        None,
    )
    .await;
    assert_eq!(picker.status, StatusCode::OK, "{}", picker.body);
    assert_eq!(picker.json()["resources"], json!([]));

    let cursors = call(
        &replica,
        "POST",
        "/agentic-airway/reset-cursors",
        body.clone(),
    )
    .await;
    assert_eq!(cursors.status, StatusCode::OK, "{}", cursors.body);
    assert_eq!(cursors.json()["cleared"], json!([]));

    let schema = call(&replica, "POST", "/agentic-airway/reset-schema", body).await;
    assert_eq!(schema.status, StatusCode::OK, "{}", schema.body);
    assert_eq!(schema.json()["dropped_tables"], json!([]));
}

/// Cancel lands on a DIFFERENT replica from the one that accepted the submit —
/// the ordinary case behind a load balancer. That replica has never heard of
/// the run in memory. The cancel must still be recorded durably for the
/// driving worker to pick up, and must not rewrite a run a worker may hold.
#[tokio::test]
async fn a_cancel_on_another_replica_is_durable_and_does_not_fail_a_queued_run() {
    let db = setup_db().await;
    let (_ws_id, platform) = diskless_workspace(&db).await;
    let accepting = replica(&db, platform.clone());
    let other = replica(&db, platform);

    let started = call(
        &accepting,
        "POST",
        "/agentic-airway/runs",
        Some(json!({ "pipeline_ref": SERVED_REF })),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let run_id = started.json()["run_id"]
        .as_str()
        .expect("run_id")
        .to_string();

    let cancelled = call(
        &other,
        "POST",
        &format!("/agentic-airway/runs/{run_id}/cancel"),
        None,
    )
    .await;
    assert_eq!(
        cancelled.status,
        StatusCode::NO_CONTENT,
        "{}",
        cancelled.body
    );

    assert!(
        agentic_runtime::crud::is_cancel_requested(&db, &run_id)
            .await
            .expect("cancel flag"),
        "the cancel must be written where a worker in another process reads it"
    );
    let status = agentic_runtime::crud::get_run(&db, &run_id)
        .await
        .expect("run lookup")
        .expect("run row")
        .task_status;
    assert_ne!(
        status.as_deref(),
        Some("failed"),
        "a queued task means a driver holds the run or is about to; writing \
         `failed` from here would race it"
    );
}
