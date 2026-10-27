//! `oxy.ask_finished` against a real Oxy database: an Ask run tells its
//! thread's owner when it ends only when they asked; the notice reads as the
//! answer or the failure, carries the question and opens the thread; it is
//! sent once; and `POST …/analytics/runs/{id}/notify` takes the request from
//! the thread's owner only, sending at once for a run that already ended — and
//! nothing but the notice that was asked for.

use std::sync::Arc;

use agentic_http::AgenticState;
use agentic_pipeline::platform::PlatformContext;
use agentic_runtime::lifecycle::done_hook::{RunDoneObserver, set_run_done_observer};
use agentic_runtime::lifecycle::failure_hook::{RunFailureObserver, set_run_failure_observer};
use agentic_runtime::lifecycle::notice_request::{
    NoticeRequestObserver, notice_requested, request_notice, set_notice_request_observer,
};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use entity::threads;
use entity::workspaces::WorkspaceStatus;
use oxy_app::agentic_wiring::OxyThreadOwnerLookup;
use oxy_app::server::api::notifications::inbox_view;
use oxy_app::server::api::notifications::platform::ask_finished::{
    COULD_NOT, READY, notify_ask_finished,
};
use oxy_app::server::api::notifications::platform::kinds::{ASK_FAILED, ASK_FINISHED, RUN_FAILED};
use oxy_app::server::api::notifications::platform::run_failed::announce_failure;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, Statement};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

use super::{people, seed_org, seed_workspace, told};
use crate::common::{Schema, fresh_db};

pub(super) const QUESTION: &str = "What was revenue last quarter, by region?";

pub(super) async fn seed_thread(
    db: &DatabaseConnection,
    workspace: Uuid,
    owner: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    threads::ActiveModel {
        id: Set(id),
        user_id: Set(owner),
        title: Set("revenue".into()),
        input: Set(QUESTION.into()),
        output: Set(String::new()),
        source_type: Set("analytics".into()),
        source: Set("agents/a.agentic.yml".into()),
        references: Set("[]".into()),
        is_processing: Set(false),
        project_id: Set(workspace),
        sandbox_info: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed thread");
    id
}

/// A root run of `source` on `thread`, at `status`; `cancelled` sets the flag
/// every cancel path sets.
pub(super) async fn seed_run(
    db: &DatabaseConnection,
    workspace: Uuid,
    thread: Uuid,
    source: &str,
    status: &str,
    cancelled: bool,
) -> String {
    let id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO agentic_runs (id, question, thread_id, source_type, task_status, \
           workspace_id, cancel_requested_at, attempt, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $7 THEN now() END, 0, now(), now())",
        [
            id.clone().into(),
            QUESTION.into(),
            thread.into(),
            source.into(),
            status.into(),
            workspace.into(),
            cancelled.into(),
        ],
    ))
    .await
    .expect("seed run");
    id
}

pub(super) async fn ask(db: &DatabaseConnection, run: &str) {
    assert!(
        request_notice(db, run).await.expect("request"),
        "{run} exists"
    );
}

/// Told only when asked; the words are the answer or the failure, the
/// question, and the thread; once per run.
#[tokio::test]
async fn platform_notices_ask_finished_tells_the_asker_once_and_only_when_asked() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let thread = seed_thread(&db, ws, Some(org.member)).await;

    let done = seed_run(&db, ws, thread, "analytics", "done", false).await;
    assert!(
        notify_ask_finished(&db, &done).await.unwrap().is_none(),
        "nobody asked: watching it finish is enough"
    );
    assert!(told(&db, ASK_FINISHED, &done).await.is_empty());

    ask(&db, &done).await;
    let sent = notify_ask_finished(&db, &done)
        .await
        .unwrap()
        .expect("sent");
    assert_eq!(sent.recipients, 1);
    let rows = told(&db, ASK_FINISHED, &done).await;
    assert_eq!(people(&rows), vec![org.member], "the thread's owner");
    let again = notify_ask_finished(&db, &done).await.unwrap().unwrap();
    assert_eq!(
        (again.recipients, again.already_told),
        (0, 1),
        "the observer and the request route may both send: one notice"
    );

    let items = inbox_view::items(&db, rows).await.expect("inbox");
    let item = &items[0];
    assert_eq!(item.kind, "oxy.ask_finished");
    assert_eq!(item.kind_label, "Answer ready");
    assert_eq!(item.title.as_deref(), Some(READY));
    assert_eq!(item.body.as_deref(), Some(QUESTION));
    assert_eq!(
        item.href.as_deref(),
        Some(format!("/{}/workspaces/{ws}/threads/{thread}", org.slug).as_str())
    );
    assert!(item.app.is_none() && item.link.is_none());
    let actions: Vec<(String, String, bool, bool)> = item
        .actions
        .iter()
        .map(|a| {
            (
                a.id.clone(),
                a.label.clone(),
                a.destructive,
                a.runs_function,
            )
        })
        .collect();
    assert_eq!(actions, vec![("open".into(), "Open".into(), false, false)]);

    let failed = seed_run(&db, ws, thread, "analytics", "failed", false).await;
    ask(&db, &failed).await;
    notify_ask_finished(&db, &failed)
        .await
        .unwrap()
        .expect("sent");
    assert!(
        told(&db, ASK_FINISHED, &failed).await.is_empty(),
        "a failure is not an answer ready"
    );
    let rows = told(&db, ASK_FAILED, &failed).await;
    assert_eq!(people(&rows), vec![org.member]);
    let items = inbox_view::items(&db, rows).await.expect("inbox");
    assert_eq!(items[0].kind, "oxy.ask_failed");
    assert_eq!(items[0].kind_label, "Couldn't answer");
    assert_eq!(items[0].title.as_deref(), Some(COULD_NOT));
    assert_eq!(items[0].body.as_deref(), Some(QUESTION));
    assert_eq!(
        items[0].href.as_deref(),
        Some(format!("/{}/workspaces/{ws}/threads/{thread}", org.slug).as_str())
    );
    let actions: Vec<_> = items[0]
        .actions
        .iter()
        .map(|a| (a.id.as_str(), a.runs_function))
        .collect();
    assert_eq!(actions, vec![("open", false)]);
}

/// A cancel, another domain's run, a run still going and an unowned thread
/// tell nobody, asked or not.
#[tokio::test]
async fn platform_notices_ask_finished_skips_cancels_other_runs_and_unowned_threads() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let thread = seed_thread(&db, ws, Some(org.member)).await;
    let unowned = seed_thread(&db, ws, None).await;

    for (thread, source, status, cancelled, what) in [
        (thread, "analytics", "failed", true, "a cancel"),
        (thread, "builder", "done", false, "a Builder Agent run"),
        (thread, "workflow", "done", false, "an automation"),
        (thread, "analytics", "running", false, "a run still going"),
        (unowned, "analytics", "done", false, "a thread nobody owns"),
    ] {
        let run = seed_run(&db, ws, thread, source, status, cancelled).await;
        ask(&db, &run).await;
        assert!(
            notify_ask_finished(&db, &run).await.unwrap().is_none(),
            "{what}"
        );
        assert!(told(&db, ASK_FINISHED, &run).await.is_empty(), "{what}");
        assert!(told(&db, ASK_FAILED, &run).await.is_empty(), "{what}");
    }
}

/// Stands in for the boot-installed observers, which take the process's own
/// database pool: these do what each real one does, through the test's.
struct SendWith(DatabaseConnection);

#[async_trait]
impl RunDoneObserver for SendWith {
    async fn root_run_done(&self, run_id: &str) {
        notify_ask_finished(&self.0, run_id).await.expect("send");
    }
}

#[async_trait]
impl RunFailureObserver for SendWith {
    async fn root_run_failed(&self, run_id: &str) {
        announce_failure(&self.0, run_id).await;
    }
}

#[async_trait]
impl NoticeRequestObserver for SendWith {
    async fn requested_after_end(&self, run_id: &str) {
        notify_ask_finished(&self.0, run_id).await.expect("send");
    }
}

fn install_observers(db: &DatabaseConnection) {
    set_run_done_observer(Arc::new(SendWith(db.clone())));
    set_run_failure_observer(Arc::new(SendWith(db.clone())));
    set_notice_request_observer(Arc::new(SendWith(db.clone())));
}

/// The analytics router as `user`, a member of `workspace_id`, reaches it.
fn analytics_as(db: &DatabaseConnection, workspace_id: Uuid, user: Uuid) -> Router {
    let state = Arc::new(AgenticState::new(
        CancellationToken::new(),
        db.clone(),
        Arc::new(OxyThreadOwnerLookup::new(db.clone())),
    ));
    let platform: Arc<dyn PlatformContext> =
        Arc::new(crate::run_routes_workspace_scope::Platform { workspace_id });
    let user = AuthenticatedUser {
        id: user,
        email: None,
        name: "Member".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    };
    Router::new()
        .nest("/analytics", agentic_http::router(state))
        .layer(Extension(platform))
        .layer(Extension(user))
}

async fn post_notify(router: &Router, run: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/analytics/runs/{run}/notify"))
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// The owner's request is recorded; anyone else's, or one for a run that is
/// not there, is a 404 that records nothing; and a run that already ended
/// sends at once.
#[tokio::test]
async fn platform_notices_ask_finished_request_route() {
    let (db, _url) = fresh_db(Schema::All).await;
    install_observers(&db);
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let other_ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, None).await;
    let thread = seed_thread(&db, ws, Some(org.member)).await;
    let as_owner = analytics_as(&db, ws, org.member);

    let going = seed_run(&db, ws, thread, "analytics", "running", false).await;
    let (status, body) = post_notify(&analytics_as(&db, ws, org.admin), &going).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::NOT_FOUND, "run not found")
    );
    let (status, _) = post_notify(&analytics_as(&db, other_ws, org.member), &going).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a run in another workspace");
    assert!(
        !notice_requested(&db, &going).await.unwrap(),
        "refused: nothing recorded"
    );
    let (status, _) = post_notify(&as_owner, "no-such-run").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = post_notify(&as_owner, &going).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "{}"));
    assert!(notice_requested(&db, &going).await.unwrap());
    assert!(
        told(&db, ASK_FINISHED, &going).await.is_empty(),
        "still going: told when it ends"
    );
    let (status, _) = post_notify(&as_owner, &going).await;
    assert_eq!(status, StatusCode::OK, "asking twice is fine");

    let finished = seed_run(&db, ws, thread, "analytics", "done", false).await;
    let (status, _) = post_notify(&as_owner, &finished).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        people(&told(&db, ASK_FINISHED, &finished).await),
        vec![org.member],
        "it had already ended: sent at once"
    );
}

/// A request that finds its run already ended causes only the notice it asked
/// for. The failure observer also announces `oxy.run_failed` — by email and
/// Slack, to a failed automation's starter or the admins — and the owner of
/// the thread an automation ran on must not be able to send that again, a day
/// later, by asking about it.
#[tokio::test]
async fn platform_notices_ask_finished_a_late_request_sends_only_the_ask_notice() {
    let (db, _url) = fresh_db(Schema::All).await;
    install_observers(&db);
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let thread = seed_thread(&db, ws, Some(org.member)).await;
    let as_owner = analytics_as(&db, ws, org.member);

    let automation = seed_run(&db, ws, thread, "workflow", "failed", false).await;
    let (status, _) = post_notify(&as_owner, &automation).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        told(&db, RUN_FAILED, &automation).await.is_empty(),
        "a request re-sent the automation's failure notice"
    );
    assert!(
        told(&db, ASK_FINISHED, &automation).await.is_empty()
            && told(&db, ASK_FAILED, &automation).await.is_empty(),
        "an automation is not an answer"
    );

    let answer = seed_run(&db, ws, thread, "analytics", "failed", false).await;
    let (status, _) = post_notify(&as_owner, &answer).await;
    assert_eq!(status, StatusCode::OK);
    assert!(told(&db, ASK_FINISHED, &answer).await.is_empty());
    let rows = told(&db, ASK_FAILED, &answer).await;
    assert_eq!(people(&rows), vec![org.member]);
    let items = inbox_view::items(&db, rows).await.expect("inbox");
    assert_eq!(items[0].title.as_deref(), Some(COULD_NOT));
    assert!(told(&db, RUN_FAILED, &answer).await.is_empty());

    // Not vacuous: the failure observer itself does announce the automation.
    announce_failure(&db, &automation).await;
    assert!(
        !told(&db, RUN_FAILED, &automation).await.is_empty(),
        "the stand-in observer sends oxy.run_failed when the run ends"
    );
}
